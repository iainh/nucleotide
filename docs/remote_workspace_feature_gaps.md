# Remote workspace feature gaps

This document tracks the remaining differences between local and remote workspaces after remote Language Server Protocol (LSP) support was made reliable. It is an implementation checklist, not a replacement for the remote workspace reference design.

Remote workspaces currently mean SSH targets and WSL paths routed through `nucleotide-remote`. Local workspaces use `LocalWorkspaceBackend` directly on the host filesystem.

## Current baseline

- Remote projects can be opened from the command line with remote paths and from the File > Open Remote prompt.
- The remote prompt supports saved project names, recent project completions, `save <name> <target>`, `forget <name>`, `reconnect` and `cancel`. The File menu also exposes Reconnect Remote and Cancel Remote Connection.
- `nucleotide-remote serve` handles the main workspace backend operations: stat, directory listing, batched directory listing, ancestor lookup, create, rename, delete, copy, read, write, file search, text search, project environment capture, git head, git status and process execution.
- Remote LSPs run on the target through `nucleotide-remote lsp-proxy`. The client sends native remote `file://` URIs to the language server and maps returned remote paths back to display paths.
- Remote terminals route through `nucleotide-remote terminal-proxy` and load the project environment on the target.
- SSH helper deployment can auto-upload or remotely download Linux `nucleotide-remote` helpers for `x86_64` and `aarch64`. WSL currently uses the configured helper path inside the distro.
- Remote file trees use backend directory loading and poll expanded directories for changes. Local file trees use host filesystem notifications.

## Gap summary

| Area | Local behaviour | Remote status | Remaining gap |
| --- | --- | --- | --- |
| Connection UI | Native file and directory open flows use host platform pickers. | Remote open uses a menu action, a `remote:` prompt, saved/recent completions, reconnect/cancel actions and progress notifications. | No connection browser, per-host health/status view or dedicated credential guidance UI. |
| Helper deployment | Local projects need no helper binary. | SSH can install bundled Linux helpers; WSL uses `nucleotide-remote` from the distro path. | No WSL auto-install, no non-Linux remote helper targets, no helper signature/checksum verification and no explicit compatibility policy for older helpers. |
| Transport concurrency | Local filesystem calls can run independently through host APIs. | Workspace requests share the remote service transport for a workspace. LSP and terminal proxies use separate remote commands. | Filesystem/search/git requests are not multiplexed over multiple in-flight channels, and long requests have no user-visible cancellation. |
| Service lifetime | Local workspaces do not start a service process. | Each remote workspace owns a helper service; LSP and terminal sessions start their own helper commands. | There is no long-running per-host `nucleotide-remote` daemon shared across windows or projects, so we still pay startup and environment costs per connection path. |
| File tree updates | Local file tree changes arrive through `notify`. | Remote file trees poll expanded directories every 2 seconds, backing off to 16 seconds while idle. | Updates outside expanded directories are not observed until the user expands or refreshes them, and remote changes are delayed compared with local notifications. |
| File metadata and large files | Local reads can rely on direct filesystem metadata and streaming options from the host. | Remote reads return metadata and bytes in protocol frames, with absolute read-only paths allowed for LSP navigation. | Very large file handling is bounded by the remote frame limit, and remote external paths support read-only opening but not full stat, write, rename, delete or directory browsing. |
| External dependency files | Local LSP results can open files anywhere the host can read. | Remote LSP results outside the workspace can be opened read-only by absolute native path. | External trees such as Cargo registry, git checkouts and `/nix/store` are not first-class workspace roots, so file tree browsing and edits are intentionally unavailable there. |
| Workspace symbols | Local workspaces can fall back to syntax-based workspace symbol scanning. | Remote workspaces use LSP workspace symbols, or open-document syntax symbols when available. | The syntax fallback is disabled for remote workspaces because it would scan files through the transport one by one. Remote workspace symbols therefore depend on the language server. |
| Search parity | Local file and text search use `ignore` walking against the host filesystem. | Remote file and text search run on the target through the backend. | The UI still needs verification for all ignore inputs, custom ignore files and large-result behaviour across SSH and WSL. Search cancellation and progress are not as visible as local work. |
| VCS freshness | Local VCS indicators can be refreshed in response to filesystem activity. | Remote git head/status and diff-base reads go through the backend, and gutters work. | Remote refresh cadence depends on backend calls and file-tree polling; there is no remote filesystem event subscription to trigger instant VCS refresh. |
| Project environment | Local LSPs and tasks inherit a captured project environment. | Remote LSPs and terminals load the project environment on the target, including nix or direnv output when available. | Environment diagnostics are mostly log-based, not UI-based. There is no persistent devshell reuse, and missing remote tools still surface as language-server or process startup failures. |
| Tasks and processes | Local commands run directly with the captured environment. | Remote process execution exists through the workspace backend, and terminal proxy commands run on the target. | Task, runnable, build and test flows need end-to-end coverage to confirm every UI path chooses the remote backend and remote environment. |
| Debugging | Local debugging would use host process and debug-adapter assumptions. | No remote-specific debug adapter path is documented. | Remote debug adapter deployment, port forwarding, path mapping and process attach behaviour remain undesigned. |
| Multi-root workspaces | Local code can rely on host paths and Helix workspace assumptions. | Remote startup currently centres on one remote workspace root. | Multiple remote roots, mixed local plus remote roots, and cross-host workspaces do not have a clear model. |
| Local-through-remote parity | Local projects use `LocalWorkspaceBackend` directly. | The remote service protocol is tested with loopback transports, but local production workspaces do not run through `nucleotide-remote`. | We do not yet have one unified service-backed implementation for local and remote projects, so local/remote parity still relies on duplicated backend behaviour and tests. |
| Transport coverage | Local projects use direct filesystem APIs. SSH and WSL have production remote paths. | The protocol is framed over helper stdin/stdout launched through SSH or WSL. | There is no production transport for containers, devcontainers, TCP, Unix sockets or a daemon protocol. |
| Security policy | Local workspaces trust the host filesystem permissions. | Remote write operations are workspace-scoped. Absolute read-only paths are allowed so LSP navigation can open dependencies and toolchain source. | The external-read policy is not exposed in settings, and there is no UI explanation when a read-only external file cannot be edited. |
| Tests and fixtures | Local backend behaviour has many direct unit and integration tests. | Remote behaviour has loopback and protocol tests plus focused UI-path tests. | We still need higher-level SSH and WSL fixtures for file tree polling, LSP, VCS, search, terminals and helper deployment. |

## Priority gaps

1. Replace or supplement remote file-tree polling with helper-side file watching when the target supports it.
2. Add request cancellation and better progress for slow remote search, file tree load, git and project environment operations.
3. Decide whether local projects should optionally use a local `nucleotide-remote` service to keep local and remote backend behaviour aligned.
4. Design persistent remote helper lifetime, especially for SSH hosts where multiple windows, terminals and LSPs can share one warmed service.
5. Document and test nix devshell and direnv behaviour for remote LSPs, terminals and tasks.
6. Add integration coverage against real SSH and WSL targets before broadening helper deployment to more architectures or transports.

## Intentional differences

- Remote writes should remain workspace-scoped by default. External dependency and toolchain paths should stay read-only unless we add an explicit trust model.
- Remote workspace symbol fallback should not scan the whole tree through single-file reads. Use LSP workspace symbols or add a target-side symbol indexer instead.
- WSL projects should prefer files inside the Linux filesystem. Accessing Windows-mounted paths from WSL will not match native performance.
