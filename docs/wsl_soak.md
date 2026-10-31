# Windows and WSL soak testing

Run `scripts\wsl-soak.ps1` from native Windows to repeatedly exercise the real remote backend against a disposable Linux workspace. This is a backend reliability gate, not proof that every editor feature works. Use it alongside the full workspace test suite and the desktop checklist below.

## Prerequisites and safety

- A working Rust toolchain, Windows `wsl.exe`, an existing WSL distribution and an existing protocol-v5 `nucleotide-remote` helper that supports process and PTY sessions.
- `git`, `/bin/bash`, `clangd` and ordinary Linux utilities in the distribution. Missing clangd makes the run fail, with an explicit coverage-unavailable event; it is never silently counted as a pass.
- The script never installs tools, replaces a helper, changes shared configuration, stops a distribution or runs against a user project. It creates a marked `/tmp/nucleotide-soak-*` fixture and deletes only that fixture.
- At each connection epoch boundary it terminates **its own helper process**, after checking the exact executable and unique fixture cwd, then verifies automatic backend recovery. `-IterationsPerConnection` defaults to 200. It does not signal another editor's helper. clangd is shut down deliberately before this fault, not expected to survive helper loss.
- Fixture-local Git commits use command-specific author settings and disable signing and hooks. Your Git configuration is not changed.
- Before writing default review artifacts, exclude `/.amp/in/` in the repository-local `info/exclude` file (`git rev-parse --git-path info/exclude`). The script checks this rule. Keep logs: they contain fixture paths, process IDs and errors, but no user-project contents.

## Run it

Supply the existing helper's **Linux** path. First run a short smoke and a negative control, then a longer soak. Use a fresh output directory or let the timestamped prefix distinguish runs.

```powershell
.\scripts\wsl-soak.ps1 -Distro Ubuntu -HelperPath /absolute/path/to/nucleotide-remote -Minutes 1 -MaxIterations 25 -IterationsPerConnection 20
.\scripts\wsl-soak.ps1 -Distro Ubuntu -HelperPath /absolute/path/to/nucleotide-remote -Minutes 1 -MaxIterations 3 -InjectFailure
.\scripts\wsl-soak.ps1 -Distro Ubuntu -HelperPath /absolute/path/to/nucleotide-remote -Minutes 30
```

The negative control **must exit nonzero** with `saved content differs`; its cleanup should still report `live_fixture_processes=0`. It overwrites a file after saving, then runs the normal content assertion. A green negative control means the harness cannot be trusted.

Use both reconnect-heavy and sustained-connection runs. For reconnect pressure, use `-Minutes 30 -IterationsPerConnection 20`. For an overnight retained helper/LSP, use `-Minutes 480 -MaxIterations 100000 -IterationsPerConnection 100000 -Seed 104729`. With that epoch larger than the number completed, the same helper and clangd remain live for the whole run; reconnect recovery is intentionally not exercised. Duration and iteration limits are both upper bounds; the first reached stops new iterations. The seed determines reproducible fixture values, not a random scheduler. OS scheduling and fixture names remain nondeterministic.

To invoke the example directly after building it:

```powershell
cargo build --locked -p nucleotide-remote --example wsl_soak
.\target\debug\examples\wsl_soak.exe --run Ubuntu /absolute/path/to/nucleotide-remote 1800 100000 104729 200 results.jsonl
```

The JSONL file must not already exist. All normal assertion failures and timeouts exit nonzero. Each step has a 30-second deadline; process commands also carry server-side deadlines. A process watchdog covers synchronous bootstrap and shutdown after the requested duration plus 180 seconds. Watchdog cleanup has a further bounded WSL-command deadline. Abruptly killing the Windows executable, closing the runner or suspending the machine can prevent cleanup; retain the logged root and inspect only that marked fixture. Never use a distro-wide shutdown as cleanup.

## Evidence and interpretation

The PowerShell wrapper records the client hash, helper hash, checkout HEAD, dirty-file list, seed and limits. It runs an executable copy from a unique temporary directory to avoid Windows Cargo locking, then removes that directory in `finally`; build artifacts do not go in the review-artifact directory. JSONL records timestamped `begin`, `pass`, `fail`, connection, iteration, resource, cleanup and summary events. The console transcript retains remote-client diagnostics and clangd stderr. A run is successful only if its process exits zero **and** its final summary has `ok: true`. No final summary is an interrupted run, not a pass.

The helper's Linux RSS, thread count and file-descriptor count are sampled every 10 iterations. Each sample verifies the sampled PID's executable and cwd against the expected helper and unique fixture; a shell wrapper cannot silently stand in for the helper. Session processes are included for context. Compare samples at the same point within connection epochs: a flat graph across different helper PIDs is not proof against a long-lived leak. The wrapper also samples the actual Windows client's private bytes, working set, handle/thread counts and CPU time every 30 seconds into `*.windows-resources.jsonl`. Record system load when comparing latencies; a concurrent full build changes timing.

Cancellation tests wait for actual child readiness, cancel or drop the handle, and require the recorded Linux PID to disappear. They also exercise a server-side process timeout. Normal cleanup waits for live processes whose cwd is the unique fixture to disappear, verifies fixture removal and fails if forced process cleanup was needed. This does not detect escaped daemonized descendants, processes that changed cwd or every possible zombie; the fixture commands do not daemonize.

