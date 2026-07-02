use anyhow::{Context, Result};
use helix_core::Position;
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;
use std::path::PathBuf;

fn get_git_commit_hash() -> Option<String> {
    // Try to get git commit hash at runtime
    if let Ok(output) = nucleotide_process::command("git")
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

    let raw_args = std::env::args().collect::<Vec<_>>();
    let mut args = Args::parse_args().context("could not parse arguments")?;
    repair_remote_file_args(&mut args, &raw_args);

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

fn repair_remote_file_args(args: &mut Args, raw_args: &[String]) {
    let remote_files = remote_file_args_from_argv(raw_args);

    for (path, _) in &remote_files {
        args.files.shift_remove(path);

        let canonicalized = helix_stdx::path::canonicalize(path);
        if canonicalized != *path {
            args.files.shift_remove(&canonicalized);
        }
    }

    for (path, position) in remote_files {
        args.files
            .entry(path)
            .and_modify(|positions| positions.push(position))
            .or_insert_with(|| vec![position]);
    }
}

fn remote_file_args_from_argv(raw_args: &[String]) -> Vec<(PathBuf, Position)> {
    let mut files = Vec::new();
    let mut line_number = None;
    let mut index = 1;

    while index < raw_args.len() {
        let arg = &raw_args[index];
        match arg.as_str() {
            "--" => {
                for arg in &raw_args[index + 1..] {
                    if let Some(file) = parse_remote_file_arg(arg) {
                        files.push(file);
                    }
                }
                break;
            }
            "--version" | "--help" | "--tutor" | "--vsplit" | "--hsplit" => {}
            "--health" => {
                if raw_args
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with('-'))
                {
                    index += 1;
                }
            }
            "-g" | "--grammar" | "-c" | "--config" | "--log" | "-w" | "--working-dir" => {
                index += 1;
            }
            arg if arg.starts_with("--") => {}
            arg if arg.starts_with('-') => {}
            arg if arg.starts_with('+') => match arg[1..].parse::<usize>() {
                Ok(line) => line_number = Some(line.saturating_sub(1)),
                Err(_) => {
                    if let Some(file) = parse_remote_file_arg(arg) {
                        files.push(file);
                    }
                }
            },
            arg => {
                if let Some(file) = parse_remote_file_arg(arg) {
                    files.push(file);
                }
            }
        }

        index += 1;
    }

    if let Some(row) = line_number
        && let Some((_, position)) = files.first_mut()
    {
        position.row = row;
    }

    files
}

fn parse_remote_file_arg(value: &str) -> Option<(PathBuf, Position)> {
    let (path, position) = split_remote_path_position(value);
    let path = PathBuf::from(path);

    if nucleotide_workspace::classify_workspace_location(&path).is_remote() {
        Some((path, position))
    } else {
        None
    }
}

fn split_remote_path_position(value: &str) -> (&str, Position) {
    let trimmed = value.trim_end_matches(':');
    let min_position_colon = remote_path_position_colon_minimum(trimmed);

    if let Some((path, row, column)) = split_remote_path_row_col(trimmed, min_position_colon) {
        return (
            path,
            Position::new(row.saturating_sub(1), column.saturating_sub(1)),
        );
    }

    if let Some((path, row)) = split_remote_path_row(trimmed, min_position_colon) {
        return (path, Position::new(row.saturating_sub(1), 0));
    }

    (value, Position::default())
}

fn remote_path_position_colon_minimum(value: &str) -> usize {
    let Some(rest) = strip_prefix_ignore_ascii_case(value, "ssh://") else {
        return 0;
    };

    rest.find('/')
        .map(|path_start| "ssh://".len() + path_start)
        .unwrap_or(value.len())
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .then(|| &value[prefix.len()..])
}

