use anyhow::{Context, Result, anyhow};
use chrono::SecondsFormat;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug)]
struct Args {
    server_cmd: String,
    server_args: Vec<String>,
    log_path: Option<PathBuf>,
    wsl_mapper: Option<WslPathMapper>,
}

fn parse_args() -> Result<Args> {
    let mut server_cmd = None::<String>;
    let mut server_args: Vec<String> = Vec::new();
    let mut log_path = None::<PathBuf>;
    let mut wsl_distro = None::<String>;
    let mut wsl_linux_root = None::<String>;
    let mut wsl_windows_root = None::<String>;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--server-cmd" => {
                server_cmd = Some(it.next().context("--server-cmd requires a value")?);
            }
            "--server-arg" => {
                if let Some(val) = it.next() {
                    server_args.push(val);
                } else {
                    return Err(anyhow!("--server-arg requires a value"));
                }
            }
            "--log" => {
                log_path = Some(PathBuf::from(it.next().context("--log requires a path")?));
            }
            "--wsl-distro" => {
                wsl_distro = Some(it.next().context("--wsl-distro requires a value")?);
            }
            "--wsl-linux-root" => {
                wsl_linux_root = Some(it.next().context("--wsl-linux-root requires a value")?);
            }
            "--wsl-windows-root" => {
                wsl_windows_root = Some(it.next().context("--wsl-windows-root requires a value")?);
            }
            _ => {
                // Support "--" separator: everything after goes to server args
                if arg == "--" {
                    server_args.extend(it);
                    break;
                } else if server_cmd.is_none() {
                    // Allow first positional to be the server command
                    server_cmd = Some(arg);
                } else {
                    server_args.push(arg);
                }
            }
        }
    }

    let server_cmd = server_cmd.context("server command not provided")?;
    let wsl_mapper = match (wsl_distro, wsl_linux_root, wsl_windows_root) {
        (None, None, None) => None,
        (Some(distro), Some(linux_root), Some(windows_root)) => {
            Some(WslPathMapper::new(distro, linux_root, windows_root))
        }
        _ => {
            return Err(anyhow!(
                "--wsl-distro, --wsl-linux-root, and --wsl-windows-root must be provided together"
            ));
        }
    };

    Ok(Args {
        server_cmd,
        server_args,
        log_path,
        wsl_mapper,
    })
}

#[derive(Debug, Clone)]
struct WslPathMapper {
    distro: String,
    linux_root: String,
    windows_root: String,
}

impl WslPathMapper {
    fn new(distro: String, linux_root: String, windows_root: String) -> Self {
        Self {
            distro,
            linux_root: normalize_linux_path(&linux_root),
            windows_root: normalize_windows_path(&windows_root),
        }
    }

    fn client_to_server_body(&self, body: &[u8]) -> Vec<u8> {
        self.transform_body(body, Direction::ClientToServer)
    }

    fn server_to_client_body(&self, body: &[u8]) -> Vec<u8> {
        self.transform_body(body, Direction::ServerToClient)
    }

    fn transform_body(&self, body: &[u8], direction: Direction) -> Vec<u8> {
        let Ok(mut value) = serde_json::from_slice::<JsonValue>(body) else {
            return body.to_vec();
        };

        self.transform_value(None, &mut value, direction);
        serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
    }

    fn transform_value(&self, key: Option<&str>, value: &mut JsonValue, direction: Direction) {
        match value {
            JsonValue::String(text) => {
                if let Some(mapped) = self.map_file_uri(text, direction) {
                    *text = mapped;
                } else if matches!(key, Some("rootPath"))
                    && let Some(mapped) = self.map_root_path(text, direction)
                {
                    *text = mapped;
                }
            }
            JsonValue::Array(items) => {
                for item in items {
                    self.transform_value(None, item, direction);
                }
            }
            JsonValue::Object(object) => self.transform_object(object, direction),
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => {}
        }
    }

    fn transform_object(&self, object: &mut JsonMap<String, JsonValue>, direction: Direction) {
        let original = std::mem::take(object);
        for (key, mut value) in original {
            self.transform_value(Some(key.as_str()), &mut value, direction);
            let mapped_key = self.map_file_uri(&key, direction).unwrap_or(key);
            object.insert(mapped_key, value);
        }
    }

