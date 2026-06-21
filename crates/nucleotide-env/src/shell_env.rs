// ABOUTME: Comprehensive environment system following Zed's architecture for directory-specific shell environments
// ABOUTME: Handles CLI environment inheritance, directory shell capture, and LSP environment injection

use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;
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

    #[error("Unsupported .envrc for native flake loading: {0}")]
    EnvrcUnsupported(String),

    #[error("nix print-dev-env failed: {0}")]
    NixPrintDevEnvFailed(String),

    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),
}

/// Environment origin tracking to match Zed's approach
#[derive(Debug, Clone, PartialEq)]
pub enum EnvironmentOrigin {
    Cli,
    NativeFlake,
    DirectoryShell,
    Process,
}

/// Cached shell environment result
#[derive(Debug, Clone)]
pub struct CachedEnvironment {
    pub environment: HashMap<String, String>,
    pub origin: EnvironmentOrigin,
    pub directory: PathBuf,
    native_watch_state: Option<Vec<WatchedFileState>>,
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
        let baseline_env = self.baseline_environment();

        // Priority 1: Directory-specific environment (cached)
        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        // Check cache first. Native flake environments are invalidated when any
        // watched input changes so project switches and lockfile updates reload.
        if let Some(cached) = {
            let cache = self.directory_environments.read().await;
            cache.get(&canonical_dir).cloned()
        } {
            if cached_environment_is_current(&cached) {
                debug!("Using cached directory environment");
                return Ok(cached.environment);
            }

            debug!(
                directory = %canonical_dir.display(),
                origin = ?cached.origin,
                "Cached directory environment is stale"
            );
            let mut cache = self.directory_environments.write().await;
            cache.remove(&canonical_dir);
        }

