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
    WslPathShapeKind, WslRemoteHelperError, WslWorkspace, build_wsl_environment_capture_command,
    build_wsl_remote_create_directory_command, build_wsl_remote_create_file_command,
    build_wsl_remote_delete_path_command, build_wsl_remote_directory_listing_command,
    build_wsl_remote_duplicate_path_command, build_wsl_remote_env_command,
    build_wsl_remote_file_content_command, build_wsl_remote_file_read_command,
    build_wsl_remote_file_search_command, build_wsl_remote_file_write_command,
    build_wsl_remote_format_command, build_wsl_remote_global_search_command,
    build_wsl_remote_hello_command, build_wsl_remote_helper_install_command,
    build_wsl_remote_metadata_command, build_wsl_remote_move_path_command,
    build_wsl_remote_rename_path_command, build_wsl_remote_set_readonly_command,
    build_wsl_remote_workspace_root_command, build_wsl_remote_workspace_symbol_files_command,
    build_wsl_shell_command, create_wsl_remote_directory_blocking, create_wsl_remote_file_blocking,
    delete_wsl_remote_path_blocking, duplicate_wsl_remote_path_blocking,
    format_wsl_remote_file_blocking, install_wsl_remote_helper, load_wsl_remote_directory_listing,
    load_wsl_remote_directory_listing_blocking, load_wsl_remote_environment,
    load_wsl_remote_file_content_blocking, load_wsl_remote_file_read_blocking,
    load_wsl_remote_file_search_blocking, load_wsl_remote_global_search_blocking,
    load_wsl_remote_metadata, load_wsl_remote_workspace_root_blocking,
    load_wsl_remote_workspace_symbol_files_blocking, move_wsl_remote_path_blocking,
    probe_wsl_remote_helper, rename_wsl_remote_path_blocking, set_wsl_remote_readonly_blocking,
    write_wsl_remote_file_blocking, wsl_path_shape_fallback_kind, wsl_remote_helper_cache_path,
    wsl_remote_helper_create_directory_script, wsl_remote_helper_create_file_script,
    wsl_remote_helper_delete_path_script, wsl_remote_helper_directory_listing_script,
    wsl_remote_helper_duplicate_path_script, wsl_remote_helper_env_script,
    wsl_remote_helper_file_content_script, wsl_remote_helper_file_read_script,
    wsl_remote_helper_file_search_script, wsl_remote_helper_file_write_script,
    wsl_remote_helper_format_script, wsl_remote_helper_global_search_script,
    wsl_remote_helper_hello_script, wsl_remote_helper_install_script,
    wsl_remote_helper_metadata_script, wsl_remote_helper_move_path_script,
    wsl_remote_helper_rename_path_script, wsl_remote_helper_set_readonly_script,
    wsl_remote_helper_workspace_root_script, wsl_remote_helper_workspace_symbol_files_script,
};

#[cfg(test)]
#[allow(unused_imports)]
pub use shell_env_focused_test::*;
