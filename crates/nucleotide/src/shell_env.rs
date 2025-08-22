// ABOUTME: Comprehensive environment system following Zed's architecture for directory-specific shell environments
// ABOUTME: Handles CLI environment inheritance, directory shell capture, and LSP environment injection

use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{Duration, timeout};

/// Error types for shell environment operations
#[derive(Debug, thiserror::Error)]
pub enum ShellEnvironmentError {
    #[error("Shell execution failed: {0}")]
    ShellExecutionFailed(String),

    #[error("Shell command timed out after {0} seconds")]
    Timeout(u64),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Environment parsing failed: {0}")]
    ParseError(String),

    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),
}

/// Environment origin tracking to match Zed's approach
#[derive(Debug, Clone, PartialEq)]
pub enum EnvironmentOrigin {
    Cli,
    DirectoryShell,
    Process,
}

/// Cached shell environment result
#[derive(Debug, Clone)]
pub struct CachedEnvironment {
    pub environment: HashMap<String, String>,
    pub origin: EnvironmentOrigin,
    pub directory: PathBuf,
}

/// Central environment management system following Zed's ProjectEnvironment pattern
/// Manages CLI environment inheritance, directory-specific shell capture, and caching
pub struct ProjectEnvironment {
    /// CLI environment provided when launched from terminal (highest priority)
    cli_environment: Option<HashMap<String, String>>,

    /// Cached environments per directory path
    directory_environments: Arc<RwLock<HashMap<PathBuf, CachedEnvironment>>>,

    /// Cached error messages per directory  
    environment_errors: Arc<RwLock<HashMap<PathBuf, String>>>,

    /// Semaphore to limit concurrent shell executions
    shell_execution_semaphore: Arc<Semaphore>,
}

impl ProjectEnvironment {
    /// Create new ProjectEnvironment with optional CLI environment
    /// CLI environment takes highest precedence when provided
    pub fn new(cli_environment: Option<HashMap<String, String>>) -> Self {
        let cli_env = cli_environment.map(|mut env| {
            // Add origin marker for CLI environment
            env.insert("ZED_ENVIRONMENT".to_string(), "cli".to_string());
            env
        });

        Self {
            cli_environment: cli_env,
            directory_environments: Arc::new(RwLock::new(HashMap::new())),
            environment_errors: Arc::new(RwLock::new(HashMap::new())),
            shell_execution_semaphore: Arc::new(Semaphore::new(3)), // Limit concurrent shell executions
        }
    }

    /// Get environment for directory following priority: CLI > directory shell > process
    #[instrument(skip(self), fields(directory = %directory.display()))]
    pub async fn get_environment_for_directory(
        &self,
        directory: &Path,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        // Priority 1: CLI environment has highest priority but merges with process environment
        if let Some(cli_env) = &self.cli_environment {
            debug!("Using CLI environment (highest priority)");

            // Start with process environment, then let CLI override
            let mut combined_env: HashMap<String, String> = std::env::vars().collect();

            // CLI environment takes precedence over process environment
            for (key, value) in cli_env {
                combined_env.insert(key.clone(), value.clone());
            }

            return Ok(combined_env);
        }

        // Priority 2: Directory-specific shell environment (cached)
        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        // Check cache first
        {
            let cache = self.directory_environments.read().await;
            if let Some(cached) = cache.get(&canonical_dir) {
                debug!("Using cached directory environment");
                return Ok(cached.environment.clone());
            }
        }

        // Load directory environment in background
        debug!("Loading directory-specific shell environment");
        self.load_directory_environment(&canonical_dir).await
    }