    fn map_file_uri(&self, text: &str, direction: Direction) -> Option<String> {
        let url = url::Url::parse(text).ok()?;
        if url.scheme() != "file" {
            return None;
        }

        match direction {
            Direction::ClientToServer => self.windows_file_url_to_linux(&url),
            Direction::ServerToClient => self.linux_file_url_to_windows(&url),
        }
    }

    fn windows_file_url_to_linux(&self, url: &url::Url) -> Option<String> {
        let host = url.host_str()?;
        if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
            return None;
        }

        let path = normalized_file_url_path(url)?;
        let distro_prefix = format!("/{}", self.distro);
        let linux_path = if path == distro_prefix {
            "/".to_string()
        } else {
            path.strip_prefix(&(distro_prefix + "/"))
                .map(|suffix| format!("/{suffix}"))?
        };

        linux_file_url(&linux_path)
    }

    fn linux_file_url_to_windows(&self, url: &url::Url) -> Option<String> {
        if url.host_str().is_some() && url.host_str() != Some("localhost") {
            return None;
        }

        let linux_path = normalized_file_url_path(url)?;
        let linux_root = self.linux_root.as_str();
        if linux_path != linux_root
            && !linux_path
                .as_str()
                .starts_with(&(linux_root.to_string() + "/"))
        {
            return None;
        }

        let unc_path = format!("/{}{}", self.distro, linux_path);
        windows_wsl_file_url(&unc_path)
    }

    fn map_root_path(&self, text: &str, direction: Direction) -> Option<String> {
        match direction {
            Direction::ClientToServer => {
                let normalized = normalize_windows_path(text);
                let root = self.windows_root.as_str();
                if normalized == root {
                    Some(self.linux_root.clone())
                } else {
                    let suffix = normalized.strip_prefix(&(root.to_string() + "\\"))?;
                    Some(format!(
                        "{}/{}",
                        self.linux_root.trim_end_matches('/'),
                        suffix.replace('\\', "/")
                    ))
                }
            }
            Direction::ServerToClient => {
                let normalized = normalize_linux_path(text);
                let root = self.linux_root.as_str();
                if normalized == root {
                    Some(self.windows_root.clone())
                } else {
                    let suffix = normalized.strip_prefix(&(root.to_string() + "/"))?;
                    Some(format!(
                        "{}\\{}",
                        self.windows_root.trim_end_matches('\\'),
                        suffix.replace('/', "\\")
                    ))
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    ClientToServer,
    ServerToClient,
}

fn normalize_linux_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    if normalized.is_empty() {
        "/".to_string()
    } else if normalized.starts_with('/') {
        normalized
    } else {
        format!("/{normalized}")
    }
}

fn normalize_windows_path(path: &str) -> String {
    let mut normalized = path.replace('/', "\\");
    while normalized.len() > 1 && normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized
}

fn normalized_file_url_path(url: &url::Url) -> Option<String> {
    percent_decode_utf8(url.path()).map(|path| normalize_linux_path(&path))
}

fn percent_decode_utf8(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).copied()?;
            let low = bytes.get(index + 2).copied()?;
            decoded.push((hex_value(high)? << 4) | hex_value(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn linux_file_url(path: &str) -> Option<String> {
    let mut url = url::Url::parse("file:///").ok()?;
    url.set_path(&normalize_linux_path(path));
    Some(url.to_string())
}

fn windows_wsl_file_url(path: &str) -> Option<String> {
    let mut url = url::Url::parse("file://wsl.localhost/").ok()?;
    url.set_path(&normalize_linux_path(path));
    Some(url.to_string())
}

struct Logger {
    file: Option<tokio::fs::File>,
}

impl Logger {
    async fn new(path: Option<PathBuf>) -> Logger {
        if let Some(p) = path {
            if let Some(parent) = p.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)
                .await
            {
                Ok(file) => return Logger { file: Some(file) },
                Err(e) => eprintln!("proxy: failed to open log file {}: {}", p.display(), e),
            }
        }
        Logger { file: None }
    }

    async fn log_json(&mut self, direction: &str, body: &[u8]) {
        if let Some(f) = &mut self.file {
            let ts = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            let mut method = None::<String>;
            let mut id = None::<JsonValue>;
            if let Ok(v) = serde_json::from_slice::<JsonValue>(body) {
                method = v
                    .get("method")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());
                if let Some(req_id) = v.get("id") {
                    id = Some(req_id.clone());
                }
            }
            let entry = serde_json::json!({
                "ts": ts,
                "direction": direction,
                "method": method,
                "id": id,
                "raw": String::from_utf8_lossy(body),
            });
            let _ = f
                .write_all(serde_json::to_string(&entry).unwrap().as_bytes())
                .await;
            let _ = f.write_all(b"\n").await;
            let _ = f.flush().await;
        }
    }
}

async fn read_headers<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    buf.clear();
    loop {
        let mut byte = [0u8; 1];
        let n = reader.read(&mut byte).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n") {
            break;
        }
        if buf.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "header too large",
            ));
        }
    }
    Ok(())
}

