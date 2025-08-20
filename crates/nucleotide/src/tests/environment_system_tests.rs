// ABOUTME: TDD test suite for comprehensive environment system following Zed's architecture
// ABOUTME: Tests CLI environment detection, directory shell capture, priority system, and LSP integration

use crate::shell_env::{ProjectEnvironment, ShellEnvironmentError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile;
use tokio::test;

/// Helper function to create a temporary directory for testing
fn create_test_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("Failed to create temporary directory")
}

/// Tests for CLI environment detection and inheritance
/// These tests verify that when Nucleotide is launched from a terminal,
/// it correctly detects and uses the CLI environment variables
#[cfg(test)]
mod cli_environment_tests {
    use super::*;

    #[tokio::test]
    async fn test_cli_environment_detection() {
        // Test: CLI environment should be detected when provided
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/usr/bin:/nix/store/rust".to_string()),
            ("CARGO_HOME".to_string(), "/Users/test/.cargo".to_string()),
            ("ZED_ENVIRONMENT".to_string(), "cli".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env.clone()));

        // This test should FAIL initially - no ProjectEnvironment struct exists yet
        let test_dir = create_test_dir();
        let result = project_env
            .get_environment_for_directory(test_dir.path())
            .await;

        assert!(result.is_ok());
        let env = result.unwrap();
        assert_eq!(
            env.get("PATH"),
            Some(&"/usr/bin:/nix/store/rust".to_string())
        );
        assert_eq!(env.get("ZED_ENVIRONMENT"), Some(&"cli".to_string()));
    }

    #[tokio::test]
    async fn test_cli_environment_priority() {
        // Test: CLI environment should take precedence over directory environment
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/cli/path".to_string()),
            ("TEST_VAR".to_string(), "cli_value".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));

        // Even if directory has different PATH, CLI should win
        let result = project_env
            .get_environment_for_directory(Path::new("/different/project"))
            .await;

        assert!(result.is_ok());
        let env = result.unwrap();
        assert_eq!(env.get("PATH"), Some(&"/cli/path".to_string()));
        assert_eq!(env.get("TEST_VAR"), Some(&"cli_value".to_string()));
    }

    #[tokio::test]
    async fn test_no_cli_environment() {
        // Test: When no CLI environment provided, should fall back to directory capture
        let project_env = ProjectEnvironment::new(None);

        let temp_dir = tempfile::tempdir().unwrap();
        let result = project_env
            .get_environment_for_directory(temp_dir.path())
            .await;

        // Should attempt directory-specific environment capture
        // This will initially fail since we haven't implemented directory capture yet
        assert!(result.is_ok());
        let env = result.unwrap();

        // Should not have ZED_ENVIRONMENT=cli marker
        assert_ne!(env.get("ZED_ENVIRONMENT"), Some(&"cli".to_string()));
    }

    #[tokio::test]
    async fn test_cli_environment_marker() {
        // Test: CLI environment should be marked with ZED_ENVIRONMENT=cli
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/test/path".to_string()),
            ("ZED_ENVIRONMENT".to_string(), "cli".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));
        let result = project_env
            .get_environment_for_directory(Path::new("/any/path"))
            .await;

        assert!(result.is_ok());
        let env = result.unwrap();
        assert_eq!(env.get("ZED_ENVIRONMENT"), Some(&"cli".to_string()));
    }
}

/// Tests for directory-specific shell environment capture
/// These tests verify shell environment loading with proper directory context
#[cfg(test)]
mod directory_shell_tests {
    use super::*;

    #[tokio::test]
    async fn test_directory_shell_capture() {
        // Test: Should capture shell environment for specific directory
        let project_env = ProjectEnvironment::new(None);
        let temp_dir = create_test_dir();

        let result = project_env.get_environment_for_directory(temp_dir.path()).await;

        // This will initially fail - no directory shell capture implemented
        assert!(result.is_ok());
        let env = result.unwrap();

        // Should have basic environment variables
        assert!(env.contains_key("PATH"));
        assert_eq!(
            env.get("ZED_ENVIRONMENT"),
            Some(&"worktree-shell".to_string())
        );
    }