    /// Load shell environment for specific directory with proper cd command
    async fn load_directory_environment(
        &self,
        directory: &PathBuf,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        // Acquire semaphore to limit concurrent shell executions
        let _permit = self
            .shell_execution_semaphore
            .acquire()
            .await
            .map_err(|_| {
                ShellEnvironmentError::ShellExecutionFailed(
                    "Failed to acquire execution semaphore".to_string(),
                )
            })?;

        // Get user's shell
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        debug!(shell = %shell, directory = %directory.display(), "Executing shell environment capture");

        // Build shell-specific command
        let command = shell_command_builder::build_environment_capture_command(&shell, directory)?;

        // Convert to tokio command for async execution
        let mut tokio_command = tokio::process::Command::new(command.get_program());
        for arg in command.get_args() {
            tokio_command.arg(arg);
        }
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                tokio_command.env(key, value);
            } else {
                tokio_command.env_remove(key);
            }
        }
        if let Some(dir) = command.get_current_dir() {
            tokio_command.current_dir(dir);
        }

        // Execute with timeout - use 2 seconds to ensure we timeout before caller's 3-second limit
        let result = timeout(Duration::from_secs(2), tokio_command.output()).await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    let directory_env = parse_shell_environment(&output.stdout)?;

                    // Start with process environment as base, then let directory shell override
                    let mut combined_env: HashMap<String, String> = std::env::vars().collect();

                    // Directory shell environment takes precedence over process environment
                    for (key, value) in directory_env {
                        combined_env.insert(key, value);
                    }

                    // Add origin marker for directory shell environment
                    combined_env
                        .insert("ZED_ENVIRONMENT".to_string(), "worktree-shell".to_string());

                    // Cache successful result
                    let cached_env = CachedEnvironment {
                        environment: combined_env.clone(),
                        origin: EnvironmentOrigin::DirectoryShell,
                        directory: directory.clone(),
                    };

                    {
                        let mut cache = self.directory_environments.write().await;
                        cache.insert(directory.clone(), cached_env);
                    }

                    // Clear any previous errors
                    {
                        let mut errors = self.environment_errors.write().await;
                        errors.remove(directory);
                    }

                    info!(
                        directory = %directory.display(),
                        env_count = combined_env.len(),
                        "Successfully loaded directory shell environment"
                    );

                    Ok(combined_env)
                } else {
                    let error_msg = String::from_utf8_lossy(&output.stderr).to_string();
                    let error = ShellEnvironmentError::ShellExecutionFailed(error_msg.clone());

                    // Cache error
                    {
                        let mut errors = self.environment_errors.write().await;
                        errors.insert(directory.clone(), error_msg);
                    }

                    Err(error)
                }
            }
            Ok(Err(io_error)) => {
                let error = ShellEnvironmentError::IoError(io_error);
                error!(error = %error, "IO error during shell execution");
                Err(error)
            }
            Err(_timeout_error) => {
                let error = ShellEnvironmentError::Timeout(2);
                warn!(directory = %directory.display(), "Shell environment capture timed out");

                // Cache timeout error
                {
                    let mut errors = self.environment_errors.write().await;
                    errors.insert(
                        directory.clone(),
                        "Shell environment capture timed out".to_string(),
                    );
                }

                Err(error)
            }
        }
    }

    /// Get environment specifically for LSP servers (may include LSP-specific variables)
    pub async fn get_lsp_environment(
        &self,
        directory: &Path,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        // For now, LSP environment is the same as directory environment
        // This can be extended later to include LSP-specific variables
        self.get_environment_for_directory(directory).await
    }

    /// Get LSP environment with additional overrides
    pub async fn get_lsp_environment_with_overrides(
        &self,
        directory: &Path,
        lsp_overrides: HashMap<String, String>,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        let mut env = self.get_environment_for_directory(directory).await?;

        // LSP-specific variables override project environment
        for (key, value) in lsp_overrides {
            env.insert(key, value);
        }

        Ok(env)
    }

    /// Get cached directories for testing/debugging
    pub async fn get_cached_directories(&self) -> Vec<PathBuf> {
        let cache = self.directory_environments.read().await;
        cache.keys().cloned().collect()
    }

    /// Invalidate cache for specific directory
    pub async fn invalidate_directory_cache(&self, directory: &Path) {
        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        debug!(directory = %canonical_dir.display(), "Invalidating directory environment cache");

        {
            let mut cache = self.directory_environments.write().await;
            cache.remove(&canonical_dir);
        }

        {
            let mut errors = self.environment_errors.write().await;
            errors.remove(&canonical_dir);
        }
    }

    /// Clear all cached environments
    pub async fn clear_all_caches(&self) {
        debug!("Clearing all environment caches");

        {
            let mut cache = self.directory_environments.write().await;
            cache.clear();
        }

        {
            let mut errors = self.environment_errors.write().await;
            errors.clear();
        }
    }
}

