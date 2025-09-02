use anyhow::{Context, Result, anyhow};
use chrono::SecondsFormat;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug)]
struct Args {
    server_cmd: String,
    server_args: Vec<String>,
    log_path: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut server_cmd = None::<String>;
    let mut server_args: Vec<String> = Vec::new();
    let mut log_path = None::<PathBuf>;

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
    Ok(Args {
        server_cmd,
        server_args,
        log_path,
    })
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

    // Start real server child process
    let mut child = Command::new(&args.server_cmd)
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
                Ok(body) => {
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
                Ok(body) => {
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