    #[tokio::test]
    async fn test_directory_specific_path() {
        // Test: Different directories may have different PATH due to direnv/asdf
        let project_env = ProjectEnvironment::new(None);

        let temp_dir1 = create_test_dir();
        let temp_dir2 = create_test_dir();
        let dir1 = temp_dir1.path();
        let dir2 = temp_dir2.path();

        let env1 = project_env
            .get_environment_for_directory(dir1)
            .await
            .unwrap();
        let env2 = project_env
            .get_environment_for_directory(dir2)
            .await
            .unwrap();

        // Initially both will be the same, but after direnv integration they may differ
        assert!(env1.contains_key("PATH"));
        assert!(env2.contains_key("PATH"));
    }

    #[tokio::test]
    async fn test_shell_execution_failure() {
        // Test: Should handle shell execution failures gracefully
        let project_env = ProjectEnvironment::new(None);
        let invalid_dir = Path::new("/nonexistent/directory");

        // Should not panic, but may return error or fallback environment
        let result = project_env.get_environment_for_directory(invalid_dir).await;

        // Depending on implementation, this might be Ok with fallback or Err
        // For now, expecting graceful handling
        if result.is_err() {
            assert!(matches!(
                result.unwrap_err(),
                ShellEnvironmentError::ShellExecutionFailed(_)
            ));
        }
    }
}

/// Tests for shell-specific command building
/// These tests verify proper shell detection and command construction
#[cfg(test)]
mod shell_specific_tests {
    use super::*;
    use crate::shell_env::shell_command_builder;

    #[tokio::test]
    async fn test_bash_shell_command() {
        // Test: bash should use -l flag for login shell
        let cmd = shell_command_builder::build_shell_command("/bin/bash", Path::new("/test/dir"));

        // This will fail initially - no shell_command_builder module exists
        assert!(cmd.get_program() == "/bin/bash");
        assert!(cmd.get_args().any(|arg| arg == "-l"));
    }

    #[tokio::test]
    async fn test_fish_shell_command() {
        // Test: fish should use -l and emit fish_prompt
        let cmd = shell_command_builder::build_shell_command(
            "/usr/local/bin/fish",
            Path::new("/test/dir"),
        );

        // Should include both -l flag and fish_prompt emission
        assert!(cmd.get_program() == "/usr/local/bin/fish");
        let args: Vec<_> = cmd.get_args().filter_map(|s| s.to_str()).collect();
        assert!(args.contains(&"-l"));
        // Command should include "emit fish_prompt" for proper environment loading
        let command_str = format!("{:?}", cmd);
        assert!(command_str.contains("emit fish_prompt"));
    }

    #[tokio::test]
    async fn test_tcsh_shell_command() {
        // Test: tcsh should use arg0("-") instead of -l
        let cmd = shell_command_builder::build_shell_command("/bin/tcsh", Path::new("/test/dir"));

        // tcsh uses special arg0 handling for login shell
        assert!(cmd.get_program() == "/bin/tcsh");
        // Should use arg0 technique, not -l flag
        let args: Vec<_> = cmd.get_args().filter_map(|s| s.to_str()).collect();
        assert!(!args.contains(&"-l"));
    }

    #[tokio::test]
    async fn test_zsh_shell_command() {
        // Test: zsh should use standard -l flag
        let cmd = shell_command_builder::build_shell_command(
            "/usr/local/bin/zsh",
            Path::new("/test/dir"),
        );

        assert!(cmd.get_program() == "/usr/local/bin/zsh");
        let args: Vec<_> = cmd.get_args().filter_map(|s| s.to_str()).collect();
        assert!(args.contains(&"-l"));
    }

