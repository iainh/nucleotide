// ABOUTME: Nucleotide remote workspace helper binary
// ABOUTME: Runs inside remote environments such as WSL to expose workspace services

use anyhow::{Context, Result, bail};
use nucleotide_remote::{
    DEFAULT_FILE_READ_LIMIT, DEFAULT_WORKSPACE_SYMBOL_FILE_BYTE_LIMIT,
    DEFAULT_WORKSPACE_SYMBOL_FILE_LIMIT, DEFAULT_WORKSPACE_SYMBOL_TOTAL_BYTE_LIMIT,
    DirectoryListingResponse, EnvironmentResponse, FileCreateResponse, FileReadResponse,
    FileRenameResponse, FileSearchResponse, GlobalSearchResponse, HelloResponse,
    WorkspaceMetadataResponse, WorkspaceRootResponse, WorkspaceSymbolFilesOptions,
    WorkspaceSymbolFilesResponse, encode_json_line,
};

fn main() -> Result<()> {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "hello".to_string());

    match command.as_str() {
        "hello" => {
            let response = HelloResponse::current().context("failed to build hello response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "env" => {
            let response =
                EnvironmentResponse::current().context("failed to build environment response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "metadata" => {
            let response = WorkspaceMetadataResponse::current()
                .context("failed to build workspace metadata response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "root" => {
            let response = WorkspaceRootResponse::current()
                .context("failed to build workspace root response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "list" => {
            let response = DirectoryListingResponse::current()
                .context("failed to build directory listing response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "create-file" => {
            let name = std::env::var("NUCLEOTIDE_REMOTE_CREATE_NAME")
                .context("NUCLEOTIDE_REMOTE_CREATE_NAME is required")?;
            let response =
                FileCreateResponse::current_file(&name).context("failed to create remote file")?;
            print!("{}", encode_json_line(&response)?);
        }
        "create-directory" => {
            let name = std::env::var("NUCLEOTIDE_REMOTE_CREATE_NAME")
                .context("NUCLEOTIDE_REMOTE_CREATE_NAME is required")?;
            let response = FileCreateResponse::current_directory(&name)
                .context("failed to create remote directory")?;
            print!("{}", encode_json_line(&response)?);
        }
        "rename" => {
            let old_name = std::env::var("NUCLEOTIDE_REMOTE_RENAME_OLD_NAME")
                .context("NUCLEOTIDE_REMOTE_RENAME_OLD_NAME is required")?;
            let new_name = std::env::var("NUCLEOTIDE_REMOTE_RENAME_NEW_NAME")
                .context("NUCLEOTIDE_REMOTE_RENAME_NEW_NAME is required")?;
            let response = FileRenameResponse::current(&old_name, &new_name)
                .context("failed to rename remote path")?;
            print!("{}", encode_json_line(&response)?);
        }
        "files" => {
            let response =
                FileSearchResponse::current().context("failed to build file search response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "search" => {
            let query = std::env::var("NUCLEOTIDE_REMOTE_SEARCH_QUERY")
                .context("NUCLEOTIDE_REMOTE_SEARCH_QUERY is required")?;
            let smart_case = std::env::var("NUCLEOTIDE_REMOTE_SEARCH_SMART_CASE")
                .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
                .unwrap_or(true);
            let limit = std::env::var("NUCLEOTIDE_REMOTE_SEARCH_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(nucleotide_remote::DEFAULT_GLOBAL_SEARCH_LIMIT);
            let response = GlobalSearchResponse::current(&query, smart_case, limit)
                .context("failed to build global search response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "read" => {
            let path = std::env::var_os("NUCLEOTIDE_REMOTE_READ_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let limit = std::env::var("NUCLEOTIDE_REMOTE_READ_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_FILE_READ_LIMIT);
            let response = FileReadResponse::current(&path, limit)
                .context("failed to build file read response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "symbol-files" => {
            let response = WorkspaceSymbolFilesResponse::current(workspace_symbol_files_options())
                .context("failed to build workspace symbol file response")?;
            print!("{}", encode_json_line(&response)?);
        }
        "--help" | "-h" => {
            println!("nucleotide-remote hello");
            println!("nucleotide-remote env");
            println!("nucleotide-remote metadata");
            println!("nucleotide-remote root");
            println!("nucleotide-remote list");
            println!("nucleotide-remote create-file");
            println!("nucleotide-remote create-directory");
            println!("nucleotide-remote rename");
            println!("nucleotide-remote files");
            println!("nucleotide-remote search");
            println!("nucleotide-remote read");
            println!("nucleotide-remote symbol-files");
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }

    Ok(())
}

fn workspace_symbol_files_options() -> WorkspaceSymbolFilesOptions {
    WorkspaceSymbolFilesOptions {
        hidden: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_HIDDEN", false),
        parents: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_PARENTS", true),
        ignore: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_IGNORE", true),
        follow_links: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_FOLLOW_LINKS", false),
        git_ignore: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_GIT_IGNORE", true),
        git_global: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_GIT_GLOBAL", true),
        git_exclude: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_GIT_EXCLUDE", true),
        deduplicate_links: env_bool("NUCLEOTIDE_REMOTE_SYMBOLS_DEDUP_LINKS", true),
        max_depth: env_usize("NUCLEOTIDE_REMOTE_SYMBOLS_MAX_DEPTH", usize::MAX),
        file_limit: env_usize(
            "NUCLEOTIDE_REMOTE_SYMBOLS_FILE_LIMIT",
            DEFAULT_WORKSPACE_SYMBOL_FILE_LIMIT,
        )
        .unwrap_or(DEFAULT_WORKSPACE_SYMBOL_FILE_LIMIT),
        file_byte_limit: env_usize(
            "NUCLEOTIDE_REMOTE_SYMBOLS_FILE_BYTE_LIMIT",
            DEFAULT_WORKSPACE_SYMBOL_FILE_BYTE_LIMIT,
        )
        .unwrap_or(DEFAULT_WORKSPACE_SYMBOL_FILE_BYTE_LIMIT),
        total_byte_limit: env_usize(
            "NUCLEOTIDE_REMOTE_SYMBOLS_TOTAL_BYTE_LIMIT",
            DEFAULT_WORKSPACE_SYMBOL_TOTAL_BYTE_LIMIT,
        )
        .unwrap_or(DEFAULT_WORKSPACE_SYMBOL_TOTAL_BYTE_LIMIT),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(default)
}

fn env_usize(name: &str, none_value: usize) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .and_then(|value| (value != none_value).then_some(value))
}
