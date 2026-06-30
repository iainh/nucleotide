// ABOUTME: Nucleotide remote workspace helper binary
// ABOUTME: Runs inside remote environments such as WSL to expose workspace services

use anyhow::{Context, Result, bail};
use nucleotide_remote::{EnvironmentResponse, HelloResponse, encode_json_line};

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
        "--help" | "-h" => {
            println!("nucleotide-remote hello");
            println!("nucleotide-remote env");
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }

    Ok(())
}