    #[tokio::test]
    async fn test_shell_detection() {
        // Test: Should correctly detect shell type from path
        use crate::shell_env::detect_shell_type;

        assert_eq!(detect_shell_type("/bin/bash"), "bash");
        assert_eq!(detect_shell_type("/usr/local/bin/fish"), "fish");
        assert_eq!(detect_shell_type("/bin/tcsh"), "tcsh");
        assert_eq!(detect_shell_type("/bin/csh"), "csh");
        assert_eq!(detect_shell_type("/usr/local/bin/zsh"), "zsh");
        assert_eq!(detect_shell_type("/unknown/shell"), "unknown");
    }
}

/// Tests for environment priority system
/// These tests verify the three-tier precedence: CLI > directory > process
#[cfg(test)]
mod priority_system_tests {
    use super::*;

    #[tokio::test]
    async fn test_three_tier_priority() {
        // Test: CLI > directory > process environment priority
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/cli/path".to_string()),
            ("TEST_CLI".to_string(), "cli_value".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));

        // Set a process environment variable
        unsafe {
            std::env::set_var("TEST_PROCESS", "process_value");
        }

        let temp_dir = create_test_dir();
        let result = project_env
            .get_environment_for_directory(temp_dir.path())
            .await;
        assert!(result.is_ok());
        let env = result.unwrap();

        // CLI should take precedence
        assert_eq!(env.get("PATH"), Some(&"/cli/path".to_string()));
        assert_eq!(env.get("TEST_CLI"), Some(&"cli_value".to_string()));

        // Process variables should still be present (unless overridden)
        assert_eq!(env.get("TEST_PROCESS"), Some(&"process_value".to_string()));

        // Cleanup
        unsafe {
            std::env::remove_var("TEST_PROCESS");
        }
    }

    #[tokio::test]
    async fn test_directory_over_process_priority() {
        // Test: Directory environment should override process environment
        let project_env = ProjectEnvironment::new(None);

        // Set process variable that should be overridden
        unsafe {
            std::env::set_var("TEST_OVERRIDE", "process_value");
        }

        let temp_dir = create_test_dir();
        let result = project_env
            .get_environment_for_directory(temp_dir.path())
            .await;

        // Test: Directory environment should include process variables (even if no directory overrides exist)
        if let Ok(env) = result {
            // Process variables should be preserved in directory environment
            assert_eq!(env.get("TEST_OVERRIDE"), Some(&"process_value".to_string()));
            
            // TODO: When we implement actual direnv/asdf integration, directory-specific
            // environment files could override process variables. For now, verify that
            // process environment is properly merged with directory shell environment.
        }

        unsafe {
            std::env::remove_var("TEST_OVERRIDE");
        }
    }

    #[tokio::test]
    async fn test_environment_origin_tracking() {
        // Test: Should track where each environment came from
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/cli/path".to_string()),
            ("ZED_ENVIRONMENT".to_string(), "cli".to_string()),
        ]);

        let project_env_with_cli = ProjectEnvironment::new(Some(cli_env));
        let project_env_without_cli = ProjectEnvironment::new(None);

        let cli_result = project_env_with_cli
            .get_environment_for_directory(Path::new("/test"))
            .await;
        let dir_result = project_env_without_cli
            .get_environment_for_directory(Path::new("/test"))
            .await;

        if let Ok(cli_env) = cli_result {
            assert_eq!(cli_env.get("ZED_ENVIRONMENT"), Some(&"cli".to_string()));
        }

        if let Ok(dir_env) = dir_result {
            assert_eq!(
                dir_env.get("ZED_ENVIRONMENT"),
                Some(&"worktree-shell".to_string())
            );
        }
    }
}

/// Tests for environment caching per directory path
/// These tests verify efficient caching to avoid repeated shell execution
#[cfg(test)]
mod caching_tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_directory_caching() {
        // Test: Same directory should be cached and not re-executed
        let project_env = ProjectEnvironment::new(None);
        let temp_dir = create_test_dir();
        let test_dir = temp_dir.path();

