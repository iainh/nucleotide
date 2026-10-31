// ABOUTME: Bounded Windows-to-real-WSL exercise of the public workspace backend
// ABOUTME: Uses disposable fixtures, asserts results, and emits JSONL evidence
use anyhow::{Context, Result, bail, ensure};
use nucleotide_remote::*;
use nucleotide_workspace::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Clone)]
struct Log(Arc<Mutex<std::fs::File>>);

impl Log {
    fn emit(&self, event: &str, detail: Value) {
        let line = json!({"timestamp_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
            "event": event, "detail": detail}).to_string();
        println!("{line}");
        let mut file = self.0.lock().unwrap();
        writeln!(file, "{line}").unwrap();
        file.flush().unwrap();
    }

    async fn step<T>(&self, name: &str, future: impl Future<Output = Result<T>>) -> Result<T> {
        self.emit("begin", json!({"step": name}));
        let start = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(30), future)
            .await
            .context("30-second step deadline exceeded")
            .and_then(|r| r)
            .with_context(|| name.to_owned());
        self.emit(
            if result.is_ok() { "pass" } else { "fail" },
            json!({"step": name, "elapsed_ms": start.elapsed().as_millis(),
                "error": result.as_ref().err().map(|e| format!("{e:#}"))}),
        );
        result
    }
}

struct Options {
    distro: String,
    helper: PathBuf,
    seconds: u64,
    iterations: u64,
    seed: u64,
    iterations_per_connection: u64,
    inject_failure: bool,
    log: Log,
}

fn options() -> Result<Options> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 8 || args.len() == 9,
        "usage: wsl_soak --run DISTRO /absolute/existing/helper SECONDS MAX_ITERATIONS SEED ITERATIONS_PER_CONNECTION LOG.jsonl [--inject-failure]"
    );
    // Keep the command line positional so the documented invocation has no hidden defaults.
    ensure!(args[0] == "--run", "first argument must be --run");
    let seconds = args[3].parse::<u64>()?;
    let iterations = args[4].parse::<u64>()?;
    let iterations_per_connection = args[6].parse::<u64>()?;
    ensure!(
        seconds > 0 && seconds <= 604800 && iterations > 0 && iterations_per_connection > 0,
        "invalid duration/iteration bounds"
    );
    ensure!(
        args[2].starts_with('/'),
        "helper must be an absolute Linux path"
    );
    let inject_failure = args.get(8).is_some_and(|v| v == "--inject-failure");
    ensure!(args.len() == 8 || inject_failure, "unknown argument");
    Ok(Options {
        distro: args[1].clone(),
        helper: args[2].clone().into(),
        seconds,
        iterations,
        seed: args[5].parse()?,
        iterations_per_connection,
        inject_failure,
        log: Log(Arc::new(Mutex::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&args[7])?,
        ))),
    })
}

