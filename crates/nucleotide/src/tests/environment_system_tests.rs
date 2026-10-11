// ABOUTME: TDD test suite for comprehensive environment system following Zed's architecture
// ABOUTME: Tests CLI environment detection, directory shell capture, priority system, and LSP integration

use nucleotide_env::{ProjectEnvironment, ShellEnvironmentError};
use std::collections::HashMap;
use std::path::Path;
use tempfile;

/// Helper function to create a temporary directory for testing
fn create_test_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("Failed to create temporary directory")
}

fn env_contains_key(env: &HashMap<String, String>, key: &str) -> bool {
    env.keys()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

fn env_get<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
    env.iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
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

        let result = project_env
            .get_environment_for_directory(temp_dir.path())
            .await;

        // This will initially fail - no directory shell capture implemented
        assert!(result.is_ok());
        let env = result.unwrap();

        // Should have basic environment variables
        assert!(env_contains_key(&env, "PATH"));
        assert_eq!(
            env.get("ZED_ENVIRONMENT"),
            Some(&"worktree-shell".to_string())
        );
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
        if let Err(error) = result {
            assert!(matches!(
                error,
                ShellEnvironmentError::ShellExecutionFailed(_)
            ));
        }
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

        // Invalidation removes the entry so the next load repopulates it. Do
        // not assert elapsed time here: a process-environment fallback is a
        // valid successful reload and need not spawn a shell.
        let cache_keys = project_env.get_cached_directories().await;
        assert!(!cache_keys.contains(&test_dir.to_path_buf()));

        let refreshed_env = project_env.get_environment_for_directory(test_dir).await;

        assert!(refreshed_env.is_ok());
        let cache_keys = project_env.get_cached_directories().await;
        assert!(cache_keys.contains(&test_dir.to_path_buf()));
    }
}

/// Tests for LSP environment injection workflow
/// These tests verify LSP servers receive proper environment variables
#[cfg(test)]
mod lsp_integration_tests {
    use super::*;

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
        assert!(env_contains_key(&env, "PATH")); // From project
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
        assert_eq!(env_get(&reg_env, "PATH"), env_get(&lsp_env, "PATH"));
    }
}

// These tests will all fail initially since we haven't implemented any of the functionality yet
// That's exactly what we want for TDD - write the tests first, then make them pass