fn parse_content_length(headers: &[u8]) -> Result<usize> {
    let text = std::str::from_utf8(headers).context("headers not utf8")?;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            let n = line
                .split(':')
                .nth(1)
                .context("malformed content-length")?
                .trim();
            return n
                .parse::<usize>()
                .map_err(|e| anyhow!("bad content-length: {}", e));
        }
    }
    Err(anyhow!("content-length not found"))
}

async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut headers = Vec::with_capacity(256);
    read_headers(reader, &mut headers).await?;
    let len = parse_content_length(&headers)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

async fn write_message<W: AsyncWriteExt + Unpin>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(body).await?;
    writer.flush().await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let wsl_mapper = args.wsl_mapper.clone();

    // Start real server child process
    let mut child = nucleotide_process::tokio_command(&args.server_cmd)
        .args(&args.server_args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn server: {}", args.server_cmd))?;

    let mut server_stdin = child.stdin.take().context("no child stdin")?;
    let mut server_stdout = child.stdout.take().context("no child stdout")?;

    let log = Arc::new(AsyncMutex::new(Logger::new(args.log_path).await));

    // Task: client stdin -> server stdin
    let mut cin = tokio::io::stdin();
    let log_to_server = Arc::clone(&log);
    let forward_to_server = async {
        let mut reader = &mut cin;
        let mut writer = &mut server_stdin;
        loop {
            match read_message(&mut reader).await {
                Ok(mut body) => {
                    if let Some(mapper) = &wsl_mapper {
                        body = mapper.client_to_server_body(&body);
                    }
                    log_to_server.lock().await.log_json("out", &body).await;
                    if let Err(e) = write_message(&mut writer, &body).await {
                        eprintln!("proxy: write to server failed: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    // EOF or error; end loop
                    eprintln!("proxy: stdin read ended: {}", e);
                    break;
                }
            }
        }
    };

    // Task: server stdout -> client stdout
    let mut cout = tokio::io::stdout();
    let log_to_client = Arc::clone(&log);
    let forward_to_client = async {
        let mut reader = &mut server_stdout;
        let mut writer = &mut cout;
        loop {
            match read_message(&mut reader).await {
                Ok(mut body) => {
                    if let Some(mapper) = &wsl_mapper {
                        body = mapper.server_to_client_body(&body);
                    }
                    log_to_client.lock().await.log_json("in", &body).await;
                    if let Err(e) = write_message(&mut writer, &body).await {
                        eprintln!("proxy: write to client failed: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("proxy: server read ended: {}", e);
                    break;
                }
            }
        }
    };

    // Run both directions concurrently
    tokio::select! {
        _ = forward_to_server => {},
        _ = forward_to_client => {},
    }

    // Try to terminate child
    let _ = child.kill().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper() -> WslPathMapper {
        WslPathMapper::new(
            "Ubuntu".to_string(),
            "/home/iain/repo".to_string(),
            r"\\wsl.localhost\Ubuntu\home\iain\repo".to_string(),
        )
    }

    #[test]
    fn maps_initialize_payload_to_linux_paths() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootPath": r"\\wsl.localhost\Ubuntu\home\iain\repo",
                "rootUri": "file://wsl.localhost/Ubuntu/home/iain/repo",
                "workspaceFolders": [{
                    "uri": "file://wsl.localhost/Ubuntu/home/iain/repo",
                    "name": "repo"
                }]
            }
        });

        let mapped = mapper().client_to_server_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(mapped["params"]["rootPath"], "/home/iain/repo");
        assert_eq!(mapped["params"]["rootUri"], "file:///home/iain/repo");
        assert_eq!(
            mapped["params"]["workspaceFolders"][0]["uri"],
            "file:///home/iain/repo"
        );
    }

    #[test]
    fn maps_text_document_payload_to_linux_paths() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file://wsl.localhost/Ubuntu/home/iain/repo/src/main.rs",
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn main() {}"
                }
            }
        });

        let mapped = mapper().client_to_server_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(
            mapped["params"]["textDocument"]["uri"],
            "file:///home/iain/repo/src/main.rs"
        );
    }

    #[test]
    fn maps_server_diagnostics_back_to_wsl_unc_uris() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///home/iain/repo/src/main.rs",
                "diagnostics": []
            }
        });

        let mapped = mapper().server_to_client_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(
            mapped["params"]["uri"],
            "file://wsl.localhost/Ubuntu/home/iain/repo/src/main.rs"
        );
    }

    #[test]
    fn maps_workspace_edits_back_to_wsl_unc_uris() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "changes": {
                    "file:///home/iain/repo/src/lib.rs": [{
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0}
                        },
                        "newText": "pub "
                    }]
                },
                "documentChanges": [{
                    "textDocument": {
                        "uri": "file:///home/iain/repo/src/lib.rs",
                        "version": 1
                    },
                    "edits": []
                }]
            }
        });

        let mapped = mapper().server_to_client_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(
            mapped["result"]["documentChanges"][0]["textDocument"]["uri"],
            "file://wsl.localhost/Ubuntu/home/iain/repo/src/lib.rs"
        );
        assert!(
            mapped["result"]["changes"]
                .as_object()
                .unwrap()
                .contains_key("file://wsl.localhost/Ubuntu/home/iain/repo/src/lib.rs")
        );
    }

    #[test]
    fn leaves_unrelated_file_uris_unchanged() {
        let body = serde_json::json!({
            "uri": "file:///tmp/outside.rs",
            "other": "https://example.com/file.rs"
        });

        let mapped = mapper().server_to_client_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(mapped["uri"], "file:///tmp/outside.rs");
        assert_eq!(mapped["other"], "https://example.com/file.rs");
    }

    #[test]
    fn maps_percent_encoded_wsl_uris_without_double_encoding() {
        let mapper = WslPathMapper::new(
            "Ubuntu".to_string(),
            "/home/iain/my repo".to_string(),
            r"\\wsl.localhost\Ubuntu\home\iain\my repo".to_string(),
        );
        let body = serde_json::json!({
            "uri": "file://wsl.localhost/Ubuntu/home/iain/my%20repo/src/hash%23tag.rs"
        });

        let mapped = mapper.client_to_server_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(
            mapped["uri"],
            "file:///home/iain/my%20repo/src/hash%23tag.rs"
        );
    }

    #[test]
    fn maps_percent_encoded_linux_uris_back_to_wsl_without_double_encoding() {
        let mapper = WslPathMapper::new(
            "Ubuntu".to_string(),
            "/home/iain/my repo".to_string(),
            r"\\wsl.localhost\Ubuntu\home\iain\my repo".to_string(),
        );
        let body = serde_json::json!({
            "uri": "file:///home/iain/my%20repo/src/hash%23tag.rs"
        });

        let mapped = mapper.server_to_client_body(&serde_json::to_vec(&body).unwrap());
        let mapped: JsonValue = serde_json::from_slice(&mapped).unwrap();

        assert_eq!(
            mapped["uri"],
            "file://wsl.localhost/Ubuntu/home/iain/my%20repo/src/hash%23tag.rs"
        );
    }
}
