// ABOUTME: Shell environment management crate for directory-specific environments
// ABOUTME: Provides CLI inheritance, shell capture, and LSP environment injection

pub mod shell_env;
pub mod shell_env_focused_test;

// Re-export main types for easy access
pub use shell_env::{
    CachedEnvironment, EnvironmentOrigin, ProjectEnvironment, ShellEnvError, ShellEnvironmentCache,
    ShellEnvironmentError, detect_shell_type, parse_shell_environment, shell_command_builder,
};

#[cfg(test)]
#[allow(unused_imports)]
pub use shell_env_focused_test::*;
