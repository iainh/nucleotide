// ABOUTME: Parses user-facing remote project open inputs into workspace paths
// ABOUTME: Keeps Open Remote UI routing independent from transport startup code

use std::path::PathBuf;

use nucleotide_workspace::{classify_workspace_location, remote_path_is_probably_file};

pub const REMOTE_OPEN_PROMPT: &str = "remote:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOpenTarget {
    pub path: PathBuf,
    pub kind: RemoteOpenTargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOpenTargetKind {
    File,
    Directory,
}

pub fn parse_remote_open_input(input: &str) -> Result<RemoteOpenTarget, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(remote_open_usage());
    }

    let direct_path = PathBuf::from(input);
    if classify_workspace_location(&direct_path).is_remote() {
        return Ok(remote_open_target_for_path(direct_path));
    }

    if let Some(path) = ssh_command_input_to_uri(input)? {
        return Ok(remote_open_target_for_path(path));
    }

    Err(remote_open_usage())
}

fn remote_open_target_for_path(path: PathBuf) -> RemoteOpenTarget {
    let kind = if remote_path_is_probably_file(&path).unwrap_or(false) {
        RemoteOpenTargetKind::File
    } else {
        RemoteOpenTargetKind::Directory
    };

    RemoteOpenTarget { path, kind }
}

fn remote_open_usage() -> String {
    "Enter ssh://host/path, ssh user@host /path, or \\\\wsl.localhost\\Distro\\path".to_string()
}

fn ssh_command_input_to_uri(input: &str) -> Result<Option<PathBuf>, String> {
    let tokens = split_shell_words(input)?;
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut index = usize::from(tokens[0] == "ssh");
    if index == 0 {
        return Ok(None);
    }

    let mut port = None;
    let mut user = None;
    let mut target = None;
    let mut remote_path = None;

    while index < tokens.len() {
        let token = &tokens[index];

        match token.as_str() {
            "-p" => {
                index += 1;
                let Some(value) = tokens.get(index) else {
                    return Err("Missing value after ssh -p".to_string());
                };
                port = parse_port(value)?;
            }
            "-l" => {
                index += 1;
                let Some(value) = tokens.get(index) else {
                    return Err("Missing value after ssh -l".to_string());
                };
                if !value.is_empty() {
                    user = Some(value.clone());
                }
            }
            "-o" | "-i" | "-F" | "-J" | "-W" | "-b" | "-c" | "-m" | "-S" => {
                index += 1;
                if tokens.get(index).is_none() {
                    return Err(format!("Missing value after ssh {token}"));
                }
            }
            token if token.starts_with("-p") && token.len() > 2 => {
                port = parse_port(&token[2..])?;
            }
            token if token.starts_with("-l") && token.len() > 2 => {
                user = Some(token[2..].to_string());
            }
            token if token.starts_with('-') => {}
            token => {
                if target.is_none() {
                    let (parsed_target, parsed_path) = split_scp_like_target(token);
                    target = Some(parsed_target.to_string());
                    remote_path = parsed_path.map(str::to_string);
                } else if remote_path.is_none() {
                    remote_path = Some(token.to_string());
                }
            }
        }

        index += 1;
    }

    let Some(mut target) = target else {
        return Err("Missing SSH target".to_string());
    };

    if !target.contains('@')
        && let Some(user) = user
    {
        target = format!("{user}@{target}");
    }

    if let Some(port) = port
        && target_port(&target).is_none()
    {
        target.push(':');
        target.push_str(&port.to_string());
    }

    let remote_path = remote_path.unwrap_or_else(|| "/".to_string());
    let remote_path = if remote_path.starts_with('/') {
        remote_path
    } else {
        format!("/{remote_path}")
    };

    Ok(Some(PathBuf::from(format!(
        "ssh://{target}{}",
        percent_encode_posix_path(&remote_path)
    ))))
}

fn parse_port(value: &str) -> Result<Option<u16>, String> {
    value
        .parse::<u16>()
        .map(Some)
        .map_err(|_| format!("Invalid SSH port: {value}"))
}

fn target_port(target: &str) -> Option<&str> {
    let host = target
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(target);
    if host.starts_with('[') {
        return host
            .split_once(']')
            .and_then(|(_, rest)| rest.strip_prefix(':'));
    }

    let (host, port) = host.rsplit_once(':')?;
    (!host.contains(':')).then_some(port)
}

fn split_scp_like_target(value: &str) -> (&str, Option<&str>) {
    if value.starts_with('[') {
        return value
            .split_once("]:")
            .map(|(target, path)| {
                let target_end = target.len() + 1;
                (&value[..target_end], Some(path))
            })
            .unwrap_or((value, None));
    }

    let Some((target, path)) = value.rsplit_once(':') else {
        return (value, None);
    };

    if target.is_empty() || path.is_empty() || target.contains('/') || target.contains(':') {
        return (value, None);
    }

    (target, Some(path))
}

fn split_shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if quote != Some('\'') => {
                escaped = true;
            }
            '\'' | '"' if quote == Some(ch) => {
                quote = None;
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(ch);
            }
            ch if ch.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            ch => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }

    if let Some(quote) = quote {
        return Err(format!("Unclosed {quote} quote"));
    }

    if !current.is_empty() {
        words.push(current);
    }

    Ok(words)
}

fn percent_encode_posix_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push_str(&format!("{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_uri_directory() {
        assert_eq!(
            parse_remote_open_input("ssh://me@example.com/home/me/project").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com/home/me/project"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn parses_ssh_uri_file_hint() {
        assert_eq!(
            parse_remote_open_input("ssh://me@example.com/home/me/project/src/main.rs").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs"),
                kind: RemoteOpenTargetKind::File,
            }
        );
    }

    #[test]
    fn parses_wsl_unc_path() {
        assert_eq!(
            parse_remote_open_input(r"\\wsl.localhost\Ubuntu\home\me\project").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn parses_common_ssh_command_with_port_and_path() {
        assert_eq!(
            parse_remote_open_input("ssh -p 2222 me@example.com /home/me/project").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn parses_ssh_command_with_trailing_options() {
        assert_eq!(
            parse_remote_open_input("ssh me@example.com -p 2222 /home/me/project").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn parses_ssh_command_with_login_name() {
        assert_eq!(
            parse_remote_open_input("ssh -l me example.com /home/me/project").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com/home/me/project"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn parses_scp_like_target_path() {
        assert_eq!(
            parse_remote_open_input("ssh me@example.com:/home/me/project/src/main.rs").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs"),
                kind: RemoteOpenTargetKind::File,
            }
        );
    }

    #[test]
    fn encodes_ssh_command_path_spaces() {
        assert_eq!(
            parse_remote_open_input("ssh me@example.com '/home/me/Project One'").unwrap(),
            RemoteOpenTarget {
                path: PathBuf::from("ssh://me@example.com/home/me/Project%20One"),
                kind: RemoteOpenTargetKind::Directory,
            }
        );
    }

    #[test]
    fn rejects_local_paths() {
        assert!(parse_remote_open_input("/home/me/project").is_err());
    }
}
