// ABOUTME: Comprehensive environment system following Zed's architecture for directory-specific shell environments
// ABOUTME: Handles CLI environment inheritance, directory shell capture, and LSP environment injection

use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::SystemTime;
use tokio::io::AsyncReadExt;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{Duration, Instant, timeout};

const DIRECTORY_SHELL_CAPTURE_TIMEOUT_SECONDS: u64 = 10;
const PROCESS_SHELL_CAPTURE_TIMEOUT_SECONDS: u64 = 10;

/// Error types for shell environment operations
#[derive(Debug, thiserror::Error)]
pub enum ShellEnvironmentError {
    #[error("Shell execution failed: {0}")]
    ShellExecutionFailed(String),

    #[error("Shell command timed out after {0} seconds")]
    Timeout(u64),

    #[error("Shell command cancelled")]
    Cancelled,

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

    /// Cached login-shell process environment for GUI launches without CLI inheritance.
    process_shell_environment: Arc<RwLock<Option<HashMap<String, String>>>>,

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
            process_shell_environment: Arc::new(RwLock::new(None)),
            shell_execution_semaphore: Arc::new(Semaphore::new(3)), // Limit concurrent shell executions
        }
    }

    /// Get environment for directory following priority: CLI > directory shell > process
    #[instrument(skip(self), fields(directory = %directory.display()))]
    pub async fn get_environment_for_directory(
        &self,
        directory: &Path,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        self.get_environment_for_directory_with_cancellation(directory, None)
            .await
    }

    /// Get environment for directory, cancelling shell child processes when requested.
    #[instrument(skip(self, cancellation), fields(directory = %directory.display()))]
    pub async fn get_environment_for_directory_with_cancellation(
        &self,
        directory: &Path,
        cancellation: Option<&AtomicBool>,
    ) -> Result<HashMap<String, String>, ShellEnvironmentError> {
        if shell_environment_cancelled(cancellation) {
            return Err(ShellEnvironmentError::Cancelled);
        }

        let baseline_env = self
            .baseline_environment_with_cancellation(cancellation)
            .await;

        // Priority 1: Directory-specific environment (cached)
        if let Some(cached) = {
            let cache = self.directory_environments.read().await;
            cache.get(directory).cloned()
        } {
            if cached_environment_is_current(&cached) {
                debug!("Using cached directory environment");
                return Ok(cached.environment);
            }

            debug!(
                directory = %directory.display(),
                origin = ?cached.origin,
                "Cached directory environment is stale"
            );
            let mut cache = self.directory_environments.write().await;
            cache.remove(directory);
        }

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
            .load_native_flake_environment(&canonical_dir, &baseline_env, cancellation)
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
                    cache.insert(canonical_dir.clone(), cached_env.clone());
                    if directory != canonical_dir {
                        cache.insert(directory.to_path_buf(), cached_env);
                    }
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

                let message = error.to_string();
                let mut errors = self.environment_errors.write().await;
                errors.insert(canonical_dir.clone(), message.clone());
                if directory != canonical_dir {
                    errors.insert(directory.to_path_buf(), message);
                }
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
        let env = self
            .load_directory_environment(&canonical_dir, baseline_env, cancellation)
            .await?;

        if directory != canonical_dir {
            let cached_env = CachedEnvironment {
                environment: env.clone(),
                origin: EnvironmentOrigin::DirectoryShell,
                directory: directory.to_path_buf(),
                native_watch_state: None,
            };

            {
                let mut cache = self.directory_environments.write().await;
                cache.insert(directory.to_path_buf(), cached_env);
            }

            {
                let mut errors = self.environment_errors.write().await;
                errors.remove(directory);
            }
        }

        Ok(env)
    }

    async fn baseline_environment(&self) -> HashMap<String, String> {
        self.baseline_environment_with_cancellation(None).await
    }

    async fn baseline_environment_with_cancellation(
        &self,
        cancellation: Option<&AtomicBool>,
    ) -> HashMap<String, String> {
        let mut combined_env: HashMap<String, String> = std::env::vars().collect();

        if let Some(cli_env) = &self.cli_environment {
            for (key, value) in cli_env {
                combined_env.insert(key.clone(), value.clone());
            }

            if login_shell_process_environment_supported()
                && environment_home_requires_repair(&combined_env)
            {
                warn!(
                    home = %combined_env.get("HOME").map(String::as_str).unwrap_or("<unset>"),
                    "CLI environment has unusable HOME; repairing with login shell baseline"
                );
                let mut repaired_env = self
                    .login_shell_process_environment_with_cancellation(combined_env, cancellation)
                    .await;
                repaired_env.insert("ZED_ENVIRONMENT".to_string(), "cli".to_string());
                return repaired_env;
            }

            return combined_env;
        }

        if login_shell_process_environment_supported() {
            return self
                .login_shell_process_environment_with_cancellation(combined_env, cancellation)
                .await;
        }

        combined_env
    }

    async fn login_shell_process_environment_with_cancellation(
        &self,
        process_env: HashMap<String, String>,
        cancellation: Option<&AtomicBool>,
    ) -> HashMap<String, String> {
        if let Some(cached) = self.process_shell_environment.read().await.clone() {
            return cached;
        }

        let _permit = match self.shell_execution_semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => return process_env,
        };

        if let Some(cached) = self.process_shell_environment.read().await.clone() {
            return cached;
        }

        let Some(home_dir) = login_shell_home_dir(&process_env) else {
            warn!("Could not determine user home directory for login shell environment capture");
            *self.process_shell_environment.write().await = Some(process_env.clone());
            return process_env;
        };

        let shell = default_environment_shell();
        debug!(
            shell = %shell,
            home_dir = %home_dir.display(),
            "Capturing login shell environment for process baseline"
        );

        let mut baseline = process_env;
        if environment_home_requires_repair(&baseline) {
            baseline.insert("HOME".to_string(), home_dir.to_string_lossy().into_owned());
        }

        match capture_shell_environment_with_timeout(
            &shell,
            &home_dir,
            PROCESS_SHELL_CAPTURE_TIMEOUT_SECONDS,
            Some(&baseline),
            cancellation,
        )
        .await
        {
            Ok(shell_env) => {
                let process_path = baseline.get("PATH").cloned();
                for (key, value) in shell_env {
                    baseline.insert(key, value);
                }
                merge_path_like_var(&mut baseline, "PATH", process_path.as_deref());
                apply_environment_to_process(&baseline);
                info!(
                    env_count = baseline.len(),
                    "Captured and applied login shell environment for process baseline"
                );
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "Failed to capture login shell environment; using process environment baseline"
                );
            }
        }

        *self.process_shell_environment.write().await = Some(baseline.clone());
        baseline
    }

    /// Capture and apply the login shell baseline early for GUI launches.
    ///
    /// This is useful when the application was launched by the OS instead of the CLI:
    /// the process can start with a minimal launcher environment, while child tools
    /// expect the user's configured shell environment.
    pub async fn bootstrap_process_environment(&self) -> HashMap<String, String> {
        self.baseline_environment().await
    }

    /// Return the inherited CLI environment, if this instance was opened from a CLI.
    pub fn cli_environment(&self) -> Option<HashMap<String, String>> {
        self.cli_environment.clone()
    }

    /// Load shell environment for specific directory with proper cd command
    async fn load_directory_environment(
        &self,
        directory: &PathBuf,
        baseline_env: HashMap<String, String>,
        cancellation: Option<&AtomicBool>,
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

        match capture_shell_environment_with_timeout(
            &shell,
            directory,
            DIRECTORY_SHELL_CAPTURE_TIMEOUT_SECONDS,
            Some(&baseline_env),
            cancellation,
        )
        .await
        {
            Ok(directory_env) => {
                let baseline_path = baseline_env.get("PATH").cloned();
                let mut combined_env = baseline_env;

                // Directory shell environment takes precedence over process environment
                for (key, value) in directory_env {
                    combined_env.insert(key, value);
                }
                merge_path_like_var(&mut combined_env, "PATH", baseline_path.as_deref());

                // Add origin marker for directory shell environment
                combined_env.insert("ZED_ENVIRONMENT".to_string(), "worktree-shell".to_string());

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
            }
            Err(error) => {
                match &error {
                    ShellEnvironmentError::Timeout(_) => {
                        warn!(directory = %directory.display(), "Shell environment capture timed out");
                    }
                    ShellEnvironmentError::IoError(_) => {
                        error!(error = %error, "IO error during shell execution");
                    }
                    _ => {}
                }

                {
                    let mut errors = self.environment_errors.write().await;
                    errors.insert(directory.clone(), error.to_string());
                }

                // A shell is an enhancement to the process environment, not a
                // prerequisite for opening a project. This is particularly
                // important for GUI launches and constrained environments where
                // `$SHELL` can point at an unavailable executable. Keep the
                // diagnostic, but let child processes use the inherited baseline.
                warn!(
                    directory = %directory.display(),
                    error = %error,
                    "Falling back to the process environment after shell capture failed"
                );

                let mut fallback_env = baseline_env;
                fallback_env.insert("ZED_ENVIRONMENT".to_string(), "worktree-shell".to_string());

                let cached_env = CachedEnvironment {
                    environment: fallback_env.clone(),
                    origin: EnvironmentOrigin::Process,
                    directory: directory.clone(),
                    native_watch_state: None,
                };
                self.directory_environments
                    .write()
                    .await
                    .insert(directory.clone(), cached_env);

                Ok(fallback_env)
            }
        }
    }

    async fn load_native_flake_environment(
        &self,
        directory: &Path,
        baseline_env: &HashMap<String, String>,
        cancellation: Option<&AtomicBool>,
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

        // Remote helpers, notably WSL helpers started with `wsl.exe --exec`, do not
        // run shell startup files. Single-user Nix installs commonly add `nix` to
        // PATH from those files, so consult the login shell only when the inherited
        // environment cannot resolve it.
        let native_baseline = if resolve_program_from_env_path("nix", baseline_env).is_none()
            && login_shell_process_environment_supported()
        {
            self.login_shell_process_environment_with_cancellation(
                baseline_env.clone(),
                cancellation,
            )
            .await
        } else {
            baseline_env.clone()
        };

        let exported =
            run_nix_print_dev_env(directory, &plan, &native_baseline, cancellation).await?;
        let mut environment = merge_native_flake_environment(&native_baseline, exported);
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
        if let Some(origin) = {
            let cache = self.directory_environments.read().await;
            cache
                .get(directory)
                .filter(|cached| cached_environment_is_current(cached))
                .map(|cached| cached.origin.clone())
        } {
            return Some(origin);
        }

        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        let cache = self.directory_environments.read().await;
        cache
            .get(&canonical_dir)
            .filter(|cached| cached_environment_is_current(cached))
            .map(|cached| cached.origin.clone())
    }

    /// Get any cached diagnostics for the last environment load attempt.
    pub async fn get_environment_diagnostics(&self, directory: &Path) -> Vec<String> {
        if let Some(error) = {
            let errors = self.environment_errors.read().await;
            errors.get(directory).cloned()
        } {
            return vec![error];
        }

        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        let errors = self.environment_errors.read().await;
        errors.get(&canonical_dir).cloned().into_iter().collect()
    }

    /// Invalidate cache for specific directory
    pub async fn invalidate_directory_cache(&self, directory: &Path) {
        let canonical_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());

        debug!(directory = %canonical_dir.display(), "Invalidating directory environment cache");

        {
            let mut cache = self.directory_environments.write().await;
            cache.remove(directory);
            cache.remove(&canonical_dir);
        }

        {
            let mut errors = self.environment_errors.write().await;
            errors.remove(directory);
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

const NIX_DIRENV_RESTORED_VARS: &[&str] = &["TMP", "TMPDIR", "TEMP", "TEMPDIR", "terminfo"];

const CALLER_OWNED_ENV_VARS: &[&str] = &[
    "HOME",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
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
    restore_caller_owned_vars(&mut environment, baseline);

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

fn restore_caller_owned_vars(
    environment: &mut HashMap<String, String>,
    baseline: &HashMap<String, String>,
) {
    for key in CALLER_OWNED_ENV_VARS {
        restore_caller_owned_var(environment, baseline, key);
    }
}

fn restore_caller_owned_var(
    environment: &mut HashMap<String, String>,
    baseline: &HashMap<String, String>,
    key: &str,
) {
    match baseline.get(key).filter(|value| {
        !value.trim().is_empty() && !home_path_is_placeholder(Path::new(value.as_str()))
    }) {
        Some(value) => {
            environment.insert(key.to_string(), value.clone());
        }
        None => {
            environment.remove(key);
        }
    }
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
    baseline_env: &HashMap<String, String>,
    cancellation: Option<&AtomicBool>,
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let nix_binary = resolve_program_from_env_path("nix", baseline_env).ok_or_else(|| {
        ShellEnvironmentError::NixPrintDevEnvFailed(
            "nix executable was not found in the inherited or login-shell PATH".to_string(),
        )
    })?;
    run_nix_print_dev_env_with_binary(directory, plan, baseline_env, &nix_binary, cancellation)
        .await
}

fn resolve_program_from_env_path(
    program: &str,
    environment: &HashMap<String, String>,
) -> Option<PathBuf> {
    let path = environment.get("PATH")?;
    env::split_paths(path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

async fn run_nix_print_dev_env_with_binary(
    directory: &Path,
    plan: &NativeFlakePlan,
    baseline_env: &HashMap<String, String>,
    nix_binary: &Path,
    cancellation: Option<&AtomicBool>,
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let profile = native_flake_profile_path(directory, baseline_env);
    if let Some(parent) = profile.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut command = nucleotide_process::tokio_command(nix_binary);
    command
        .envs(baseline_env)
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

    let output = cancellable_command_output(command, 30, cancellation).await?;

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
    let mut command = nucleotide_process::tokio_command(nix_binary);
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

fn native_flake_profile_path(directory: &Path, environment: &HashMap<String, String>) -> PathBuf {
    let key = stable_hash_hex(directory.to_string_lossy().as_bytes());
    nucleotide_cache_dir(environment)
        .join("native-flake-env")
        .join(key)
        .join("flake-profile")
}

fn nucleotide_cache_dir(environment: &HashMap<String, String>) -> PathBuf {
    if let Some(path) = non_empty_env_path(environment, "NUCLEOTIDE_CACHE_DIR") {
        return path;
    }

    if let Some(path) = non_empty_env_path(environment, "XDG_CACHE_HOME") {
        return path.join("nucleotide");
    }

    if let Some(home) = non_empty_env_path(environment, "HOME") {
        return home.join(".cache").join("nucleotide");
    }

    env::temp_dir().join("nucleotide")
}

fn non_empty_env_path(environment: &HashMap<String, String>, key: &str) -> Option<PathBuf> {
    environment
        .get(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

async fn capture_shell_environment_with_timeout(
    shell: &str,
    directory: &Path,
    timeout_seconds: u64,
    baseline_env: Option<&HashMap<String, String>>,
    cancellation: Option<&AtomicBool>,
) -> Result<HashMap<String, String>, ShellEnvironmentError> {
    let command = shell_command_builder::build_environment_capture_command(shell, directory)?;

    let mut tokio_command = nucleotide_process::tokio_command(command.get_program());
    if let Some(baseline_env) = baseline_env {
        tokio_command.envs(baseline_env);
    }
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

    let output = cancellable_command_output(tokio_command, timeout_seconds, cancellation).await;

    match output {
        Ok(output) if output.status.success() => parse_shell_environment(&output.stdout),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(ShellEnvironmentError::ShellExecutionFailed(stderr))
        }
        Err(error) => Err(error),
    }
}

async fn cancellable_command_output(
    mut command: tokio::process::Command,
    timeout_seconds: u64,
    cancellation: Option<&AtomicBool>,
) -> Result<Output, ShellEnvironmentError> {
    if shell_environment_cancelled(cancellation) {
        return Err(ShellEnvironmentError::Cancelled);
    }

    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    configure_environment_child_process(&mut command);

    let mut child = command.spawn().map_err(ShellEnvironmentError::IoError)?;
    let process_id = child.id();
    let mut stdout = child.stdout.take().ok_or_else(|| {
        ShellEnvironmentError::ShellExecutionFailed(
            "child process stdout was not piped".to_string(),
        )
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        ShellEnvironmentError::ShellExecutionFailed(
            "child process stderr was not piped".to_string(),
        )
    })?;

    let mut stdout_task = Some(tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    }));
    let mut stderr_task = Some(tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    }));

    let started = Instant::now();
    let timeout_duration = Duration::from_secs(timeout_seconds);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(ShellEnvironmentError::IoError)? {
            break status;
        }

        if shell_environment_cancelled(cancellation) {
            terminate_environment_child_process(&mut child, process_id).await?;
            let _ = join_environment_pipe(stdout_task.take().unwrap()).await;
            let _ = join_environment_pipe(stderr_task.take().unwrap()).await;
            return Err(ShellEnvironmentError::Cancelled);
        }

        if started.elapsed() >= timeout_duration {
            terminate_environment_child_process(&mut child, process_id).await?;
            let _ = join_environment_pipe(stdout_task.take().unwrap()).await;
            let _ = join_environment_pipe(stderr_task.take().unwrap()).await;
            return Err(ShellEnvironmentError::Timeout(timeout_seconds));
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    let stdout = join_environment_pipe(stdout_task.take().unwrap()).await?;
    let stderr = join_environment_pipe(stderr_task.take().unwrap()).await?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn join_environment_pipe(
    task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, ShellEnvironmentError> {
    task.await
        .map_err(|error| ShellEnvironmentError::ShellExecutionFailed(error.to_string()))?
        .map_err(ShellEnvironmentError::IoError)
}

#[cfg(unix)]
fn configure_environment_child_process(command: &mut tokio::process::Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_environment_child_process(_command: &mut tokio::process::Command) {}

async fn terminate_environment_child_process(
    child: &mut tokio::process::Child,
    process_id: Option<u32>,
) -> Result<(), ShellEnvironmentError> {
    #[cfg(not(unix))]
    let _ = process_id;

    #[cfg(unix)]
    if let Some(process_id) = process_id
        && kill_environment_process_group(process_id).is_ok()
    {
        let _ = child.wait().await;
        return Ok(());
    }

    child.kill().await.map_err(ShellEnvironmentError::IoError)
}

#[cfg(unix)]
fn kill_environment_process_group(process_id: u32) -> std::io::Result<()> {
    let status = Command::new("kill")
        .arg("-KILL")
        // A negative PID targets a process group, so terminate option parsing first.
        .arg("--")
        .arg(format!("-{process_id}"))
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "kill process group exited with {status}"
        )))
    }
}

fn shell_environment_cancelled(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|cancellation| cancellation.load(Ordering::Relaxed))
}

fn apply_environment_to_process(environment: &HashMap<String, String>) {
    for (key, value) in environment {
        if key.contains('=') || key.is_empty() {
            continue;
        }

        // SAFETY: This is called during environment initialization/capture and mirrors
        // the app-wide process environment used by child tools launched afterwards.
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

fn login_shell_process_environment_supported() -> bool {
    cfg!(unix) && !cfg!(test)
}

fn environment_home_requires_repair(environment: &HashMap<String, String>) -> bool {
    environment
        .get("HOME")
        .is_none_or(|home| home.trim().is_empty() || home_path_is_placeholder(Path::new(home)))
}

fn login_shell_home_dir(process_env: &HashMap<String, String>) -> Option<PathBuf> {
    if let Some(home) = process_env
        .get("HOME")
        .and_then(|home| usable_home_dir(Path::new(home)))
    {
        return Some(home);
    }

    if let Some(home) = dirs::home_dir().and_then(|home| usable_home_dir(&home)) {
        return Some(home);
    }

    process_env
        .get("USER")
        .or_else(|| process_env.get("LOGNAME"))
        .and_then(|user| fallback_home_for_user(user))
}

fn usable_home_dir(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() || home_path_is_placeholder(path) || !path.is_dir() {
        return None;
    }

    Some(path.to_path_buf())
}

fn home_path_is_placeholder(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value == "/homeless-shelter" || value.contains("/homeless-shelter/")
}

fn fallback_home_for_user(user: &str) -> Option<PathBuf> {
    if user.trim().is_empty() || user.contains('/') || user.contains('\\') {
        return None;
    }

    #[cfg(target_os = "macos")]
    let candidates = [PathBuf::from("/Users").join(user)];

    #[cfg(all(unix, not(target_os = "macos")))]
    let candidates = [PathBuf::from("/home").join(user)];

    #[cfg(not(unix))]
    let candidates: [PathBuf; 0] = [];

    candidates.into_iter().find(|path| path.is_dir())
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
        let mut command = nucleotide_process::command(shell);

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
                command.arg("-l").arg("-i").arg("-c").arg(command_string);
            }
            "tcsh" | "csh" => {
                // csh/tcsh require setting argv[0] to "-" for login shell mode, which
                // std::process::Command cannot do portably here. Keep the command
                // non-login instead of passing unsupported flags.
                command.arg("-c").arg(command_string);
            }
            "nu" => {
                // Nushell does not allow non-interactive login shells. Use eval mode.
                let nu_command = format!("cd {}; ^env -0; exit", escaped_dir);
                command.arg("-l").arg("-e").arg(nu_command);
            }
            _ => {
                // Default POSIX-style shells use login + interactive mode so startup
                // files and prompt-hook integrations can populate the environment.
                command.arg("-l").arg("-i").arg("-c").arg(command_string);
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
        let mut command = nucleotide_process::command(shell);
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
    let file_name = shell_path.rsplit(['/', '\\']).next().unwrap_or("unknown");
    let shell_name = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn executable_tempdir() -> tempfile::TempDir {
        let base = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test-tmp");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new()
            .prefix("nucleotide-env-")
            .tempdir_in(base)
            .unwrap()
    }

    #[cfg(unix)]
    fn generated_executable_is_allowed(executable: &Path) -> bool {
        match std::process::Command::new(executable)
            .arg("--probe")
            .status()
        {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
            Err(error) => panic!(
                "failed to probe generated executable {}: {error}",
                executable.display()
            ),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellable_command_output_kills_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("env-started");
        let mut command = nucleotide_process::tokio_command("/bin/sh");
        command
            .args(["-c", "printf started > \"$STARTED_FILE\"; sleep 3"])
            .env("STARTED_FILE", &started_file);
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = tokio::spawn(async move {
            cancellable_command_output(command, 10, Some(worker_cancellation.as_ref())).await
        });

        let started = Instant::now();
        while !started_file.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for fake environment process to start"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let cancelled_at = Instant::now();
        cancellation.store(true, Ordering::Relaxed);
        let error = worker.await.unwrap().unwrap_err();

        assert!(matches!(error, ShellEnvironmentError::Cancelled));
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(2),
            "environment capture waited for the child sleep instead of killing its process group"
        );
    }

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
    fn test_native_flake_environment_restores_temp_vars_but_keeps_nix_build_top() {
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
        assert_eq!(
            env.get("NIX_BUILD_TOP").map(String::as_str),
            Some("/tmp/nix-shell.abc123")
        );
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
    fn test_native_flake_environment_restores_caller_owned_home_and_xdg_vars() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().to_string_lossy().to_string();
        let cache_path = home.path().join(".cache").to_string_lossy().to_string();
        let config_path = home.path().join(".config").to_string_lossy().to_string();
        let data_path = home
            .path()
            .join(".local/share")
            .to_string_lossy()
            .to_string();
        let state_path = home
            .path()
            .join(".local/state")
            .to_string_lossy()
            .to_string();
        let baseline = HashMap::from([
            ("HOME".to_string(), home_path.clone()),
            ("XDG_CACHE_HOME".to_string(), cache_path.clone()),
            ("XDG_CONFIG_HOME".to_string(), config_path.clone()),
            ("XDG_DATA_HOME".to_string(), data_path.clone()),
            ("XDG_STATE_HOME".to_string(), state_path.clone()),
        ]);
        let exported = HashMap::from([
            ("HOME".to_string(), "/homeless-shelter".to_string()),
            (
                "CARGO_HOME".to_string(),
                "/homeless-shelter/.cargo".to_string(),
            ),
            (
                "PIP_CACHE_DIR".to_string(),
                "/homeless-shelter/.cache/pip".to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_string(),
                "/homeless-shelter/.cache".to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                "/homeless-shelter/.config".to_string(),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                "/homeless-shelter/.local/share".to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                "/homeless-shelter/.local/state".to_string(),
            ),
        ]);

        let env = merge_native_flake_environment(&baseline, exported);

        assert_eq!(env.get("HOME"), Some(&home_path));
        assert_eq!(
            env.get("XDG_CACHE_HOME").map(String::as_str),
            Some(cache_path.as_str())
        );
        assert_eq!(
            env.get("XDG_CONFIG_HOME").map(String::as_str),
            Some(config_path.as_str())
        );
        assert_eq!(
            env.get("XDG_DATA_HOME").map(String::as_str),
            Some(data_path.as_str())
        );
        assert_eq!(
            env.get("XDG_STATE_HOME").map(String::as_str),
            Some(state_path.as_str())
        );
        assert_eq!(
            env.get("CARGO_HOME").map(String::as_str),
            Some("/homeless-shelter/.cargo")
        );
        assert_eq!(
            env.get("PIP_CACHE_DIR").map(String::as_str),
            Some("/homeless-shelter/.cache/pip")
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
    async fn test_native_flake_fallback_records_environment_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join(".envrc"), "export FOO=bar\n").unwrap();
        let project_env = ProjectEnvironment::new(Some(HashMap::from([(
            "PATH".to_string(),
            "/cli/bin".to_string(),
        )])));

        let env = project_env
            .get_environment_for_directory(temp_dir.path())
            .await
            .unwrap();
        let diagnostics = project_env
            .get_environment_diagnostics(temp_dir.path())
            .await;

        assert_eq!(env.get("PATH"), Some(&"/cli/bin".to_string()));
        assert!(
            diagnostics
                .first()
                .is_some_and(|message| message.contains("Unsupported .envrc"))
        );
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

    #[tokio::test]
    async fn test_get_environment_uses_raw_directory_cache_before_canonicalizing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let raw_dir = temp_dir.path().join(".");
        let project_env = ProjectEnvironment::new(Some(HashMap::from([(
            "PATH".to_string(),
            "/cli/bin".to_string(),
        )])));
        let cached = CachedEnvironment {
            environment: HashMap::from([("FROM_CACHE".to_string(), "yes".to_string())]),
            origin: EnvironmentOrigin::Process,
            directory: raw_dir.clone(),
            native_watch_state: None,
        };

        project_env
            .directory_environments
            .write()
            .await
            .insert(raw_dir.clone(), cached);

        let env = project_env
            .get_environment_for_directory(&raw_dir)
            .await
            .unwrap();

        assert_eq!(env.get("FROM_CACHE"), Some(&"yes".to_string()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_nix_print_dev_env_builds_expected_command() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = executable_tempdir();
        let fake_nix = temp_dir.path().join("nix");
        let calls_file = temp_dir.path().join("calls.txt");
        std::fs::write(
            &fake_nix,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> '{}'
case "$*" in
  *"print-dev-env"*)
    printf '{{"variables":{{"PATH":{{"type":"exported","value":"/fake/bin"}},"HOME":{{"type":"exported","value":"%s"}},"CARGO_HOME":{{"type":"exported","value":"%s"}}}}}}\n' "$HOME" "$CARGO_HOME"
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
        if !generated_executable_is_allowed(&fake_nix) {
            return;
        }

        let project_dir = tempfile::tempdir().unwrap();
        let plan = NativeFlakePlan {
            flake_args: vec![".".to_string(), "--impure".to_string()],
            watched_files: Vec::new(),
        };

        let baseline = HashMap::from([
            ("HOME".to_string(), "/Users/test".to_string()),
            ("CARGO_HOME".to_string(), "/Users/test/.cargo".to_string()),
            (
                "NUCLEOTIDE_CACHE_DIR".to_string(),
                temp_dir.path().join("cache").display().to_string(),
            ),
        ]);

        let env = run_nix_print_dev_env_with_binary(
            project_dir.path(),
            &plan,
            &baseline,
            &fake_nix,
            None,
        )
        .await
        .unwrap();

        assert_eq!(env.get("PATH"), Some(&"/fake/bin".to_string()));
        assert_eq!(env.get("HOME"), Some(&"/Users/test".to_string()));
        assert_eq!(
            env.get("CARGO_HOME"),
            Some(&"/Users/test/.cargo".to_string())
        );
        let calls = std::fs::read_to_string(calls_file).unwrap();
        assert!(calls.contains("print-dev-env"));
        assert!(calls.contains("--json"));
        assert!(calls.contains("--profile"));
        assert!(calls.contains("--impure"));
        assert!(calls.contains("profile"));
        assert!(calls.contains("wipe-history"));
    }

    #[test]
    fn test_resolve_program_from_env_path_uses_baseline_path() {
        let empty_bin = tempfile::tempdir().unwrap();
        let fake_bin = tempfile::tempdir().unwrap();
        let fake_nix = fake_bin.path().join("nix");
        std::fs::write(&fake_nix, "").unwrap();
        let baseline = HashMap::from([(
            "PATH".to_string(),
            env::join_paths([empty_bin.path(), fake_bin.path()])
                .unwrap()
                .to_string_lossy()
                .to_string(),
        )]);

        assert_eq!(
            resolve_program_from_env_path("nix", &baseline),
            Some(fake_nix)
        );
    }

    #[tokio::test]
    async fn test_run_nix_print_dev_env_reports_missing_nix() {
        let empty_bin = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let baseline =
            HashMap::from([("PATH".to_string(), empty_bin.path().display().to_string())]);
        let plan = NativeFlakePlan {
            flake_args: vec![".".to_string()],
            watched_files: Vec::new(),
        };

        let error = run_nix_print_dev_env(project_dir.path(), &plan, &baseline, None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ShellEnvironmentError::NixPrintDevEnvFailed(message)
                if message.contains("nix executable was not found")
        ));
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
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"-c".to_string()));
        let command_string = args
            .iter()
            .find(|s| s.contains("cd "))
            .expect("missing command string");
        assert!(command_string.contains(r#"cd '/tmp/it'"'"'s complicated' && env -0"#));
    }

    #[test]
    fn test_build_environment_capture_command_uses_shell_specific_login_modes() {
        let fish = shell_command_builder::build_environment_capture_command(
            "/opt/homebrew/bin/fish",
            Path::new("/tmp/project"),
        )
        .expect("fish command should build");
        let fish_args: Vec<String> = fish
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(fish_args.contains(&"-l".to_string()));
        assert!(fish_args.contains(&"-i".to_string()));
        assert!(fish_args.contains(&"-c".to_string()));

        let nu = shell_command_builder::build_environment_capture_command(
            "/opt/homebrew/bin/nu",
            Path::new("/tmp/project"),
        )
        .expect("nu command should build");
        let nu_args: Vec<String> = nu
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(nu_args.contains(&"-l".to_string()));
        assert!(nu_args.contains(&"-e".to_string()));
        assert!(!nu_args.contains(&"-i".to_string()));

        let csh = shell_command_builder::build_environment_capture_command(
            "/bin/tcsh",
            Path::new("/tmp/project"),
        )
        .expect("tcsh command should build");
        let csh_args: Vec<String> = csh
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(!csh_args.contains(&"-l".to_string()));
        assert!(!csh_args.contains(&"-i".to_string()));
        assert!(csh_args.contains(&"-c".to_string()));
    }

    #[test]
    fn test_login_shell_home_dir_does_not_return_homeless_shelter() {
        let env = HashMap::from([
            ("HOME".to_string(), "/homeless-shelter".to_string()),
            ("USER".to_string(), "definitely-not-a-real-user".to_string()),
        ]);

        assert_ne!(
            login_shell_home_dir(&env).as_deref(),
            Some(Path::new("/homeless-shelter"))
        );
    }

    #[test]
    fn test_environment_home_requires_repair_for_missing_or_placeholder_home() {
        assert!(environment_home_requires_repair(&HashMap::new()));
        assert!(environment_home_requires_repair(&HashMap::from([(
            "HOME".to_string(),
            String::new(),
        )])));
        assert!(environment_home_requires_repair(&HashMap::from([(
            "HOME".to_string(),
            "/homeless-shelter".to_string(),
        )])));
        assert!(!environment_home_requires_repair(&HashMap::from([(
            "HOME".to_string(),
            "/Users/test".to_string(),
        )])));
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
