# WSL Remote Development

Nucleotide can open projects through Windows WSL UNC paths such as
`\\wsl.localhost\Ubuntu\home\iain\project`. For these workspaces the editor UI
continues to run on Windows, while project tools should run inside the WSL
distribution that owns the files.

## Product Model

The target experience follows the same broad shape used by VS Code Remote WSL,
Zed remote development, and JetBrains remote development:

- Keep rendering and input local so the editor feels native.
- Run language servers, terminals, project scanning, and workspace commands where
  the project files live.
- Install or discover a small remote helper by version, then reuse it from a
  remote cache instead of doing expensive setup on every window open.
- Translate paths at the client boundary so Windows UI code sees WSL UNC paths
  while remote tools see Linux paths.
- Treat remote support as project-local state, not a global mode that changes
  native Windows projects.

## Current Implementation

The initial WSL path supports WSL language servers without moving the full
editor backend into Linux:

- `nucleotide-env` detects WSL UNC roots and converts them to distro plus Linux
  path metadata.
- `ProjectEnvironment` captures project environment through `wsl.exe` for WSL
  roots and tags it with `NUCLEOTIDE_REMOTE_KIND=wsl`. These snapshots stay
  scoped to WSL launch paths instead of being applied to the Windows host
  process environment.
- WSL workspaces force project LSP startup with fallback enabled, because
  project-level startup gives us one remote command boundary per language server.
- `nucleotide-lsp-proxy` maps file URIs between Windows WSL UNC URLs and Linux
  file URLs in both directions.
- `HelixLspBridge` launches WSL language servers through the proxy via `wsl.exe`
  and keeps the Windows editor side talking normal LSP. WSL LSP startup does
  not inject Linux environment snapshots into the Windows process; the remote
  launch boundary owns those values.
- `nucleotide-remote` is a versioned helper binary with `hello`, `env`,
  `metadata`, `list`, `files`, `search`, and `read` protocol commands. Metadata
  includes workspace marker and shallow source-directory facts so project/LSP
  detection can avoid repeated Windows UNC filesystem probes when the helper is
  available.
  Directory listing returns compact file metadata from inside WSL so the project
  tree can populate rows without enumerating `\\wsl...` paths through Windows.
  Recursive file search returns picker-ready relative paths from inside WSL.
  Global text search walks ignore-filtered files and reads file contents inside
  WSL, returning relative path, line number, and line text matches to the
  Windows UI.
  The `list`, `files`, `search`, and `read` commands are part of helper protocol
  version 5, so older cached helpers are bypassed by the versioned cache path.
- Application startup schedules a short, non-blocking WSL helper health probe for
  WSL roots. Probes prefer
  `~/.cache/nucleotide/remote-helper/<protocol-version>/nucleotide-remote`
  before falling back to `nucleotide-remote` on `PATH`. Helper success is logged;
  helper failure can bootstrap from `NUCLEOTIDE_REMOTE_HELPER_INSTALL_SOURCE`
  when that variable points at a Linux helper binary, then falls back to direct
  WSL language server launch if the helper remains unavailable. Helper and
  environment commands use a portable `/bin/sh -c` wrapper that re-enters the
  user's login shell when available, so common user PATH setup is preserved
  without relying on non-portable `sh -l` behavior.
- Workspace terminals and runnable commands opened from WSL roots are launched
  through `wsl.exe --distribution <distro> --cd <linux-path>`, so shells and
  commands start where the project files live.
- The file tree uses the remote helper for WSL initial root population,
  directory expansion, and directory refresh when a Tokio runtime is available.
  Native workspaces keep the existing local filesystem path. WSL file watching is
  disabled for the Windows watcher path so the UI does not pay for recursive UNC
  monitoring.
- Local path completion uses the same helper-backed directory listing for WSL
  paths with a short timeout, avoiding per-keystroke `\\wsl...` directory reads
  from the Windows side.
- Command-prompt file and directory completions also use helper-backed WSL
  directory listing, preserving native prompt behavior without walking UNC paths
  from Windows.
- The file picker uses helper-backed recursive file search for WSL roots, so the
  expensive ignore-aware walk runs in Linux rather than across the Windows UNC
  filesystem boundary.
- File picker previews use the helper-backed `read` command for WSL file paths,
  so previewing a selected file reads a bounded text slice inside WSL instead of
  through the Windows UNC filesystem path.
- Git status and repository HEAD checks run through `wsl.exe` for WSL roots, so
  file decorations and VCS events use Linux Git against local Linux paths while
  still mapping results back to Windows WSL UNC paths for the UI.
- Manifest-based project detection uses a WSL manifest delegate for WSL UNC
  paths. Marker checks and manifest reads run inside the distro with a
  per-detection existence cache, while native paths keep the normal filesystem
  delegate.
- Project type indicators use the helper-backed WSL directory listing when it is
  available, turning several synchronous marker-file checks into one Linux-side
  directory read.
- Global text search uses the helper-backed `search` command for WSL roots and
  merges those disk results with open editor buffers on the Windows side so
  unsaved edits remain authoritative.

This means the first supported path is direct WSL LSP execution with path
translation. The helper is currently an optional foundation for richer remote
services rather than a hard dependency.

## Runtime Flow

```mermaid
flowchart LR
    UI["Windows Nucleotide UI"]
    Bridge["HelixLspBridge"]
    Proxy["nucleotide-lsp-proxy on Windows"]
    WSL["wsl.exe"]
    Server["Language server in WSL"]
    Helper["nucleotide-remote in WSL"]
    FileTree["Project tree"]
    Picker["Picker, search, and completions"]
    Terminal["Terminal or runnable in WSL"]

    UI --> Bridge
    Bridge --> Proxy
    Proxy --> WSL
    WSL --> Server
    UI -. "background health probe" .-> Helper
    FileTree --> Helper
    Picker --> Helper
    UI --> Terminal
    Proxy <--> Server
```

The proxy is deliberately the compatibility layer. It keeps existing editor and
Helix integration code mostly native while only translating the file URI shapes
that cross the process boundary.

## Remote Helper Direction

The next step toward a more native-feeling remote experience is to make
`nucleotide-remote` self-managing:

1. Resolve a per-distro helper path such as
   `~/.cache/nucleotide/remote-helper/<protocol-version>/nucleotide-remote`.
2. Probe that exact path before falling back to `PATH`. This is implemented for
   helper health, environment snapshot, and workspace metadata commands.
3. Bootstrap or update the helper when the cached binary is missing or reports a
   protocol mismatch. The first bootstrap path is explicit via
   `NUCLEOTIDE_REMOTE_HELPER_INSTALL_SOURCE`, because WSL must receive a Linux
   helper binary rather than the Windows GUI binary.
4. Move remote services behind helper commands where that improves latency or
   correctness, starting with environment, workspace metadata, directory listing,
   and file search.
5. Keep direct WSL LSP launch as the fallback path so helper bootstrap problems
   do not block editing.

## References

- VS Code Remote WSL documentation:
  https://code.visualstudio.com/docs/remote/wsl
- Zed remote development documentation:
  https://zed.dev/docs/remote-development
- JetBrains remote development documentation:
  https://www.jetbrains.com/help/idea/remote-development-overview.html
