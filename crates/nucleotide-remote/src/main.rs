// ABOUTME: Nucleotide remote workspace helper binary
// ABOUTME: Runs inside remote environments such as WSL to expose workspace services

use anyhow::{Context, Result, bail};
use nucleotide_remote::{
    DEFAULT_FILE_READ_LIMIT, DirectoryListingResponse, EnvironmentResponse, FileReadResponse,
    FileSearchResponse, GlobalSearchResponse, HelloResponse, WorkspaceMetadataResponse,
    WorkspaceRootResponse, encode_json_line,
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
        "--help" | "-h" => {
            println!("nucleotide-remote hello");
            println!("nucleotide-remote env");
            println!("nucleotide-remote metadata");
            println!("nucleotide-remote root");
            println!("nucleotide-remote list");
            println!("nucleotide-remote files");
            println!("nucleotide-remote search");
            println!("nucleotide-remote read");
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }

    Ok(())
}
