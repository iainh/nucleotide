use anyhow::{Context, Result};
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;

fn get_git_commit_hash() -> Option<String> {
    // Try to get git commit hash at runtime
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        && output.status.success()
    {
        let commit_hash = String::from_utf8_lossy(&output.stdout).to_string();
        let commit_hash = commit_hash.trim().to_string();
        if !commit_hash.is_empty() {
            return Some(commit_hash);
        }
    }
    None
}

pub fn parse_args() -> Result<Args> {
    let help = format!(
        "
{}
{}
{}

USAGE:
    nucl [FLAGS] [files]...

ARGS:
    <files>...    Sets the input file to use, position can also be specified via file[:row[:col]]

FLAGS:
    -h, --help                     Prints help information
    --tutor                        Loads the tutorial
    --health [CATEGORY]            Checks for potential errors in editor setup
                                   CATEGORY can be a language or one of 'clipboard', 'languages'
                                   or 'all'. 'all' is the default if not specified.
    -g, --grammar {{fetch|build}}    Fetches or builds tree-sitter grammars listed in languages.toml
    -c, --config <file>            Specifies a file to use for configuration
    -v                             Increases logging verbosity each use for up to 3 times
    --log <file>                   Specifies a file to use for logging
                                   (default file: {})
    -V, --version                  Prints version information
    --vsplit                       Splits all given files vertically into different windows
    --hsplit                       Splits all given files horizontally into different windows
    -w, --working-dir <path>       Specify an initial working directory
    +N                             Open the first given file at line number N
",
        env!("CARGO_PKG_NAME"),
        VERSION_AND_GIT_HASH,
        env!("CARGO_PKG_AUTHORS"),
        helix_loader::default_log_file().display(),
    );

    let args = Args::parse_args().context("could not parse arguments")?;

    if args.display_help {
        print!("{help}");
        std::process::exit(0);
    }

    if args.display_version {
        let commit_hash = get_git_commit_hash().unwrap_or_else(|| "unknown".to_string());
        eprintln!("Nucleotide {}", env!("CARGO_PKG_VERSION"));
        eprintln!("Commit {}", commit_hash);
        eprintln!("The Nucleotide Contributors");
        eprintln!();
        eprintln!("A native GUI implementation of the Helix modal text editor");
        eprintln!("Built with GPUI and Rust");
        std::process::exit(0);
    }

    Ok(args)
}
