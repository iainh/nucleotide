// ABOUTME: Focused test for ProjectEnvironment to validate core implementation without external dependencies
// ABOUTME: This tests our shell environment system in isolation

#[cfg(test)]
mod focused_shell_env_tests {
    use crate::shell_env::{
        detect_shell_type, parse_shell_environment, shell_command_builder, ProjectEnvironment,
    };
    use std::collections::HashMap;
    use std::path::Path;

    #[tokio::test]
    async fn test_project_environment_cli_priority() {
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/cli/path".to_string()),
            ("TEST_VAR".to_string(), "cli_value".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));

        let result = project_env
            .get_environment_for_directory(Path::new("/any/dir"))
            .await;
        assert!(result.is_ok());

        let env = result.unwrap();
        assert_eq!(env.get("PATH"), Some(&"/cli/path".to_string()));
        assert_eq!(env.get("TEST_VAR"), Some(&"cli_value".to_string()));
        assert_eq!(env.get("ZED_ENVIRONMENT"), Some(&"cli".to_string()));
    }

    #[tokio::test]
    async fn test_shell_detection_basics() {
        assert_eq!(detect_shell_type("/bin/bash"), "bash");
        assert_eq!(detect_shell_type("/usr/local/bin/fish"), "fish");
        assert_eq!(detect_shell_type("/bin/tcsh"), "tcsh");
        assert_eq!(detect_shell_type("/bin/zsh"), "zsh");
        assert_eq!(detect_shell_type("/unknown/shell"), "unknown");
    }

    #[tokio::test]
    async fn test_environment_parsing() {
        let test_output = b"PATH=/usr/bin:/bin\0HOME=/Users/test\0SHELL=/bin/bash\0";
        let parsed = parse_shell_environment(test_output).unwrap();

        assert_eq!(parsed.get("PATH"), Some(&"/usr/bin:/bin".to_string()));
        assert_eq!(parsed.get("HOME"), Some(&"/Users/test".to_string()));
        assert_eq!(parsed.get("SHELL"), Some(&"/bin/bash".to_string()));
    }

    #[tokio::test]
    async fn test_shell_command_building() {
        let bash_cmd = shell_command_builder::build_shell_command("/bin/bash", Path::new("/test"));
        assert_eq!(bash_cmd.get_program(), "/bin/bash");

        let fish_cmd =
            shell_command_builder::build_shell_command("/usr/local/bin/fish", Path::new("/test"));
        assert_eq!(fish_cmd.get_program(), "/usr/local/bin/fish");

        let tcsh_cmd = shell_command_builder::build_shell_command("/bin/tcsh", Path::new("/test"));
        assert_eq!(tcsh_cmd.get_program(), "/bin/tcsh");
    }

    #[tokio::test]
    async fn test_environment_capture_command() {
        let result = shell_command_builder::build_environment_capture_command(
            "/bin/bash",
            Path::new("/test/dir"),
        );
        assert!(result.is_ok());

        let cmd = result.unwrap();
        assert_eq!(cmd.get_program(), "/bin/bash");

        // Command should include both cd and printenv
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("cd"));
        assert!(debug_str.contains("printenv"));
    }

    #[tokio::test]
    async fn test_fish_shell_command_special_handling() {
        let result = shell_command_builder::build_environment_capture_command(
            "/usr/local/bin/fish",
            Path::new("/test/dir"),
        );
        assert!(result.is_ok());

        let cmd = result.unwrap();
        assert_eq!(cmd.get_program(), "/usr/local/bin/fish");

        // Fish should include emit fish_prompt
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("emit fish_prompt"));
    }

    #[tokio::test]
    async fn test_project_environment_caching_interface() {
        let project_env = ProjectEnvironment::new(None);

        // Test that cache interface methods exist and work
        let cached_dirs = project_env.get_cached_directories().await;
        assert_eq!(cached_dirs.len(), 0); // Should be empty initially

        // Test cache invalidation doesn't panic
        project_env
            .invalidate_directory_cache(Path::new("/test"))
            .await;
        project_env.clear_all_caches().await;
    }

    #[tokio::test]
    async fn test_lsp_environment_methods() {
        let cli_env = HashMap::from([
            ("PATH".to_string(), "/nix/store/rust:/usr/bin".to_string()),
            ("CARGO_HOME".to_string(), "/Users/test/.cargo".to_string()),
        ]);

        let project_env = ProjectEnvironment::new(Some(cli_env));

        // Test basic LSP environment
        let lsp_env = project_env
            .get_lsp_environment(Path::new("/test/project"))
            .await;
        assert!(lsp_env.is_ok());

        let env = lsp_env.unwrap();
        assert!(env.get("PATH").unwrap().contains("rust"));
        assert_eq!(
            env.get("CARGO_HOME"),
            Some(&"/Users/test/.cargo".to_string())
        );

        // Test LSP environment with overrides
        let overrides = HashMap::from([("RUST_LOG".to_string(), "debug".to_string())]);

        let merged_env = project_env
            .get_lsp_environment_with_overrides(Path::new("/test/project"), overrides)
            .await;
        assert!(merged_env.is_ok());

        let env = merged_env.unwrap();
        assert!(env.contains_key("PATH"));
        assert_eq!(env.get("RUST_LOG"), Some(&"debug".to_string()));
    }
}