        // Priority 2: Native `.envrc` subset for `use flake`.
        match self
            .load_native_flake_environment(&canonical_dir, &baseline_env)
            .await
        {
            Ok(Some(native_env)) => {
                let cached_env = CachedEnvironment {
                    environment: native_env.environment.clone(),
                    origin: EnvironmentOrigin::NativeFlake,
                    directory: canonical_dir.clone(),
                    native_watch_state: Some(native_env.watch_state.clone()),
                };

                {
                    let mut cache = self.directory_environments.write().await;
                    cache.insert(canonical_dir.clone(), cached_env);
                }

                {
                    let mut errors = self.environment_errors.write().await;
                    errors.remove(&canonical_dir);
                }

                info!(
                    directory = %canonical_dir.display(),
                    env_count = native_env.environment.len(),
                    watched_files = native_env.watch_state.len(),
                    "Successfully loaded native flake environment"
                );

                return Ok(native_env.environment);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    directory = %canonical_dir.display(),
                    error = %error,
                    "Native flake environment loading skipped"
                );
            }
        }

        // If the app was launched with a full CLI environment and there is no supported
        // native project environment, use that baseline directly. Shell capture remains
        // the fallback for dock launches and unsupported direnv/asdf/mise integrations.
        if self.cli_environment.is_some() {
            debug!("Using CLI environment baseline");
            return Ok(baseline_env);
        }

        // Priority 3: Directory shell environment.
        debug!("Loading directory-specific shell environment");
        self.load_directory_environment(&canonical_dir, baseline_env)
            .await
    }

    fn baseline_environment(&self) -> HashMap<String, String> {
        let mut combined_env: HashMap<String, String> = std::env::vars().collect();

        if let Some(cli_env) = &self.cli_environment {
            for (key, value) in cli_env {
                combined_env.insert(key.clone(), value.clone());
            }
        }

        combined_env
    }

    /// Load shell environment for specific directory with proper cd command
    async fn load_directory_environment(
        &self,
        directory: &PathBuf,
        baseline_env: HashMap<String, String>,
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

        let shell = default_environment_shell();

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

                    let baseline_path = baseline_env.get("PATH").cloned();
                    let mut combined_env = baseline_env;

                    // Directory shell environment takes precedence over process environment
                    for (key, value) in directory_env {
                        combined_env.insert(key, value);
                    }
                    merge_path_like_var(&mut combined_env, "PATH", baseline_path.as_deref());

                    // Add origin marker for directory shell environment
                    combined_env
                        .insert("ZED_ENVIRONMENT".to_string(), "worktree-shell".to_string());

                    // Cache successful result
                    let cached_env = CachedEnvironment {
                        environment: combined_env.clone(),
                        origin: EnvironmentOrigin::DirectoryShell,
                        directory: directory.clone(),
                        native_watch_state: None,
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

    async fn load_native_flake_environment(
        &self,
        directory: &Path,
        baseline_env: &HashMap<String, String>,
    ) -> Result<Option<NativeFlakeEnvironment>, ShellEnvironmentError> {
        let envrc_path = directory.join(".envrc");
        if !envrc_path.is_file() {
            return Ok(None);
        }

        let envrc = fs::read_to_string(&envrc_path)?;
        let Some(plan) = parse_native_flake_envrc(&envrc).map_err(|error| {
            ShellEnvironmentError::EnvrcUnsupported(format!(
                "{} in {}",
                error,
                envrc_path.display()
            ))
        })?
        else {
            return Ok(None);
        };

        let exported = run_nix_print_dev_env(directory, &plan).await?;
        let mut environment = merge_native_flake_environment(baseline_env, exported);
        environment.insert("ZED_ENVIRONMENT".to_string(), "native-flake".to_string());

        let watch_paths = native_flake_watch_paths(directory, &envrc_path, &plan);

        Ok(Some(NativeFlakeEnvironment {
            environment,
            watch_state: snapshot_watched_files(watch_paths),
        }))
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

    /// Get the cached origin for a directory if its environment is current.
    pub async fn get_cached_origin(&self, directory: &Path) -> Option<EnvironmentOrigin> {
        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        let cache = self.directory_environments.read().await;
        cache
            .get(&canonical_dir)
            .filter(|cached| cached_environment_is_current(cached))
            .map(|cached| cached.origin.clone())
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

#[derive(Debug)]
struct NativeFlakeEnvironment {
    environment: HashMap<String, String>,
    watch_state: Vec<WatchedFileState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeFlakePlan {
    flake_args: Vec<String>,
    watched_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchedFileState {
    path: PathBuf,
    state: WatchedPathState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WatchedPathState {
    Present {
        modified: Option<SystemTime>,
        len: u64,
    },
    Missing,
}

fn cached_environment_is_current(cached: &CachedEnvironment) -> bool {
    match cached.origin {
        EnvironmentOrigin::NativeFlake => cached
            .native_watch_state
            .as_deref()
            .is_some_and(watched_files_are_current),
        EnvironmentOrigin::Cli | EnvironmentOrigin::DirectoryShell | EnvironmentOrigin::Process => {
            true
        }
    }
}

fn watched_files_are_current(watched_files: &[WatchedFileState]) -> bool {
    watched_files
        .iter()
        .all(|watched| watched.state == snapshot_watched_file(&watched.path))
}

fn snapshot_watched_files(paths: Vec<PathBuf>) -> Vec<WatchedFileState> {
    paths
        .into_iter()
        .map(|path| WatchedFileState {
            state: snapshot_watched_file(&path),
            path,
        })
        .collect()
}

fn snapshot_watched_file(path: &Path) -> WatchedPathState {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => WatchedPathState::Present {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        },
        _ => WatchedPathState::Missing,
    }
}

fn native_flake_watch_paths(
    directory: &Path,
    envrc_path: &Path,
    plan: &NativeFlakePlan,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_path(&mut paths, envrc_path.to_path_buf());
    push_unique_path(&mut paths, directory.join("flake.nix"));
    push_unique_path(&mut paths, directory.join("flake.lock"));

    for path in &plan.watched_files {
        push_unique_path(&mut paths, directory.join(path));
    }

    paths
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

const NIX_DIRENV_RESTORED_VARS: &[&str] = &[
    "NIX_BUILD_TOP",
    "TMP",
    "TMPDIR",
    "TEMP",
    "TEMPDIR",
    "terminfo",
];

fn merge_native_flake_environment(
    baseline: &HashMap<String, String>,
    exported: HashMap<String, String>,
) -> HashMap<String, String> {
    let mut environment = baseline.clone();
    for (key, value) in exported {
        environment.insert(key, value);
    }

    for key in NIX_DIRENV_RESTORED_VARS {
        restore_baseline_var(&mut environment, baseline, key);
    }

    match merge_colon_separated_paths(
        environment.get("XDG_DATA_DIRS").map(String::as_str),
        baseline.get("XDG_DATA_DIRS").map(String::as_str),
    ) {
        Some(value) => {
            environment.insert("XDG_DATA_DIRS".to_string(), value);
        }
        None => {
            environment.remove("XDG_DATA_DIRS");
        }
    }

    merge_path_like_var(
        &mut environment,
        "PATH",
        baseline.get("PATH").map(String::as_str),
    );

    environment
}

fn restore_baseline_var(
    environment: &mut HashMap<String, String>,
    baseline: &HashMap<String, String>,
    key: &str,
) {
    if let Some(value) = baseline.get(key) {
        environment.insert(key.to_string(), value.clone());
    } else {
        environment.remove(key);
    }
}

fn merge_path_like_var(
    environment: &mut HashMap<String, String>,
    key: &str,
    baseline_value: Option<&str>,
) {
    match merge_colon_separated_paths(environment.get(key).map(String::as_str), baseline_value) {
        Some(value) => {
            environment.insert(key.to_string(), value);
        }
        None => {
            environment.remove(key);
        }
    }
}

fn merge_colon_separated_paths(new_value: Option<&str>, old_value: Option<&str>) -> Option<String> {
    let mut dirs: Vec<&str> = Vec::new();

    for value in [new_value, old_value].into_iter().flatten() {
        for dir in value.split(':') {
            let dir = normalize_colon_path_entry(dir);
            if !dir.is_empty() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }

    (!dirs.is_empty()).then(|| dirs.join(":"))
}

fn normalize_colon_path_entry(dir: &str) -> &str {
    if dir == "/" {
        dir
    } else {
        dir.trim_end_matches('/')
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum EnvrcParseError {
    #[error("line {line}: {message}")]
    Unsupported { line: usize, message: String },
}

fn parse_native_flake_envrc(contents: &str) -> Result<Option<NativeFlakePlan>, EnvrcParseError> {
    let mut flake_args: Option<Vec<String>> = None;
    let mut watched_files = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let words = split_envrc_words(line).map_err(|message| EnvrcParseError::Unsupported {
            line: line_number,
            message,
        })?;

        if words.is_empty() {
            continue;
        }

        match words.as_slice() {
            [use_cmd, flake, args @ ..] if use_cmd == "use" && flake == "flake" => {
                if flake_args.is_some() {
                    return Err(EnvrcParseError::Unsupported {
                        line: line_number,
                        message: "multiple use flake declarations are not supported".to_string(),
                    });
                }
                flake_args = Some(normalize_flake_args(args));
            }
            [use_flake, args @ ..] if use_flake == "use_flake" => {
                if flake_args.is_some() {
                    return Err(EnvrcParseError::Unsupported {
                        line: line_number,
                        message: "multiple use flake declarations are not supported".to_string(),
                    });
                }
                flake_args = Some(normalize_flake_args(args));
            }
            [watch_file, paths @ ..] if watch_file == "watch_file" && !paths.is_empty() => {
                watched_files.extend(paths.iter().map(PathBuf::from));
            }
            _ => {
                return Err(EnvrcParseError::Unsupported {
                    line: line_number,
                    message: format!("unsupported command `{}`", words[0]),
                });
            }
        }
    }

    Ok(flake_args.map(|flake_args| NativeFlakePlan {
        flake_args,
        watched_files,
    }))
}

fn normalize_flake_args(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec![".".to_string()]
    } else {
        args.to_vec()
    }
}

fn split_envrc_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        return Err("unterminated escape sequence".to_string());
                    }
                }
                _ => current.push(ch),
            },
            Some(_) => unreachable!("only single and double quotes are used"),
            None => match ch {
                '#' if current.is_empty() => break,
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        return Err("unterminated escape sequence".to_string());
                    }
                }
                ch if ch.is_whitespace() => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if let Some(quote) = quote {
        return Err(format!("unterminated {} quote", quote));
    }

    if !current.is_empty() {
        words.push(current);
    }

    Ok(words)
}

async fn run_nix_print_dev_env(
    directory: &Path,
    plan: &NativeFlakePlan,
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    run_nix_print_dev_env_with_binary(directory, plan, Path::new("nix")).await
}

async fn run_nix_print_dev_env_with_binary(
    directory: &Path,
    plan: &NativeFlakePlan,
    nix_binary: &Path,
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let profile = native_flake_profile_path(directory);
    if let Some(parent) = profile.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut command = tokio::process::Command::new(nix_binary);
    command
        .current_dir(directory)
        .arg("--extra-experimental-features")
        .arg("nix-command flakes")
        .arg("print-dev-env")
        .arg("--json")
        .arg("--no-pretty")
        .arg("--profile")
        .arg(&profile);

    for arg in &plan.flake_args {
        command.arg(arg);
    }

    let result = timeout(Duration::from_secs(30), command.output()).await;
    let output = match result {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(ShellEnvironmentError::IoError(error)),
        Err(_) => return Err(ShellEnvironmentError::Timeout(30)),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ShellEnvironmentError::NixPrintDevEnvFailed(
            stderr.trim().to_string(),
        ));
    }

    let env = parse_nix_print_dev_env_json(&output.stdout)?;
    wipe_native_flake_profile_history(nix_binary, &profile).await;
    Ok(env)
}

async fn wipe_native_flake_profile_history(nix_binary: &Path, profile: &Path) {
    let mut command = tokio::process::Command::new(nix_binary);
    command
        .arg("--extra-experimental-features")
        .arg("nix-command flakes")
        .arg("profile")
        .arg("wipe-history")
        .arg("--profile")
        .arg(profile);

    match timeout(Duration::from_secs(10), command.output()).await {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => {
            warn!(
                profile = %profile.display(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "Failed to wipe native flake profile history"
            );
        }
        Ok(Err(error)) => {
            warn!(
                profile = %profile.display(),
                error = %error,
                "Failed to run nix profile wipe-history"
            );
        }
        Err(_) => {
            warn!(
                profile = %profile.display(),
                "Timed out wiping native flake profile history"
            );
        }
    }
}

fn parse_nix_print_dev_env_json(
    output: &[u8],
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let json: serde_json::Value = serde_json::from_slice(output)
        .map_err(|error| ShellEnvironmentError::ParseError(error.to_string()))?;
    let variables = json
        .get("variables")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ShellEnvironmentError::ParseError("missing variables object".to_string()))?;

    let mut env = HashMap::new();
    for (key, entry) in variables {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            continue;
        }

        let is_exported = entry
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "exported");
        if !is_exported {
            continue;
        }

        if let Some(value) = entry.get("value").and_then(serde_json::Value::as_str) {
            env.insert(key.clone(), value.to_string());
        }
    }

    if env.is_empty() {
        return Err(ShellEnvironmentError::ParseError(
            "nix print-dev-env produced no exported string variables".to_string(),
        ));
    }

    Ok(env)
}