// Sending a script over stdin avoids PowerShell/WSL nested quoting, and never
// targets a caller-supplied project. Every invocation has its own kill-on-drop.
async fn wsl(distro: &str, script: &str, args: &[&str]) -> Result<String> {
    let mut child = tokio::process::Command::new("wsl.exe")
        .args(["--distribution", distro, "--exec", "/bin/sh", "-s", "--"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await??;
    ensure!(
        output.status.success(),
        "WSL fixture command: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

async fn cleanup_fixture(distro: &str, root: &str) -> Result<String> {
    wsl(
        distro,
        r#"set -eu
case "$1" in /tmp/nucleotide-soak-*) ;; *) exit 90;; esac
test "$(cat "$1/.soak-owner")" = nucleotide-wsl-soak
owned() {
    for entry in /proc/[0-9]*/cwd; do
        cwd=$(readlink "$entry" 2>/dev/null) || continue
        case "$cwd" in "$1"|"$1/"*) pid=${entry#/proc/}; printf '%s\n' "${pid%/cwd}";; esac
    done
}
for n in 1 2 3 4 5; do
    pids=$(owned "$1")
    test -n "$pids" || break
    sleep 1
done
forced=0
if test -n "$pids"; then
    forced=1
    printf 'forced_cleanup_pids=%s\n' "$pids" >&2
    kill -KILL $pids 2>/dev/null || true
    sleep 1
fi
test -z "$(owned "$1")"
rm -rf -- "$1"
test ! -e "$1"
printf 'fixture_removed; live_fixture_processes=0\n'
test "$forced" = 0
"#,
        &[root],
    )
    .await
}

fn spec(root: &Path, script: &str) -> ProcessSpec {
    ProcessSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: root.into(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: true,
        stdin: vec![],
        max_output_bytes: Some(1024 * 1024),
        timeout_ms: Some(10_000),
    }
}

async fn command(backend: &WorkspaceBackendHandle, root: &Path, script: &str) -> Result<Vec<u8>> {
    let result = backend.run_process(spec(root, script)).await?;
    ensure!(
        result.success && !result.timed_out && !result.stdout_truncated && !result.stderr_truncated,
        "command {script:?}: {result:?}"
    );
    Ok(result.stdout)
}

fn process_request(root: &Path, program: &str, args: &[&str]) -> RemotePipedProcessSessionRequest {
    RemotePipedProcessSessionRequest {
        program: program.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        cwd: root.into(),
        environment_anchor: root.into(),
        env_overrides: BTreeMap::new(),
    }
}

async fn piped(
    launcher: &dyn RemotePipedProcessSessionLauncher,
    root: &Path,
    close: bool,
) -> Result<()> {
    let mut session = launcher.launch(process_request(
        root,
        "/bin/sh",
        &["-c", "printf pipe-out; printf pipe-err >&2; exit 7"],
    ))?;
    if close {
        let _ = session.stdin.shutdown().await;
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let (_, _, completion) = tokio::try_join!(
        async {
            session
                .stdout
                .read_to_end(&mut stdout)
                .await
                .map_err(anyhow::Error::from)
        },
        async {
            session
                .stderr
                .read_to_end(&mut stderr)
                .await
                .map_err(anyhow::Error::from)
        },
        async { session.completion.await.map_err(anyhow::Error::from) },
    )?;
    ensure!(
        stdout == b"pipe-out" && stderr == b"pipe-err" && completion.status_code == Some(7),
        "piped output/status mismatch"
    );
    Ok(())
}

async fn pty(launcher: &dyn RemotePtySessionLauncher, root: &Path, turn: u64) -> Result<()> {
    let mut session = launcher.launch(PtySessionRequest {
        command: PtySessionCommand::LoginShell {
            shell: Some("/bin/bash".into()),
        },
        cwd: root.into(),
        environment: ProjectEnvironmentSelection::WorkspaceOrNearest {
            anchor: root.into(),
        },
        env_overrides: BTreeMap::from([("SOAK_VALUE".into(), format!("turn-{turn}"))]),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    session.control.resize(103, 31, 0, 0)?;
    let script = format!(
        "printf '\\nSOAK:%s:%s:%s\\n' \"$PWD\" \"$SOAK_VALUE\" \"$(tty)\"; stty size; exit 7\r"
    );
    session.input.write_all(script.as_bytes()).await?;
    session.input.flush().await?;
    let mut bytes = Vec::new();
    session.output.read_to_end(&mut bytes).await?;
    let done = session.completion.await?;
    let output = String::from_utf8_lossy(&bytes);
    ensure!(
        output.contains(&format!("SOAK:{}:turn-{turn}:/dev/pts/", root.display())),
        "PTY marker/cwd/env/tty missing: {output}"
    );
    ensure!(
        output.contains("31 103") && done.status_code == Some(7),
        "PTY resize/status mismatch: {output}"
    );
    Ok(())
}

struct Lsp {
    session: RemotePipedProcessSessionParts,
    reader: BufReader<RemoteProcessSessionReader>,
    stderr: tokio::task::JoinHandle<std::io::Result<u64>>,
    next_id: u64,
}

impl Lsp {
    async fn send(&mut self, message: Value) -> Result<()> {
        let body = serde_json::to_vec(&message)?;
        self.session
            .stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await?;
        self.session.stdin.write_all(&body).await?;
        self.session.stdin.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Value> {
        let mut len = None;
        loop {
            let mut line = String::new();
            ensure!(self.reader.read_line(&mut line).await? > 0, "LSP EOF");
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                len = Some(value.trim().parse::<usize>()?);
            }
        }
        let len = len.context("LSP content length missing")?;
        ensure!(len <= 4 * 1024 * 1024, "LSP response exceeds 4 MiB");
        let mut body = vec![0; len];
        self.reader.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await?;
        loop {
            let message = self.read().await?;
            if message["id"] == id {
                ensure!(message.get("error").is_none(), "LSP {method}: {message}");
                return Ok(message["result"].clone());
            }
        }
    }

    async fn start(
        launcher: &dyn RemotePipedProcessSessionLauncher,
        root: &Path,
        log: Log,
    ) -> Result<Self> {
        let mut session = launcher.launch(process_request(
            root,
            "clangd",
            &["--background-index=false", "--log=error"],
        ))?;
        let stdout = std::mem::replace(&mut session.stdout, Box::pin(tokio::io::empty()));
        let mut stderr = std::mem::replace(&mut session.stderr, Box::pin(tokio::io::empty()));
        let stderr = tokio::spawn(async move {
            let mut buffer = [0; 4096];
            let mut total = 0;
            loop {
                let n = stderr.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                log.emit(
                    "clangd.stderr",
                    json!({"text":String::from_utf8_lossy(&buffer[..n])}),
                );
            }
            Ok(total)
        });
        let mut lsp = Self {
            session,
            reader: BufReader::new(stdout),
            stderr,
            next_id: 0,
        };
        let result = lsp.request("initialize", json!({"processId":null, "rootUri":format!("file://{}", root.display()), "capabilities":{}})).await?;
        ensure!(
            result["capabilities"].is_object(),
            "clangd capabilities missing"
        );
        lsp.notify("initialized", json!({})).await?;
        Ok(lsp)
    }

    async fn exercise(&mut self, root: &Path, turn: u64) -> Result<()> {
        let uri = format!("file://{}/main.cpp", root.display());
        let text = "int add(int a, int b) { return a + b; }\nint main() { return add(2, 3); }\n";
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"cpp","version":1,"text":text}}),
        )
        .await?;
        let symbols = self
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument":{"uri":uri}}),
            )
            .await?;
        let names: Vec<_> = symbols
            .as_array()
            .context("symbols not array")?
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        ensure!(
            names.contains(&"add") && names.contains(&"main"),
            "clangd symbols: {symbols}"
        );
        let at = json!({"textDocument":{"uri":uri}, "position":{"line":1,"character":21}});
        let definition = self.request("textDocument/definition", at.clone()).await?;
        ensure!(
            definition.to_string().contains("main.cpp")
                && definition.to_string().contains("\"line\":0"),
            "clangd definition: {definition}"
        );
        let hover = self.request("textDocument/hover", at.clone()).await?;
        ensure!(hover.to_string().contains("add"), "clangd hover: {hover}");
        let completion = self
            .request(
                "textDocument/completion",
                json!({"textDocument":{"uri":uri},"position":{"line":1,"character":22}}),
            )
            .await?;
        ensure!(
            completion.to_string().contains("add"),
            "clangd completion: {completion}"
        );
        let mut rename_at = at;
        rename_at["newName"] = json!(format!("sum_{turn}"));
        let rename = self.request("textDocument/rename", rename_at).await?;
        ensure!(
            rename.to_string().contains(&format!("sum_{turn}")),
            "clangd rename: {rename}"
        );
        let format = self
            .request(
                "textDocument/formatting",
                json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}}),
            )
            .await?;
        ensure!(format.is_array(), "clangd formatting: {format}");
        self.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"int main() { return missing_soak_symbol; }\n"}]})).await?;
        loop {
            let message = self.read().await?;
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
                && message["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(|a| {
                        a.iter().any(|d| {
                            d["message"]
                                .as_str()
                                .is_some_and(|m| m.contains("missing_soak_symbol"))
                        })
                    })
            {
                break;
            }
        }
        self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await?;
        Ok(())
    }

    async fn close(mut self) -> Result<()> {
        self.request("shutdown", Value::Null).await?;
        self.notify("exit", Value::Null).await?;
        let done = self.session.completion.await?;
        ensure!(done.status_code == Some(0), "clangd exit: {done:?}");
        self.stderr.await??;
        Ok(())
    }
}