### Coverage matrix

| Area | Automated assertions | Remaining coverage |
| --- | --- | --- |
| Connections | Persistent multiplexed connections; configurable epoch length; fresh connections; loss and automatic recovery of the run-owned helper at epoch boundaries | SSH, installation/version upgrades, network loss, sleep/resume and other hosts |
| Files | Create, byte-exact read/save/edit, UTF-8 names with spaces, CRLF/Unicode content, stale-version rejection, streamed read, rename, copy and delete | Very large files, permissions, symlink escapes, filesystem exhaustion, editor undo/redo and save UI |
| Tree | Stat, single and batched listings, ancestor lookup, mutations reflected in listings; watch start/events/stop | Watch update/overflow/resync, file-tree widgets, expanded-directory polling and selection |
| Search | Streamed file/text search, known results and line numbers, `.gitignore`, hidden-file overrides, result limits, pre-cancelled search | Custom/global ignore files, huge result sets and picker rendering/navigation |
| VCS | HEAD exists; tracked modification appears in status; exact `git show HEAD:tracked.txt` bytes over backend process execution | Gutter rendering, staging/committing UI, merge conflicts, worktrees and all status kinds |
| Processes | Exact stdout/stderr and exit 7, input half-close and independently exiting children, in-flight cancellation, handle-drop cancellation, server timeout and PID disappearance | Task/runnable UI, debug adapters, daemonized descendants and build-tool families |
| PTY | Real bash login shell, immediate input, native cwd, environment override, `/dev/pts/`, resize to 103×31 and exit 7 | Windows ConPTY/fallback path, selection, clipboard, scrolling, keyboard/menu and panel lifecycle |
| LSP | Real retained clangd on the same connection as repeated PTYs; initialize/open/change/close/shutdown, symbols, definition, hover, completion, rename edits, formatting response and planted-error diagnostics | Helix attachment, URI conversion in UI, applying edits, diagnostic rendering and other language servers |
| Project environment | PATH present and command-specific terminal variable reaches child | Nix/devenv/direnv activation, environment refresh and missing-tool UI |
| Resources | Verified helper RSS/threads/fds; Windows client private bytes, working set, handles, threads and CPU; child PID cleanup and post-cancellation/backend liveness | Exhaustive leak detection, performance acceptance budgets and multi-window ownership |

Watch support is optional: unsupported watches produce explicit `skip` events, not fake event coverage. Process sessions, PTYs and real clangd are required. This matrix describes harness intent; use the actual run's log to establish what executed.

## Lifecycle regression checks

The soak stops watches explicitly. Run the watch lifecycle regressions as well:

```sh
cargo test --locked -p nucleotide-remote --test v5_loopback_fixtures
cargo test --locked -p nucleotide --lib remote_watch_is_disposed_when_disabled_or_view_closes
```

Dropping the final `WorkspaceWatch` consumer now triggers asynchronous subscription cleanup, even on a quiet root or through a display/native path adapter. Dropping one clone leaves the others usable. Restored watches clean up on the transport that owns their physical ID. File-tree receivers also dispose of their watches when watching is disabled or the view closes, including late watch-start responses.

## Desktop checklist before updating a work machine

Run this in an interactive Windows session using an isolated fixture and the exact candidate build. Don't redirect input into an existing editor or capture private projects. A service-session executor cannot drive an RDP desktop merely because an RDP user is logged in.

1. Open the WSL fixture through both the remote prompt and a Windows UNC path. Confirm the remote indicator, cwd and Linux tools. Open a second window and verify closing one doesn't break the other.
2. Edit, undo/redo, split panes, switch tabs, save, close and reopen. Include Unicode/spaces, CRLF and a larger source file. Confirm saved bytes independently in WSL.
3. Create, rename, copy and delete files/directories through the tree. Make an external WSL change while a directory is expanded. Check refresh, selection and focus.
4. Use file/text pickers with ignored and hidden paths. Navigate results with keyboard and mouse; cancel a search while a language server and terminal stay live.
5. In C++, verify diagnostic navigation, completion, hover, definition, references, symbols, formatting and rename. Apply the edits and confirm their bytes. Restart the LSP and reopen documents. Check for duplicate attachment or stale diagnostics.
6. Keep the LSP active while opening and exiting at least 20 terminals. Use the shortcut and menu, resize, scroll, select and copy/paste. Keep several terminals open together; close one while output is arriving. Confirm another terminal and LSP still work.
7. Run the project's actual build/test command from the terminal. Cancel a task and check that terminals, LSP and file operations survive. Check VCS status and diff gutters before and after saves.
8. Reconnect the remote workspace. Check error messages, unsaved buffers, watcher recovery and that dead terminals aren't silently replayed. Test a missing-tool command deliberately.
9. Change theme and window size, reopen the app and restore the project. Check menus, focus and session persistence. Leave the candidate running while doing representative editing, then inspect application/helper logs for unexplained errors.

Keep the previous working editor build available. A passing finite soak increases confidence; it cannot establish that all bugs are gone or that an untested work environment is safe.