/// Shell command building utilities following Zed's approach
pub mod shell_command_builder {
    use super::*;

    /// Build shell command for environment capture with directory context
    /// Mirrors Zed's approach with cd command and shell-specific handling
    pub fn build_environment_capture_command(
        shell: &str,
        directory: &Path,
    ) -> Result<Command, ShellEnvironmentError> {
        let shell_name = detect_shell_type(shell);
        let mut command = Command::new(shell);

        // Build command string with cd to trigger directory hooks (direnv, asdf, etc.)
        let mut command_string = format!("cd '{}' && ", directory.display());

        // Use printenv to capture environment variables
        command_string.push_str("printenv -0"); // Use null separators for reliable parsing

        match shell_name {
            "fish" => {
                // Fish requires special handling to trigger hooks
                command_string = format!(
                    "cd '{}'; emit fish_prompt; printenv -0",
                    directory.display()
                );
                command.arg("-l").arg("-c").arg(command_string);
            }
            "tcsh" | "csh" => {
                // tcsh/csh should use arg0 technique, but std::process::Command doesn't support it
                // Use -c without -l flag (shell will inherit login status from parent)
                command.arg("-c").arg(command_string);
            }
            "nu" => {
                // Nushell requires ^ prefix for external commands
                let nu_command = format!("cd '{}'; ^printenv -0", directory.display());
                command.arg("-l").arg("-c").arg(nu_command);
            }
            _ => {
                // Default shells (bash, zsh) use standard -l flag
                command.arg("-l").arg("-c").arg(command_string);
            }
        }

        Ok(command)
    }

    /// Build a generic shell command (for testing)
    pub fn build_shell_command(shell: &str, directory: &Path) -> Command {
        let mut command = Command::new(shell);
        let shell_name = detect_shell_type(shell);

        match shell_name {
            "fish" => {
                // Fish requires special handling to trigger hooks properly
                command.arg("-l").arg("-c").arg("emit fish_prompt");
            }
            "tcsh" | "csh" => {
                // tcsh/csh should use arg0 technique for login shell, but std::process::Command
                // doesn't support setting arg0. The shell will inherit login status from parent.
                // Note: No flags added - tcsh/csh don't use -l flag
            }
            _ => {
                command.arg("-l");
            }
        }

        command
    }
}

/// Detect shell type from shell path
pub fn detect_shell_type(shell_path: &str) -> &'static str {
    let shell_name = std::path::Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");

    match shell_name {
        "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "tcsh" => "tcsh",
        "csh" => "csh",
        "nu" => "nu",
        _ => "unknown",
    }
}

/// Parse shell environment output (null-separated key=value pairs)
pub fn parse_shell_environment(
    output: &[u8],
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let output_str = String::from_utf8_lossy(output);
    let mut env_map = HashMap::new();

    // Split on null bytes for reliable parsing
    for line in output_str.split('\0') {
        if line.is_empty() {
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].to_string();
            let value = line[eq_pos + 1..].to_string();
            env_map.insert(key, value);
        }
    }

    if env_map.is_empty() {
        return Err(ShellEnvironmentError::ParseError(
            "No environment variables found".to_string(),
        ));
    }

    Ok(env_map)
}