fn split_remote_path_row_col(value: &str, min_colon: usize) -> Option<(&str, usize, usize)> {
    let (path_and_row, column) = value.rsplit_once(':')?;
    let column = column.parse::<usize>().ok()?;
    let column_colon = path_and_row.len();
    if column_colon <= min_colon {
        return None;
    }

    let (path, row) = path_and_row.rsplit_once(':')?;
    let row_colon = path.len();
    if row_colon <= min_colon {
        return None;
    }

    let row = row.parse::<usize>().ok()?;
    Some((path, row, column))
}

fn split_remote_path_row(value: &str, min_colon: usize) -> Option<(&str, usize)> {
    let (path, row) = value.rsplit_once(':')?;
    let row_colon = path.len();
    if row_colon <= min_colon {
        return None;
    }

    let row = row.parse::<usize>().ok()?;
    Some((path, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn remote_file_args_preserve_ssh_uri_with_port() {
        let files = remote_file_args_from_argv(&argv(&[
            "nucl",
            "ssh://me@example.com:2222/home/me/project/src/main.rs",
        ]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from("ssh://me@example.com:2222/home/me/project/src/main.rs"),
                Position::default()
            )]
        );
    }

    #[test]
    fn remote_file_args_parse_ssh_uri_position_suffix() {
        let files = remote_file_args_from_argv(&argv(&[
            "nucl",
            "ssh://me@example.com:2222/home/me/project/src/main.rs:12:4",
        ]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from("ssh://me@example.com:2222/home/me/project/src/main.rs"),
                Position::new(11, 3)
            )]
        );
    }

    #[test]
    fn remote_file_args_parse_uppercase_ssh_uri_position_suffix() {
        let files = remote_file_args_from_argv(&argv(&[
            "nucl",
            "SSH://me@example.com:2222/home/me/project/src/main.rs:12:4",
        ]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from("SSH://me@example.com:2222/home/me/project/src/main.rs"),
                Position::new(11, 3)
            )]
        );
    }

    #[test]
    fn remote_file_args_do_not_treat_ssh_port_as_position() {
        let files = remote_file_args_from_argv(&argv(&["nucl", "ssh://me@example.com:2222"]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from("ssh://me@example.com:2222"),
                Position::default()
            )]
        );
    }

    #[test]
    fn remote_file_args_do_not_treat_uppercase_ssh_port_as_position() {
        let files = remote_file_args_from_argv(&argv(&["nucl", "SSH://me@example.com:2222"]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from("SSH://me@example.com:2222"),
                Position::default()
            )]
        );
    }

    #[test]
    fn remote_file_args_preserve_wsl_unc_path() {
        let files = remote_file_args_from_argv(&argv(&[
            "nucl",
            r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs",
        ]));

        assert_eq!(
            files,
            vec![(
                PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs"),
                Position::default()
            )]
        );
    }

    #[test]
    fn remote_file_args_apply_plus_line_to_first_remote_file() {
        let files = remote_file_args_from_argv(&argv(&[
            "nucl",
            "+42",
            "ssh://me@example.com/home/me/project/src/main.rs",
        ]));

        assert_eq!(files[0].1, Position::new(41, 0));
    }

    #[test]
    fn repair_remote_file_args_replaces_helix_canonicalized_uri() {
        let mut args = Args::default();
        let remote_path = PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs");
        let canonicalized = helix_stdx::path::canonicalize(&remote_path);
        args.files.insert(canonicalized, vec![Position::default()]);

        repair_remote_file_args(
            &mut args,
            &argv(&["nucl", "ssh://me@example.com/home/me/project/src/main.rs"]),
        );

        assert!(args.files.contains_key(&remote_path));
        assert_eq!(args.files.len(), 1);
    }

    #[test]
    fn repair_remote_file_args_does_not_duplicate_unc_path() {
        let mut args = Args::default();
        let remote_path =
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs");
        args.files
            .insert(remote_path.clone(), vec![Position::default()]);

        repair_remote_file_args(
            &mut args,
            &argv(&[
                "nucl",
                r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs",
            ]),
        );

        assert_eq!(args.files.get(&remote_path).map(Vec::len), Some(1));
        assert_eq!(args.files.len(), 1);
    }
}