fn native_flake_profile_path(directory: &Path) -> PathBuf {
    let key = stable_hash_hex(directory.to_string_lossy().as_bytes());
    nucleotide_cache_dir()
        .join("native-flake-env")
        .join(key)
        .join("flake-profile")
}

fn nucleotide_cache_dir() -> PathBuf {
    if let Some(path) = non_empty_env_path("NUCLEOTIDE_CACHE_DIR") {
        return path;
    }

    if let Some(path) = non_empty_env_path("XDG_CACHE_HOME") {
        return path.join("nucleotide");
    }

    if let Some(home) = non_empty_env_path("HOME") {
        return home.join(".cache").join("nucleotide");
    }

    env::temp_dir().join("nucleotide")
}

fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    // FNV-1a is sufficient here: this is only a stable cache directory key.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

        if matches!(shell_name, "powershell" | "pwsh") {
            let escaped_dir = quote_path_for_powershell_literal(directory);
            let command_string = format!(
                "Set-Location -LiteralPath {}; $envText = ((Get-ChildItem Env:) | ForEach-Object {{ \"$($_.Name)=$($_.Value)\" }}) -join [char]0; $envText += [char]0; $bytes = [System.Text.Encoding]::UTF8.GetBytes($envText); [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)",
                escaped_dir
            );
            command
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(command_string);
            return Ok(command);
        }

        if shell_name == "cmd" {
            let escaped_dir = quote_path_for_cmd(directory);
            command
                .arg("/d")
                .arg("/c")
                .arg(format!("cd /d {} && set", escaped_dir));
            return Ok(command);
        }

        // Quote directory path for POSIX shells to avoid breaking on characters like '
        let escaped_dir = quote_path_for_shell(directory);

        // Build command string with cd to trigger directory hooks (direnv, asdf, etc.)
        let mut command_string = format!("cd {} && ", escaped_dir);

        // Use `env -0` to capture environment variables with null separators (portable)
        command_string.push_str("env -0");

        match shell_name {
            "fish" => {
                // Fish requires special handling to trigger hooks
                command_string = format!("cd {}; emit fish_prompt; env -0", escaped_dir);
                command.arg("-l").arg("-c").arg(command_string);
            }
            "tcsh" | "csh" => {
                // tcsh/csh should use arg0 technique, but std::process::Command doesn't support it
                // Use -c without -l flag (shell will inherit login status from parent)
                command.arg("-c").arg(command_string);
            }
            "nu" => {
                // Nushell requires ^ prefix for external commands
                let nu_command = format!("cd {}; ^env -0", escaped_dir);
                command.arg("-l").arg("-c").arg(nu_command);
            }
            _ => {
                // Default shells (bash, zsh) use standard -l flag
                command.arg("-l").arg("-c").arg(command_string);
            }
        }

        Ok(command)
    }

    pub(crate) fn quote_path_for_shell(path: &Path) -> String {
        let path_str = path.to_string_lossy();
        let mut quoted = String::with_capacity(path_str.len() + 2);
        quoted.push('\'');
        for ch in path_str.chars() {
            if ch == '\'' {
                quoted.push_str("'\"'\"'");
            } else {
                quoted.push(ch);
            }
        }
        quoted.push('\'');
        quoted
    }

    pub(crate) fn quote_path_for_powershell_literal(path: &Path) -> String {
        let path_str = path.to_string_lossy();
        let mut quoted = String::with_capacity(path_str.len() + 2);
        quoted.push('\'');
        for ch in path_str.chars() {
            if ch == '\'' {
                quoted.push('\'');
            }
            quoted.push(ch);
        }
        quoted.push('\'');
        quoted
    }

    pub(crate) fn quote_path_for_cmd(path: &Path) -> String {
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    }

    /// Build a generic shell command (for testing)
    pub fn build_shell_command(shell: &str) -> Command {
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
        .file_stem()
        .or_else(|| std::path::Path::new(shell_path).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_ascii_lowercase();

    match shell_name.as_str() {
        "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "tcsh" => "tcsh",
        "csh" => "csh",
        "nu" => "nu",
        "powershell" => "powershell",
        "pwsh" => "pwsh",
        "cmd" => "cmd",
        _ => "unknown",
    }
}

fn default_environment_shell() -> String {
    #[cfg(windows)]
    {
        windows_shell::system_shell()
    }

    #[cfg(not(windows))]
    {
        env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

#[cfg(windows)]
mod windows_shell {
    use std::path::PathBuf;
    use std::sync::LazyLock;

    pub(super) fn system_shell() -> String {
        static SYSTEM_SHELL: LazyLock<String> = LazyLock::new(detect_system_shell);
        (*SYSTEM_SHELL).clone()
    }

    fn detect_system_shell() -> String {
        for path in [
            find_pwsh_in_programfiles(false, false),
            find_pwsh_in_programfiles(true, false),
            find_pwsh_in_msix(false),
            find_pwsh_in_programfiles(false, true),
            find_pwsh_in_msix(true),
            find_pwsh_in_programfiles(true, true),
            find_pwsh_in_scoop(),
            which::which_global("pwsh.exe").ok(),
            which::which_global("powershell.exe").ok(),
        ]
        .into_iter()
        .flatten()
        {
            return path.to_string_lossy().trim().to_string();
        }

        std::env::var("COMSPEC")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "cmd.exe".to_string())
    }

    fn find_pwsh_in_programfiles(find_alternate: bool, find_preview: bool) -> Option<PathBuf> {
        #[cfg(target_pointer_width = "64")]
        let env_var = if find_alternate {
            "ProgramFiles(x86)"
        } else {
            "ProgramFiles"
        };

        #[cfg(target_pointer_width = "32")]
        let env_var = if find_alternate {
            "ProgramW6432"
        } else {
            "ProgramFiles"
        };

        let install_base_dir = PathBuf::from(std::env::var_os(env_var)?).join("PowerShell");
        install_base_dir
            .read_dir()
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let dir_name = dir_name.to_string_lossy();
                let version = if find_preview {
                    let dash_index = dir_name.find('-')?;
                    if &dir_name[dash_index + 1..] != "preview" {
                        return None;
                    }
                    dir_name[..dash_index].parse::<u32>().ok()?
                } else {
                    dir_name.parse::<u32>().ok()?
                };

                let exe_path = entry.path().join("pwsh.exe");
                exe_path.exists().then_some((version, exe_path))
            })
            .max_by_key(|(version, _)| *version)
            .map(|(_, path)| path)
    }

    fn find_pwsh_in_msix(find_preview: bool) -> Option<PathBuf> {
        let msix_app_dir =
            PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Microsoft\\WindowsApps");
        if !msix_app_dir.exists() {
            return None;
        }

        let prefix = if find_preview {
            "Microsoft.PowerShellPreview_"
        } else {
            "Microsoft.PowerShell_"
        };

        msix_app_dir
            .read_dir()
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
            .find_map(|entry| {
                if !entry.file_name().to_string_lossy().starts_with(prefix) {
                    return None;
                }

                let exe_path = entry.path().join("pwsh.exe");
                exe_path.exists().then_some(exe_path)
            })
    }

    fn find_pwsh_in_scoop() -> Option<PathBuf> {
        let pwsh_exe =
            PathBuf::from(std::env::var_os("USERPROFILE")?).join("scoop\\shims\\pwsh.exe");
        pwsh_exe.exists().then_some(pwsh_exe)
    }
}