async fn files(
    backend: &WorkspaceBackendHandle,
    display: &Path,
    turn: u64,
    inject: bool,
) -> Result<()> {
    let dir = display.join("work");
    backend.create_dir(&dir).await?;
    let path = dir.join("space \u{00e9}\u{96ea}.txt");
    backend.create_file(&path).await?;
    let content = format!("needle_{turn}\nUnicode: \u{00e9} \u{96ea}\r\n");
    backend
        .write_file(&path, content.as_bytes(), WriteOptions::default())
        .await?;
    if inject {
        backend
            .write_file(
                &path,
                b"deliberately corrupted fixture",
                WriteOptions::default(),
            )
            .await?;
    }
    let read = backend.read_file(&path, ReadOptions::default()).await?;
    ensure!(
        read.bytes == content.as_bytes(),
        "saved content differs (negative control if --inject-failure)"
    );
    let old = read.version.context("file version missing")?;
    let updated = format!("{content}edited\n");
    backend
        .write_file(
            &path,
            updated.as_bytes(),
            WriteOptions {
                expected_version: Some(old.clone()),
                ..Default::default()
            },
        )
        .await?;
    ensure!(
        backend
            .write_file(
                &path,
                b"stale overwrite",
                WriteOptions {
                    expected_version: Some(old),
                    ..Default::default()
                }
            )
            .await
            .is_err(),
        "stale save was accepted"
    );
    let streamed = backend
        .read_file_stream(&path, ReadOptions::default())
        .await?
        .collect_file(&path)
        .await?;
    ensure!(
        streamed.bytes == updated.as_bytes(),
        "streamed file content differs"
    );
    let renamed = dir.join("renamed.txt");
    let copied = dir.join("copied.txt");
    backend.rename_path(&path, &renamed).await?;
    backend.copy_path(&renamed, &copied).await?;
    ensure!(backend.stat(&path).await.is_err(), "rename left old path");
    ensure!(
        backend
            .read_file(&copied, ReadOptions::default())
            .await?
            .bytes
            == updated.as_bytes(),
        "copy differs"
    );
    let listings = backend.list_dirs(vec![display.into(), dir.clone()]).await;
    ensure!(listings.len() == 2, "batch listing length");
    for (path, listing) in listings {
        ensure!(
            !listing
                .with_context(|| format!("listing {path:?}"))?
                .entries
                .is_empty(),
            "empty tree"
        );
    }
    let listing = backend.list_dir(&dir).await?;
    ensure!(
        listing.entries.len() == 2 && listing.entries.iter().any(|e| e.name == "copied.txt"),
        "tree after mutations: {listing:?}"
    );
    ensure!(
        backend
            .find_ancestor_file(&dir, ".gitignore", 3)
            .await?
            .is_some(),
        "ancestor lookup"
    );
    backend.delete_path(&renamed).await?;
    backend.delete_path(&copied).await?;
    ensure!(
        backend.list_dir(&dir).await?.entries.is_empty(),
        "delete left tree entries"
    );
    backend.delete_path(&dir).await?;
    Ok(())
}

