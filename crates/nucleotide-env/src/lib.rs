// ABOUTME: Shell environment management crate for directory-specific environments
// ABOUTME: Provides CLI inheritance, shell capture, and LSP environment injection

pub mod shell_env;
pub mod shell_env_focused_test;
pub mod wsl;

// Re-export main types for easy access
pub use shell_env::{
    CachedEnvironment, EnvironmentOrigin, ProjectEnvironment, ShellEnvError, ShellEnvironmentCache,
    ShellEnvironmentError, detect_shell_type, parse_shell_environment, shell_command_builder,
};
pub use wsl::{
    WslRemoteHelperError, WslWorkspace, build_wsl_environment_capture_command,
    build_wsl_remote_hello_command, build_wsl_shell_command, probe_wsl_remote_helper,
};

#[cfg(test)]
#[allow(unused_imports)]
pub use shell_env_focused_test::*;