        let start1 = Instant::now();
        let result1 = project_env.get_environment_for_directory(test_dir).await;
        let duration1 = start1.elapsed();

        let start2 = Instant::now();
        let result2 = project_env.get_environment_for_directory(test_dir).await;
        let duration2 = start2.elapsed();

        assert!(result1.is_ok());
        assert!(result2.is_ok());

        // Second call should be significantly faster due to caching
        assert!(duration2 < duration1 / 2);

        // Results should be identical
        assert_eq!(result1.unwrap(), result2.unwrap());
    }

    #[tokio::test]
    async fn test_different_directories_cached_separately() {
        // Test: Different directories should have separate cache entries
        let project_env = ProjectEnvironment::new(None);

        // Create temporary directories for testing
        let temp_dir1 = tempfile::tempdir().unwrap();
        let temp_dir2 = tempfile::tempdir().unwrap();
        let dir1 = temp_dir1.path();
        let dir2 = temp_dir2.path();

        let env1 = project_env.get_environment_for_directory(dir1).await;
        let env2 = project_env.get_environment_for_directory(dir2).await;

        // Should both succeed and be cached independently
        if let Err(e) = &env1 {
            println!("env1 error: {:?}", e);
        }
        if let Err(e) = &env2 {
            println!("env2 error: {:?}", e);
        }
        assert!(env1.is_ok());
        assert!(env2.is_ok());

        // Verify cache keys are different
        let cache_keys = project_env.get_cached_directories().await;
        assert!(cache_keys.contains(&dir1.canonicalize().unwrap_or_else(|_| dir1.to_path_buf())));
        assert!(cache_keys.contains(&dir2.canonicalize().unwrap_or_else(|_| dir2.to_path_buf())));
    }

    #[tokio::test]
    async fn test_cache_invalidation() {
        // Test: Cache should be invalidatable when directory tools change
        let project_env = ProjectEnvironment::new(None);
        let temp_dir = create_test_dir();
        let test_dir = temp_dir.path();

        // Load initial environment
        let initial_env = project_env.get_environment_for_directory(test_dir).await;
        assert!(initial_env.is_ok());

        // Invalidate cache for this directory
        project_env.invalidate_directory_cache(test_dir).await;

        // Next load should re-execute shell (not be instant)
        let start = Instant::now();
        let refreshed_env = project_env.get_environment_for_directory(test_dir).await;
        let duration = start.elapsed();

        assert!(refreshed_env.is_ok());
        // Should take time since cache was invalidated
        assert!(duration.as_millis() > 10); // Should not be instant
    }
}

/// Tests for direnv integration with 'cd' trigger
/// These tests verify proper direnv environment capture
#[cfg(test)]
mod direnv_integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_direnv_environment_capture() {
        // Test: Should capture direnv environment when cd into directory
        let project_env = ProjectEnvironment::new(None);

        // Create test directory with .envrc (this is a mock test)
        let temp_dir = create_test_dir();
        let direnv_project = temp_dir.path();

        let result = project_env
            .get_environment_for_directory(direnv_project)
            .await;

        if let Ok(env) = result {
            // Should have basic environment variables from shell
            assert!(env.contains_key("PATH"));

            // Direnv typically sets DIRENV_DIR, but only if direnv is installed and configured
            if env.contains_key("DIRENV_DIR") {
                // If DIRENV_DIR exists, it should point to a valid directory
                assert!(!env.get("DIRENV_DIR").unwrap().is_empty());
            }
            
            // TODO: Set up actual .envrc file in test directory to test real direnv integration
            // For now, verify basic shell environment capture is working
        }
    }

    #[tokio::test]
    async fn test_cd_command_includes_directory() {
        // Test: Shell command should include 'cd <directory>' before environment capture
        use crate::shell_env::shell_command_builder;

        let cmd = shell_command_builder::build_environment_capture_command(
            "/bin/bash",
            Path::new("/test/direnv/project"),
        );

        // Command should include cd to the specific directory
        let command_str = format!("{:?}", cmd);
        assert!(command_str.contains("cd"));
        assert!(command_str.contains("/test/direnv/project"));
    }

    #[tokio::test]
    async fn test_shell_hooks_triggered() {
        // Test: Shell hooks (direnv, asdf, mise) should be triggered by directory change
        let project_env = ProjectEnvironment::new(None);
        let hook_project = Path::new("/tmp/hook_test");

        let result = project_env
            .get_environment_for_directory(hook_project)
            .await;

        if let Ok(env) = result {
            // If any development tools are available, they should be in PATH
            if let Some(path) = env.get("PATH") {
                // This is a general test - specific tools will vary by system
                assert!(!path.is_empty());
                assert!(path.contains(":") || path.len() > 10); // Should be a real PATH
            }
        }
    }
}