async fn searches(backend: &WorkspaceBackendHandle, display: &Path) -> Result<()> {
    let query = FileSearchQuery {
        root: display.into(),
        parents: false,
        git_global: false,
        ..Default::default()
    };
    let result = backend
        .file_search_stream(query.clone())
        .await?
        .collect_search(display)
        .await?;
    ensure!(
        result.files.contains(&PathBuf::from("main.cpp")),
        "file picker misses main.cpp: {result:?}"
    );
    ensure!(
        !result
            .files
            .iter()
            .any(|p| p.to_string_lossy().contains("ignored")
                || p.to_string_lossy().contains(".hidden")),
        "ignore/hidden leak: {result:?}"
    );
    let all = backend
        .file_search(FileSearchQuery {
            hidden: true,
            git_ignore: false,
            ignore: false,
            excluded_relative_prefixes: vec![".git".into()],
            ..query
        })
        .await?;
    ensure!(
        all.files.contains(&PathBuf::from("ignored/secret.txt"))
            && all.files.contains(&PathBuf::from(".hidden.txt")),
        "explicit ignore override: {all:?}"
    );
    let text = TextSearchQuery {
        root: display.into(),
        pattern: "SOAK_SEARCH".into(),
        parents: false,
        git_global: false,
        ..Default::default()
    };
    let found = backend
        .text_search_stream(text.clone())
        .await?
        .collect_search(display)
        .await?;
    ensure!(
        found.matches.len() == 1
            && found.matches[0].relative_path == Path::new("visible.txt")
            && found.matches[0].line_number == 2,
        "text search oracle: {found:?}"
    );
    let limit = backend
        .text_search(TextSearchQuery {
            hidden: true,
            git_ignore: false,
            ignore: false,
            limit: 1,
            excluded_relative_paths: vec![".git".into()],
            ..text.clone()
        })
        .await?;
    ensure!(
        limit.matches.len() == 1 && limit.truncated,
        "search limit: {limit:?}"
    );
    let cancellation = WorkspaceCancellationToken::new();
    cancellation.cancel();
    ensure!(
        matches!(
            backend
                .text_search_stream_with_cancellation(text, &cancellation)
                .await,
            Err(WorkspaceError::Cancelled { .. })
        ),
        "pre-cancelled search not cancelled"
    );
    Ok(())
}