/// Legacy compatibility: Maintain existing function signature for current code
pub async fn capture_shell_environment(
    directory: &Path,
) -> Result<HashMap<String, String>, ShellEnvError> {
    // Create a temporary ProjectEnvironment without CLI env for compatibility
    let project_env = ProjectEnvironment::new(None);

    match project_env.get_environment_for_directory(directory).await {
        Ok(env) => Ok(env),
        Err(ShellEnvironmentError::ShellExecutionFailed(msg)) => {
            Err(ShellEnvError::CommandFailed(msg))
        }
        Err(ShellEnvironmentError::Timeout(_)) => Err(ShellEnvError::Timeout),
        Err(ShellEnvironmentError::IoError(e)) => Err(ShellEnvError::IoError(e)),
        Err(ShellEnvironmentError::ParseError(msg)) => Err(ShellEnvError::ParseError(msg)),
        Err(ShellEnvironmentError::DirectoryNotFound(msg)) => Err(ShellEnvError::ParseError(msg)),
    }
}

/// Legacy error type for compatibility
#[derive(Debug, thiserror::Error)]
pub enum ShellEnvError {
    #[error("Shell command timed out after 10 seconds")]
    Timeout,

    #[error("IO error running shell command: {0}")]
    IoError(std::io::Error),

    #[error("Shell command failed: {0}")]
    CommandFailed(String),

    #[error("Failed to parse shell output: {0}")]
    ParseError(String),
}

/// Legacy cache structure for compatibility
pub struct ShellEnvironmentCache {
    project_env: Arc<ProjectEnvironment>,
}

impl Default for ShellEnvironmentCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellEnvironmentCache {
    pub fn new() -> Self {
        Self {
            project_env: Arc::new(ProjectEnvironment::new(None)),
        }
    }

    /// Get environment for directory, using cache if available
    pub async fn get_environment(
        &mut self,
        directory: &Path,
    ) -> Result<HashMap<String, String>, ShellEnvError> {
        match self
            .project_env
            .get_environment_for_directory(directory)
            .await
        {
            Ok(env) => Ok(env),
            Err(ShellEnvironmentError::ShellExecutionFailed(msg)) => {
                Err(ShellEnvError::CommandFailed(msg))
            }
            Err(ShellEnvironmentError::Timeout(_)) => Err(ShellEnvError::Timeout),
            Err(ShellEnvironmentError::IoError(e)) => Err(ShellEnvError::IoError(e)),
            Err(ShellEnvironmentError::ParseError(msg)) => Err(ShellEnvError::ParseError(msg)),
            Err(ShellEnvironmentError::DirectoryNotFound(msg)) => {
                Err(ShellEnvError::ParseError(msg))
            }
        }
    }

    /// Clear the cache (useful when directory-specific tools change)
    pub async fn clear_cache(&mut self) {
        self.project_env.clear_all_caches().await;
    }

    /// Clear cache for specific directory
    pub async fn clear_directory_cache(&mut self, directory: &Path) {
        self.project_env.invalidate_directory_cache(directory).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test;

    #[tokio::test]
    async fn test_shell_detection() {
        assert_eq!(detect_shell_type("/bin/bash"), "bash");
        assert_eq!(detect_shell_type("/usr/local/bin/fish"), "fish");
        assert_eq!(detect_shell_type("/bin/tcsh"), "tcsh");
        assert_eq!(detect_shell_type("/bin/csh"), "csh");
        assert_eq!(detect_shell_type("/usr/local/bin/zsh"), "zsh");
        assert_eq!(detect_shell_type("/unknown/shell"), "unknown");
    }

    #[tokio::test]
    async fn test_project_environment_creation() {
        let cli_env = HashMap::from([("PATH".to_string(), "/test/path".to_string())]);

        let project_env = ProjectEnvironment::new(Some(cli_env));
        assert!(project_env.cli_environment.is_some());

        let project_env_no_cli = ProjectEnvironment::new(None);
        assert!(project_env_no_cli.cli_environment.is_none());
    }

    #[tokio::test]
    async fn test_environment_parsing() {
        let test_output = b"PATH=/usr/bin:/bin\0HOME=/Users/test\0SHELL=/bin/bash\0";
        let parsed = parse_shell_environment(test_output).unwrap();

        assert_eq!(parsed.get("PATH"), Some(&"/usr/bin:/bin".to_string()));
        assert_eq!(parsed.get("HOME"), Some(&"/Users/test".to_string()));
        assert_eq!(parsed.get("SHELL"), Some(&"/bin/bash".to_string()));
    }
}