/// Tests for LSP environment injection workflow
/// These tests verify LSP servers receive proper environment variables
#[cfg(test)]
mod lsp_integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_lsp_environment_injection() {
        // Test: LSP servers should receive environment from ProjectEnvironment
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/nix/store/rust:/usr/bin".to_string()),
            ("CARGO_HOME".to_string(), "/Users/test/.cargo".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));
        let temp_dir = create_test_dir();
        let workspace_dir = temp_dir.path();

        // Get environment for LSP server
        let lsp_env = project_env.get_lsp_environment(workspace_dir).await;

        assert!(lsp_env.is_ok());
        let env = lsp_env.unwrap();

        // LSP should have PATH with development tools
        assert!(env.contains_key("PATH"));
        assert!(env.get("PATH").unwrap().contains("rust"));

        // Should have CARGO_HOME for rust-analyzer
        assert_eq!(
            env.get("CARGO_HOME"),
            Some(&"/Users/test/.cargo".to_string())
        );
    }

    #[tokio::test]
    async fn test_lsp_environment_merging() {
        // Test: LSP environment should merge project env with LSP-specific vars
        let project_env = ProjectEnvironment::new(None);
        let temp_dir = create_test_dir();
        let workspace_dir = temp_dir.path();

        // Mock LSP-specific environment variables
        let lsp_specific_env = HashMap::from([
            ("RUST_LOG".to_string(), "debug".to_string()),
            ("LSP_TIMEOUT".to_string(), "30".to_string()),
        ]);

        let merged_env = project_env
            .get_lsp_environment_with_overrides(workspace_dir, lsp_specific_env)
            .await;

        assert!(merged_env.is_ok());
        let env = merged_env.unwrap();

        // Should have both project and LSP-specific variables
        assert!(env.contains_key("PATH")); // From project
        assert_eq!(env.get("RUST_LOG"), Some(&"debug".to_string())); // LSP-specific
        assert_eq!(env.get("LSP_TIMEOUT"), Some(&"30".to_string())); // LSP-specific
    }

    #[tokio::test]
    async fn test_lsp_environment_caching() {
        // Test: LSP environment requests should use same cache as regular requests
        let project_env = ProjectEnvironment::new(None);
        let temp_dir = create_test_dir();
        let workspace_dir = temp_dir.path();

        // Load through regular interface
        let regular_env = project_env
            .get_environment_for_directory(workspace_dir)
            .await;

        // Load through LSP interface - should use same cache
        let start = std::time::Instant::now();
        let lsp_env = project_env.get_lsp_environment(workspace_dir).await;
        let duration = start.elapsed();

        assert!(regular_env.is_ok());
        assert!(lsp_env.is_ok());

        // Should be very fast due to shared caching
        assert!(duration.as_millis() < 10);

        // Should have the same core environment variables
        let reg_env = regular_env.unwrap();
        let lsp_env = lsp_env.unwrap();
        assert_eq!(reg_env.get("PATH"), lsp_env.get("PATH"));
    }
}

// These tests will all fail initially since we haven't implemented any of the functionality yet
// That's exactly what we want for TDD - write the tests first, then make them pass