async fn cycle(options: &Options, root: &str, started: Instant) -> Result<u64> {
    let native = PathBuf::from(root);
    let display = PathBuf::from(format!(
        "\\\\wsl.localhost\\{}{}",
        options.distro,
        root.replace('/', "\\")
    ));
    let mut connection = None;
    let mut lsp = None;
    let mut completed = 0;
    while started.elapsed().as_secs() < options.seconds && completed < options.iterations {
        if completed % options.iterations_per_connection == 0 {
            if let Some(lsp) = lsp.take() {
                options.log.step("clangd.shutdown", Lsp::close(lsp)).await?;
            }
            drop(connection.take());
            let location = classify_workspace_location(&display);
            let backend_options = RemoteWorkspaceBackendOptions {
                remote_helper_path: options.helper.clone(),
                remote_helper_path_is_override: true,
                wsl_helper_install_policy: RemoteHelperInstallPolicy::Never,
                ..Default::default()
            };
            connection = Some(
                options
                    .log
                    .step("connect", async {
                        tokio::task::spawn_blocking(move || {
                            connect_workspace_backend_for_location(location, &backend_options)
                        })
                        .await?
                    })
                    .await?,
            );
            let c = connection.as_ref().unwrap();
            options.log.emit(
                "connection",
                json!({"iteration":completed, "hello":c.hello}),
            );
            ensure!(
                c.pty_session_launcher.is_some() && c.process_session_launcher.is_some(),
                "helper lacks required process/PTY capabilities"
            );
            let clangd = command(&c.backend, &native, "command -v clangd || true").await?;
            if !clangd.is_empty() {
                lsp = Some(
                    options
                        .log
                        .step(
                            "clangd.initialize",
                            Lsp::start(
                                c.process_session_launcher.as_ref().unwrap().as_ref(),
                                &native,
                                options.log.clone(),
                            ),
                        )
                        .await?,
                );
            } else {
                options.log.emit("skip", json!({"feature":"LSP", "reason":"clangd not installed; required coverage unavailable"}));
                bail!("clangd is required for this soak; no installation attempted");
            }
        }
        let c = connection.as_ref().unwrap();
        let b = &c.backend;
        let turn = options.seed.wrapping_add(completed);
        options.log.emit(
            "iteration",
            json!({"iteration":completed, "turn":turn, "elapsed_ms":started.elapsed().as_millis()}),
        );
        let watch = options
            .log
            .step("watch.start", async {
                Ok(
                    b.start_watch(WorkspaceWatchRequest::expanded_dirs([display.clone()]))
                        .await?,
                )
            })
            .await?;
        let mixed = options
            .log
            .step("mixed.concurrent", async {
                tokio::try_join!(
                    files(b, &display, turn, options.inject_failure && completed == 0),
                    searches(b, &display),
                    piped(
                        c.process_session_launcher.as_ref().unwrap().as_ref(),
                        &native,
                        completed % 2 == 0
                    ),
                    pty(
                        c.pty_session_launcher.as_ref().unwrap().as_ref(),
                        &native,
                        turn
                    ),
                )?;
                Ok(())
            })
            .await;
        if mixed.is_err() {
            if let Some(watch) = &watch {
                options
                    .log
                    .step("watch.stop_after_failure", async {
                        Ok(b.stop_watch(watch.watch_id).await?)
                    })
                    .await?;
            }
            mixed?;
        }
        if let Some(watch) = watch {
            let observed = options
                .log
                .step("watch.events", async {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    let mut observed = false;
                    while Instant::now() < deadline {
                        if let Some(batch) = watch.try_recv().ok() {
                            ensure!(
                                !batch.overflow && !batch.resync_required,
                                "watch overflow/resync in small fixture: {batch:?}"
                            );
                            if batch
                                .events
                                .iter()
                                .any(|e| e.path.to_string_lossy().contains("work"))
                            {
                                observed = true;
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    ensure!(observed, "watch missed tree mutations");
                    Ok(())
                })
                .await;
            options
                .log
                .step("watch.stop", async {
                    Ok(b.stop_watch(watch.watch_id).await?)
                })
                .await?;
            observed?;
        } else {
            options.log.emit(
                "skip",
                json!({"feature":"watch", "reason":"backend returned unsupported"}),
            );
        }
        if let Some(lsp) = lsp.as_mut() {
            options
                .log
                .step(
                    "clangd.requests_and_diagnostics",
                    lsp.exercise(&native, turn),
                )
                .await?;
        }
        options
            .log
            .step("git_and_environment", async {
                let head = b.git_head(&display).await?;
                ensure!(head.head.is_some(), "git HEAD missing");
                b.write_file(
                    &display.join("tracked.txt"),
                    format!("edited-{turn}\n").as_bytes(),
                    WriteOptions::default(),
                )
                .await?;
                let status = b.git_status(&display, GitStatusOptions::default()).await?;
                ensure!(
                    status
                        .entries
                        .iter()
                        .any(|e| e.relative_path == Path::new("tracked.txt")
                            && e.working_tree_status == GitStatusKind::Modified),
                    "VCS modified status: {status:?}"
                );
                ensure!(
                    command(b, &native, "git show HEAD:tracked.txt").await? == b"baseline\n",
                    "VCS diff base differs"
                );
                let env = b.project_environment(&display).await?;
                ensure!(
                    env.variables.contains_key("PATH"),
                    "project environment lacks PATH"
                );
                Ok(())
            })
            .await?;
        options
            .log
            .step("cancellation_and_liveness", async {
                let cancellation = WorkspaceCancellationToken::new();
                let mut result = b
                    .run_process_stream_with_cancellation(
                        spec(&native, "printf '%s' $$ > cancel.pid; printf cancel-ready; exec sleep 20"),
                        &cancellation,
                    )
                    .await?;
                let mut ready = Vec::new();
                while !String::from_utf8_lossy(&ready).contains("cancel-ready") {
                    match result.next().await.context("cancellable process EOF")?? {
                        ProcessEvent::Stdout(bytes) => ready.extend(bytes),
                        ProcessEvent::Complete(_) => bail!("process completed before cancellation"),
                        _ => {}
                    }
                }
                cancellation.cancel();
                ensure!(
                    result.collect_output(&native).await.is_err(),
                    "in-flight process cancellation did not fail"
                );
                let mut session =
                    c.process_session_launcher
                        .as_ref()
                        .unwrap()
                        .launch(process_request(
                            &native,
                            "/bin/sh",
                            &["-c", "printf '%s' $$ > drop.pid; printf drop-ready; exec sleep 20"],
                        ))?;
                let mut ready = [0; 10];
                session.stdout.read_exact(&mut ready).await?;
                ensure!(&ready == b"drop-ready", "dropped process readiness");
                drop(session);
                let mut timed = spec(&native, "printf '%s' $$ > timeout.pid; exec sleep 20");
                timed.timeout_ms = Some(100);
                let output = b.run_process(timed).await?;
                ensure!(output.timed_out && !output.success, "process timeout: {output:?}");
                command(b, &native, "set -eu; for pid in $(cat cancel.pid drop.pid timeout.pid); do for n in 1 2 3 4 5 6 7 8 9 10; do test ! -e /proc/$pid && break; sleep 0.1; done; test ! -e /proc/$pid; done").await?;
                ensure!(
                    b.stat(&display.join("main.cpp")).await?.size > 0,
                    "connection not live after cancellation"
                );
                Ok(())
            })
            .await?;
        if completed % 10 == 0 {
            let data = options.log.step("resources", async {
                let mut request = spec(&native,
                    "set -eu; p=$PPID; exe=$(readlink /proc/$p/exe); cwd=$(readlink /proc/$p/cwd); test \"$exe\" = \"$SOAK_HELPER\"; test \"$cwd\" = \"$SOAK_ROOT\"; printf 'helper_pid=%s\\nhelper_exe=%s\\nhelper_cwd=%s\\n' \"$p\" \"$exe\" \"$cwd\"; grep -E '^(VmRSS|Threads):' /proc/$p/status; printf 'fds='; ls /proc/$p/fd | wc -l; printf 'session_processes\\n'; ps -eo pid,ppid,rss,stat,comm | grep -E 'clangd|nucleotide|sleep|bash' || true");
                request.env.insert("SOAK_HELPER".into(), options.helper.to_str().context("helper encoding")?.into());
                request.env.insert("SOAK_ROOT".into(), root.into());
                let output = b.run_process(request).await?;
                ensure!(output.success && !output.timed_out, "resource PID verification: {output:?}");
                Ok(output.stdout)
            }).await?;
            options.log.emit(
                "resources",
                json!({"iteration":completed,"remote":String::from_utf8_lossy(&data)}),
            );
        }
        if (completed + 1) % options.iterations_per_connection == 0 {
            if let Some(lsp) = lsp.take() {
                options.log.step("clangd.shutdown", lsp.close()).await?;
            }
            options.log.step("transport_recovery", async {
                let pid = String::from_utf8(command(b, &native, "printf '%s' $PPID").await?)?;
                pid.parse::<u32>()?;
                // Verify both executable and unique cwd before signalling only
                // this run's helper, never a distribution or another editor.
                wsl(&options.distro, "set -eu\ntest \"$(readlink /proc/$1/exe)\" = \"$2\"\ntest \"$(readlink /proc/$1/cwd)\" = \"$3\"\nkill -TERM \"$1\"\n", &[&pid, options.helper.to_str().context("helper encoding")?, root]).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
                ensure!(b.stat(&display.join("main.cpp")).await?.size > 0, "backend did not recover helper loss");
                Ok(())
            }).await?;
        }
        completed += 1;
        options.log.emit(
            "iteration_pass",
            json!({"completed":completed,"elapsed_ms":started.elapsed().as_millis()}),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if let Some(lsp) = lsp {
        options.log.step("clangd.shutdown", lsp.close()).await?;
    }
    drop(connection);
    Ok(completed)
}

fn main() -> Result<()> {
    ensure!(
        cfg!(windows),
        "this probe must run as a native Windows client"
    );
    let options = options()?;
    nucleotide_logging::init_stderr_logging(nucleotide_logging::LoggingConfig::default())?;
    let log = options.log.clone();
    // Covers synchronous bootstrap and runtime shutdown too. Normal operations
    // have tighter step deadlines; this is a last-resort process watchdog.
    let watchdog_seconds = options.seconds + 180;
    let watchdog_log = log.clone();
    let fixture = Arc::new(Mutex::new(None::<String>));
    let watchdog_fixture = fixture.clone();
    let watchdog_distro = options.distro.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(watchdog_seconds));
        watchdog_log.emit(
            "fail",
            json!({"step":"watchdog","seconds":watchdog_seconds}),
        );
        if let Some(root) = watchdog_fixture.lock().unwrap().as_ref() {
            let result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(cleanup_fixture(&watchdog_distro, root));
            watchdog_log.emit("watchdog_cleanup", json!({"result":format!("{result:?}")}));
        }
        std::process::exit(2);
    });
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async {
        let root = wsl(&options.distro, "set -eu\nr=$(mktemp -d /tmp/nucleotide-soak-XXXXXXXX)\nprintf '%s' nucleotide-wsl-soak > \"$r/.soak-owner\"\nprintf '%s' \"$r\"\n", &[]).await?;
        ensure!(root.starts_with("/tmp/nucleotide-soak-") && root.len() < 80 && !root.contains(char::is_whitespace), "invalid fixture path");
        *fixture.lock().unwrap() = Some(root.clone());
        log.emit("start", json!({"root":root,"distro":options.distro,"helper":options.helper,"seconds":options.seconds,"max_iterations":options.iterations,"seed":options.seed,"iterations_per_connection":options.iterations_per_connection,"inject_failure":options.inject_failure,"client_pid":std::process::id()}));
        let setup = wsl(&options.distro, "set -eu\ncd \"$1\"\nmkdir ignored\nprintf 'ignored/\\n' > .gitignore\nprintf 'SOAK_SEARCH\\n' > ignored/secret.txt\nprintf 'SOAK_SEARCH\\n' > .hidden.txt\nprintf 'first line\\nSOAK_SEARCH visible\\n' > visible.txt\nprintf 'int add(int a, int b) { return a + b; }\\nint main() { return add(2, 3); }\\n' > main.cpp\nprintf 'baseline\\n' > tracked.txt\ngit -c init.defaultBranch=main init -q\ngit add .gitignore visible.txt main.cpp tracked.txt\ngit -c user.name=Soak -c user.email=soak.invalid -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -qm fixture\n", &[&root]).await;
        let started = Instant::now();
        let outcome = match setup { Ok(_) => cycle(&options, &root, started).await, Err(e) => Err(e) };
        // Only remove the unique, marked fixture allocated by this process.
        let cleanup = cleanup_fixture(&options.distro, &root).await;
        log.emit("cleanup", json!({"root":root,"ok":cleanup.is_ok(),"observation":cleanup.as_ref().ok(),"error":cleanup.as_ref().err().map(|e| format!("{e:#}"))}));
        log.emit("summary", json!({"ok":outcome.is_ok() && cleanup.is_ok(),"completed":outcome.as_ref().ok(),"elapsed_ms":started.elapsed().as_millis(),"error":outcome.as_ref().err().map(|e| format!("{e:#}"))}));
        cleanup?;
        outcome?;
        Ok(())
    });
    runtime.shutdown_timeout(Duration::from_secs(5));
    result
}
