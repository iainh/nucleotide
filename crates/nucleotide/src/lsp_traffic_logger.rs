use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use helix_lsp::LanguageServerId;
use once_cell::sync::Lazy;
use serde_json::Value as JsonValue;

static LOGGER_STATE: Lazy<LoggerState> = Lazy::new(LoggerState::new);

struct LoggerState {
    enabled: bool,
    inner: Mutex<Inner>,
}

struct Inner {
    dir: PathBuf,
    files: HashMap<LanguageServerId, File>,
}

impl LoggerState {
    fn new() -> Self {
        let enabled = std::env::var("NUCLEOTIDE_LSP_TRAFFIC")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let dir = PathBuf::from("logs").join("lsp");
        Self {
            enabled,
            inner: Mutex::new(Inner {
                dir,
                files: HashMap::new(),
            }),
        }
    }
}

fn ensure_dir(path: &Path) {
    if let Err(e) = fs::create_dir_all(path) {
        // Best-effort; if it fails, writing will also fail and be ignored.
        eprintln!(
            "nucleotide: failed to create LSP log dir {}: {}",
            path.display(),
            e
        );
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn get_file_mut() -> Option<std::sync::MutexGuard<'static, Inner>> {
    if !LOGGER_STATE.enabled {
        return None;
    }

    let guard = LOGGER_STATE.inner.lock().ok()?;
    Some(guard)
}

fn open_file_if_needed(inner: &mut Inner, server_id: LanguageServerId, server_name: &str) {
    if inner.files.contains_key(&server_id) {
        return;
    }

    ensure_dir(&inner.dir);
    let fname = format!("{}-{:?}.jsonl", sanitize_name(server_name), server_id);
    let path = inner.dir.join(fname);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            inner.files.insert(server_id, file);
        }
        Err(e) => {
            eprintln!(
                "nucleotide: failed to open LSP traffic log {}: {}",
                path.display(),
                e
            );
        }
    }
}

fn write_json_line(file: &mut File, value: &serde_json::Value) {
    if let Ok(mut s) = serde_json::to_string(value) {
        s.push('\n');
        let _ = file.write_all(s.as_bytes());
        let _ = file.flush();
    }
}

/// Log an incoming server->client LSP message (notification or method call) as a JSONL entry.
pub fn log_incoming(
    server_id: LanguageServerId,
    server_name: &str,
    method: &str,
    params: &JsonValue,
) {
    if !LOGGER_STATE.enabled {
        return;
    }
    if let Some(mut inner) = get_file_mut() {
        open_file_if_needed(&mut inner, server_id, server_name);
        if let Some(file) = inner.files.get_mut(&server_id) {
            let entry = serde_json::json!({
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "direction": "in",
                "server_name": server_name,
                "server_id": format!("{:?}", server_id),
                "method": method,
                "params": params,
            });
            write_json_line(file, &entry);
        }
    }
}

/// Log a server trace message (e.g., $/logTrace) to the same per-server file.
pub fn log_server_trace(server_id: LanguageServerId, server_name: &str, message: &str) {
    if !LOGGER_STATE.enabled {
        return;
    }
    if let Some(mut inner) = get_file_mut() {
        open_file_if_needed(&mut inner, server_id, server_name);
        if let Some(file) = inner.files.get_mut(&server_id) {
            let entry = serde_json::json!({
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "direction": "in",
                "server_name": server_name,
                "server_id": format!("{:?}", server_id),
                "trace": message,
            });
            write_json_line(file, &entry);
        }
    }
}

/// Optionally log an outgoing client->server LSP request.
pub fn log_outgoing(
    server_id: LanguageServerId,
    server_name: &str,
    method: &str,
    params: &JsonValue,
) {
    if !LOGGER_STATE.enabled {
        return;
    }
    if let Some(mut inner) = get_file_mut() {
        open_file_if_needed(&mut inner, server_id, server_name);
        if let Some(file) = inner.files.get_mut(&server_id) {
            let entry = serde_json::json!({
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "direction": "out",
                "server_name": server_name,
                "server_id": format!("{:?}", server_id),
                "method": method,
                "params": params,
            });
            write_json_line(file, &entry);
        }
    }
}