/// Parse shell environment output (null-separated key=value pairs)
pub fn parse_shell_environment(
    output: &[u8],
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let output_str = String::from_utf8_lossy(output);
    let mut env_map = HashMap::new();

    if output_str.contains('\0') {
        for line in output_str.split('\0') {
            insert_env_line(&mut env_map, line);
        }
    } else {
        for line in output_str.lines() {
            insert_env_line(&mut env_map, line.trim_end_matches('\r'));
        }
    }

    if env_map.is_empty() {
        return Err(ShellEnvironmentError::ParseError(
            "No environment variables found".to_string(),
        ));
    }

    Ok(env_map)
}

fn insert_env_line(env_map: &mut HashMap<String, String>, line: &str) {
    if line.is_empty() {
        return;
    }

    if let Some(eq_pos) = line.find('=') {
        let key = line[..eq_pos].to_string();
        let value = line[eq_pos + 1..].to_string();
        env_map.insert(key, value);
    }
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
        Err(ShellEnvironmentError::EnvrcUnsupported(msg)) => Err(ShellEnvError::ParseError(msg)),
        Err(ShellEnvironmentError::NixPrintDevEnvFailed(msg)) => {
            Err(ShellEnvError::CommandFailed(msg))
        }
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
            Err(ShellEnvironmentError::EnvrcUnsupported(msg)) => {
                Err(ShellEnvError::ParseError(msg))
            }
            Err(ShellEnvironmentError::NixPrintDevEnvFailed(msg)) => {
                Err(ShellEnvError::CommandFailed(msg))
            }
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

    #[test]
    fn test_parse_native_flake_envrc_subset() {
        let plan = parse_native_flake_envrc(
            r#"
                # Load the default dev shell
                use flake ".#dev shell" --impure # trailing comment
                watch_file rust-toolchain
                watch_file "config/local file"
            "#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(plan.flake_args, vec![".#dev shell", "--impure"]);
        assert_eq!(
            plan.watched_files,
            vec![
                PathBuf::from("rust-toolchain"),
                PathBuf::from("config/local file")
            ]
        );
    }

    #[test]
    fn test_parse_native_flake_envrc_accepts_use_flake_function() {
        let plan = parse_native_flake_envrc("use_flake . --no-write-lock-file")
            .unwrap()
            .unwrap();

        assert_eq!(plan.flake_args, vec![".", "--no-write-lock-file"]);
    }

    #[test]
    fn test_parse_native_flake_envrc_defaults_bare_use_flake_to_current_dir() {
        let plan = parse_native_flake_envrc("use flake").unwrap().unwrap();

        assert_eq!(plan.flake_args, vec!["."]);
    }

    #[test]
    fn test_parse_native_flake_envrc_rejects_arbitrary_shell() {
        let error = parse_native_flake_envrc("export CARGO_HOME=$PWD/.cargo")
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsupported command `export`"));
    }

    #[test]
    fn test_parse_nix_print_dev_env_json_exports_only_string_vars() {
        let output = br#"{
            "bashFunctions": {},
            "variables": {
                "PATH": {"type": "exported", "value": "/nix/bin"},
                "HELIX_RUNTIME": {"type": "exported", "value": "/nix/helix-runtime"},
                "notExported": {"type": "var", "value": "ignored"},
                "arrayVar": {"type": "array", "value": ["ignored"]},
                "BAD=KEY": {"type": "exported", "value": "ignored"}
            }
        }"#;

        let env = parse_nix_print_dev_env_json(output).unwrap();

        assert_eq!(env.get("PATH"), Some(&"/nix/bin".to_string()));
        assert_eq!(
            env.get("HELIX_RUNTIME"),
            Some(&"/nix/helix-runtime".to_string())
        );
        assert!(!env.contains_key("notExported"));
        assert!(!env.contains_key("arrayVar"));
        assert!(!env.contains_key("BAD=KEY"));
    }

    #[test]
    fn test_native_flake_environment_restores_nix_direnv_session_vars() {
        let baseline = HashMap::from([
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("TMPDIR".to_string(), "/var/folders/user/tmp".to_string()),
            ("terminfo".to_string(), "/usr/share/terminfo".to_string()),
        ]);
        let exported = HashMap::from([
            ("PATH".to_string(), "/nix/store/bin:/usr/bin".to_string()),
            ("TMPDIR".to_string(), "/tmp/nix-shell.abc123".to_string()),
            ("TEMP".to_string(), "/tmp/nix-shell.abc123".to_string()),
            ("terminfo".to_string(), "/nix/store/terminfo".to_string()),
            (
                "NIX_BUILD_TOP".to_string(),
                "/tmp/nix-shell.abc123".to_string(),
            ),
        ]);

        let env = merge_native_flake_environment(&baseline, exported);

        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some("/nix/store/bin:/usr/bin")
        );
        assert_eq!(
            env.get("TMPDIR").map(String::as_str),
            Some("/var/folders/user/tmp")
        );
        assert_eq!(
            env.get("terminfo").map(String::as_str),
            Some("/usr/share/terminfo")
        );
        assert!(!env.contains_key("TEMP"));
        assert!(!env.contains_key("NIX_BUILD_TOP"));
    }

    #[test]
    fn test_native_flake_environment_preserves_baseline_path_suffixes() {
        let baseline = HashMap::from([(
            "PATH".to_string(),
            "/Users/test/.cargo/bin:/usr/bin:/bin".to_string(),
        )]);
        let exported = HashMap::from([(
            "PATH".to_string(),
            "/nix/store/tool/bin:/project/.direnv/bin".to_string(),
        )]);

        let env = merge_native_flake_environment(&baseline, exported);

        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some("/nix/store/tool/bin:/project/.direnv/bin:/Users/test/.cargo/bin:/usr/bin:/bin")
        );
    }

    #[test]
    fn test_path_merge_preserves_project_order_and_deduplicates_baseline() {
        let mut env = HashMap::from([(
            "PATH".to_string(),
            "/project/bin:/usr/bin:/nix/bin".to_string(),
        )]);

        merge_path_like_var(&mut env, "PATH", Some("/usr/local/bin:/usr/bin:/bin"));

        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some("/project/bin:/usr/bin:/nix/bin:/usr/local/bin:/bin")
        );
    }

    #[test]
    fn test_native_flake_environment_merges_xdg_data_dirs() {
        let baseline = HashMap::from([(
            "XDG_DATA_DIRS".to_string(),
            "/usr/local/share:/usr/share".to_string(),
        )]);
        let exported = HashMap::from([(
            "XDG_DATA_DIRS".to_string(),
            "/nix/share:/usr/share/:/project/share".to_string(),
        )]);

        let env = merge_native_flake_environment(&baseline, exported);

        assert_eq!(
            env.get("XDG_DATA_DIRS").map(String::as_str),
            Some("/nix/share:/usr/share:/project/share:/usr/local/share")
        );
    }

    #[test]
    fn test_native_watch_paths_include_defaults_and_deduplicate() {
        let directory = Path::new("/project");
        let envrc_path = directory.join(".envrc");
        let plan = NativeFlakePlan {
            flake_args: Vec::new(),
            watched_files: vec![
                PathBuf::from("flake.lock"),
                PathBuf::from("rust-toolchain.toml"),
            ],
        };

        let paths = native_flake_watch_paths(directory, &envrc_path, &plan);

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/project/.envrc"),
                PathBuf::from("/project/flake.nix"),
                PathBuf::from("/project/flake.lock"),
                PathBuf::from("/project/rust-toolchain.toml"),
            ]
        );
    }

    #[test]
    fn test_native_watch_state_detects_created_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let watched_path = temp_dir.path().join("flake.lock");
        let watch_state = snapshot_watched_files(vec![watched_path.clone()]);

        assert!(watched_files_are_current(&watch_state));

        std::fs::write(watched_path, "new lock").unwrap();

        assert!(!watched_files_are_current(&watch_state));
    }

    #[test]
    fn test_native_watch_state_detects_modified_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let watched_path = temp_dir.path().join(".envrc");
        std::fs::write(&watched_path, "use flake\n").unwrap();
        let watch_state = snapshot_watched_files(vec![watched_path.clone()]);

        assert!(watched_files_are_current(&watch_state));

        std::fs::write(watched_path, "use flake --impure\n").unwrap();

        assert!(!watched_files_are_current(&watch_state));
    }

    #[test]
    fn test_native_flake_cached_environment_stales_when_watched_file_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let envrc_path = temp_dir.path().join(".envrc");
        std::fs::write(&envrc_path, "use flake\n").unwrap();
        let watch_state = snapshot_watched_files(vec![envrc_path.clone()]);
        let cached = CachedEnvironment {
            environment: HashMap::new(),
            origin: EnvironmentOrigin::NativeFlake,
            directory: temp_dir.path().to_path_buf(),
            native_watch_state: Some(watch_state),
        };

        assert!(cached_environment_is_current(&cached));

        std::fs::write(envrc_path, "use flake .#dev\n").unwrap();

        assert!(!cached_environment_is_current(&cached));
    }

    #[tokio::test]
    async fn test_cli_environment_is_baseline_when_envrc_is_unsupported() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join(".envrc"), "export FOO=bar\n").unwrap();
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/cli/bin".to_string()),
            ("FROM_CLI".to_string(), "yes".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));
        let env = project_env
            .get_environment_for_directory(temp_dir.path())
            .await
            .unwrap();

        assert_eq!(env.get("PATH"), Some(&"/cli/bin".to_string()));
        assert_eq!(env.get("FROM_CLI"), Some(&"yes".to_string()));
        assert!(!env.contains_key("FOO"));
    }

    #[tokio::test]
    async fn test_get_cached_origin_returns_current_origin() {
        let temp_dir = tempfile::tempdir().unwrap();
        let envrc_path = temp_dir.path().join(".envrc");
        std::fs::write(&envrc_path, "use flake\n").unwrap();

        let project_env = ProjectEnvironment::new(None);
        let canonical_dir = temp_dir.path().canonicalize().unwrap();
        let cached = CachedEnvironment {
            environment: HashMap::new(),
            origin: EnvironmentOrigin::NativeFlake,
            directory: canonical_dir.clone(),
            native_watch_state: Some(snapshot_watched_files(vec![envrc_path])),
        };

        project_env
            .directory_environments
            .write()
            .await
            .insert(canonical_dir, cached);

        assert_eq!(
            project_env.get_cached_origin(temp_dir.path()).await,
            Some(EnvironmentOrigin::NativeFlake)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_nix_print_dev_env_builds_expected_command() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let fake_nix = temp_dir.path().join("nix");
        let calls_file = temp_dir.path().join("calls.txt");
        std::fs::write(
            &fake_nix,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> '{}'
case "$*" in
  *"print-dev-env"*)
    printf '%s\n' '{{"variables":{{"PATH":{{"type":"exported","value":"/fake/bin"}}}}}}'
    ;;
  *"profile wipe-history"*)
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
"#,
                calls_file.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_nix).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_nix, permissions).unwrap();

        let project_dir = tempfile::tempdir().unwrap();
        let plan = NativeFlakePlan {
            flake_args: vec![".".to_string(), "--impure".to_string()],
            watched_files: Vec::new(),
        };

        let env = run_nix_print_dev_env_with_binary(project_dir.path(), &plan, &fake_nix)
            .await
            .unwrap();

        assert_eq!(env.get("PATH"), Some(&"/fake/bin".to_string()));
        let calls = std::fs::read_to_string(calls_file).unwrap();
        assert!(calls.contains("print-dev-env"));
        assert!(calls.contains("--json"));
        assert!(calls.contains("--profile"));
        assert!(calls.contains("--impure"));
        assert!(calls.contains("profile"));
        assert!(calls.contains("wipe-history"));
    }

    #[test]
    fn test_quote_path_for_shell_simple() {
        let path = Path::new("/tmp/project");
        let quoted = shell_command_builder::quote_path_for_shell(path);
        assert_eq!(quoted, "'/tmp/project'");
    }

    #[test]
    fn test_quote_path_for_shell_handles_single_quotes() {
        let path = Path::new("/tmp/it's complicated");
        let quoted = shell_command_builder::quote_path_for_shell(path);
        assert_eq!(quoted, r#"'/tmp/it'"'"'s complicated'"#);
    }

    #[test]
    fn test_build_environment_capture_command_uses_quoted_path() {
        let cmd = shell_command_builder::build_environment_capture_command(
            "/bin/zsh",
            Path::new("/tmp/it's complicated"),
        )
        .expect("command should build");

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-l".to_string()));
        assert!(args.contains(&"-c".to_string()));
        let command_string = args
            .iter()
            .find(|s| s.contains("cd "))
            .expect("missing command string");
        assert!(command_string.contains(r#"cd '/tmp/it'"'"'s complicated' && env -0"#));
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
