# Remote connection manager design

Status: guided dialog implemented; exploratory wireframes retained below

This document describes the remote workspace connection manager. The current guided flow is summarized below; the original wireframes describe exploratory states and are not exact screenshots of the implementation.

The manager should preserve the current "just works" behaviour while replacing the command-style `remote:` prompt as the primary guided path. The prompt can remain as a quick entry path for users who already know the exact target.

## Current guided dialog

- **Choose host:** Enter a host or choose a saved, recent or discovered target. Source headings group suggestions, and an empty state explains how to start. `Connect` is the only primary action; it is disabled without a target and while a connection is in progress.
- **Choose folder:** After connecting, the host input is replaced by the remote directory tree. The connected host and exact remote folder path stay visible. `Open folder` is the only primary action.
- The shared `nucleotide-ui::Stepper` centres the two stages, fills current and completed indicators, and marks completion with a check and coloured connector. It is informational, not a way to skip connecting.
- `Back` returns to host selection without discarding the host or selected folder. Editing the host clears that folder selection.
- Saving is offered in the folder step. An exact target already in the saved list shows `Saved connection` instead of a redundant save checkbox. Selecting another folder on that host can still be saved separately.
- Windows offers an SSH/WSL selector. Linux and macOS show SSH guidance without a single-option dropdown.
- Connection progress and errors appear inline, with `Cancel` available in both steps.
- Connection groups have rounded borders and Phosphor monitor icons; source badges only distinguish SSH config entries from known hosts. The folder tree uses inset rounded selection highlights and subtle row separators.
- The header close button cancels through the same path as `Cancel` and Escape. In the host step, `Cancel` is on the left and `Connect` is on the right; the folder step keeps `Back` on the left and its other actions on the right.

## Goals

- Let users choose `SSH` or `WSL` from a protocol dropdown on Windows. On Linux and macOS, offer only `SSH`; Tab must not switch to WSL.
- Autocomplete the server field from the right source for the selected protocol.
- Connect far enough to discover the remote home directory.
- Show a directory browser rooted at the remote home directory.
- Let users choose a directory as the project root and open it as the workspace.
- Reuse saved and recent remote connections.
- Show connection progress and failures without requiring log inspection.

## Non-goals

- Do not design a new remote protocol in this document.
- Do not design remote helper deployment in detail.
- Do not add a full file manager for remote files.
- Do not load project-specific environment hooks while the user is only browsing for a workspace root.

## Entry points

- `File > Open Remote...` opens the manager.
- `File > Reconnect Remote` reopens the last successful remote workspace.
- The `remote:` prompt remains available for direct paths, saved names and power-user commands.

## Primary flow

1. The user opens the connection manager.
2. On Windows, the user selects `SSH` or `WSL`. Other platforms use SSH.
3. The server field autocompletes from known hosts, SSH config aliases, WSL distributions, saved connections and recent connections.
4. The user selects a server or distro and presses `Connect`.
5. Nucleotide connects, checks or starts `nucleotide-remote`, resolves the remote home directory and lists its child directories.
6. The user browses directories and selects the project root.
7. The user opens the selected directory as the workspace.

## Wireframe: initial state

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  Connect to                                                       |
|  +----------+  +----------------------------------------------+   |
|  | SSH   v  |  | Host, alias or saved connection              |   |
|  +----------+  +----------------------------------------------+   |
|                                                                   |
|  Suggestions                                                      |
|  +------------------------------------------------------------+   |
|  | work-linux                         saved  /home/me/work    |   |
|  | staging                           recent /srv/app          |   |
|  | devbox                            ssh config               |   |
|  | localhost                         known host               |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Connect ]                                      [ Cancel ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

The protocol dropdown changes the meaning of the server field. The suggestions list is filtered by the selected protocol and by the text typed into the field.

## Wireframe: SSH host autocomplete

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  Connect to                                                       |
|  +----------+  +----------------------------------------------+   |
|  | SSH   v  |  | dev                                          |   |
|  +----------+  +----------------------------------------------+   |
|                                                                   |
|  Matching SSH targets                                             |
|  +------------------------------------------------------------+   |
|  | devbox                    ~/.ssh/config alias               |   |
|  | dev.internal.example.com  ~/.ssh/known_hosts                |   |
|  | dev-arm                   recent /home/iheggie/projects     |   |
|  | dev-vm                    saved  /workspace/nucleotide      |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Path after connect                                               |
|  +------------------------------------------------------------+   |
|  | Browse remote home directory                                |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Connect ]                                      [ Cancel ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

SSH suggestions should favour saved and recent connections because they already include a project path. SSH config aliases are more useful than raw `known_hosts` entries because they preserve user intent, custom ports, jump hosts and identity configuration.

If `known_hosts` contains hashed hostnames, the manager should not try to reverse them. The user can still type any SSH target manually.

## Wireframe: WSL distro autocomplete

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  Connect to                                                       |
|  +----------+  +----------------------------------------------+   |
|  | WSL   v  |  | ubu                                          |   |
|  +----------+  +----------------------------------------------+   |
|                                                                   |
|  Matching WSL distributions                                       |
|  +------------------------------------------------------------+   |
|  | Ubuntu-24.04                     running                   |   |
|  | Ubuntu                           stopped                   |   |
|  | Ubuntu-22.04                     recent /home/me/src        |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Path after connect                                               |
|  +------------------------------------------------------------+   |
|  | Browse remote home directory                                |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Connect ]                                      [ Cancel ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

When WSL is unavailable on the host platform, the dropdown can still show `WSL`, but selecting it should show an unavailable state rather than an empty list.

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  Connect to                                                       |
|  +----------+  +----------------------------------------------+   |
|  | WSL   v  |  |                                              |   |
|  +----------+  +----------------------------------------------+   |
|                                                                   |
|  WSL is not available on this machine.                            |
|                                                                   |
|  Use SSH for remote Linux hosts, or open this manager from a       |
|  Windows host with WSL installed.                                 |
|                                                                   |
|                                                   [ Cancel ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

## Wireframe: connecting state

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  SSH  devbox                                                      |
|                                                                   |
|  Connecting                                                       |
|  +------------------------------------------------------------+   |
|  | [x] Connecting to SSH host                                  |   |
|  | [x] Detecting remote platform                               |   |
|  | [ ] Checking nucleotide-remote                              |   |
|  | [ ] Starting browse session                                 |   |
|  | [ ] Loading home directory                                  |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Details                                                          |
|  devbox:/home/iheggie                                             |
|                                                                   |
|                                                 [ Cancel ]        |
|                                                                   |
+-------------------------------------------------------------------+
```

The progress list should use the same deployment phases already reported by remote startup, plus a final "Loading home directory" phase for the manager.

## Wireframe: home directory browser

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  SSH  devbox                                      Connected       |
|                                                                   |
|  Location                                                         |
|  +------------------------------------------------------------+   |
|  | ~                                                          |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Directories                                                     |
|  +------------------------------------------------------------+   |
|  | > projects                                18 directories    |   |
|  | > src                                      6 directories    |   |
|  | > work                                    12 directories    |   |
|  | > .config                                  9 directories    |   |
|  | > .cache                                  14 directories    |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Selected workspace root                                          |
|  +------------------------------------------------------------+   |
|  | /home/iheggie/projects/nucleotide                         |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Back ]  [ New folder ]  [ Save this connection ] [ Open ]     |
|                                                                   |
+-------------------------------------------------------------------+
```

The browser starts at the remote home directory, not the filesystem root. Most users keep projects under `~/projects`, `~/src` or `~/work`, so this avoids unnecessary traversal.

Rows should represent directories only by default. A small "Show files" option can be added later if users need confirmation before opening a root.

## Wireframe: nested directory selection

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  WSL  Ubuntu-24.04                                Connected       |
|                                                                   |
|  Directories                                                      |
|  +------------------------------------------------------------+   |
|  | v iheggie                                                   |   |
|  |   v projects                                                |   |
|  |     > nucleotide                                            |   |
|  |     > zed                                                   |   |
|  |   > downloads                                               |   |
|  |   > tmp                                                     |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Workspace root                                                   |
|  +------------------------------------------------------------+   |
|  | /home/iheggie/projects/nucleotide                         |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|                                          [ Cancel ] [ Open ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

Selecting a directory row immediately updates the workspace root field. Pressing `Open` opens that selected root; clicking or pressing `Enter` toggles expansion, and left/right collapse or expand the selected directory.

## Wireframe: saved and recent connections

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  Saved and recent                 Connection                      |
|  +----------------------------+   +---------------------------+   |
|  | * nucleotide-linux         |   | Protocol  SSH v           |   |
|  |   ssh://devbox/...         |   | Server    devbox          |   |
|  |                            |   | Path      /home/me/...    |   |
|  |   catnap-wsl               |   |                           |   |
|  |   wsl://Ubuntu/...         |   | [ Connect ]               |   |
|  |                            |   +---------------------------+   |
|  |   staging                  |                                   |
|  |   ssh://staging/...        |                                   |
|  +----------------------------+                                   |
|                                                                   |
|  [ Forget ]                                      [ Cancel ]       |
|                                                                   |
+-------------------------------------------------------------------+
```

Saved and recent entries should be visible, but not required. Choosing one should populate protocol, server and path. If the saved path still exists, the manager can skip directly to the project root confirmation or open it immediately from `Reconnect Remote`.

## Wireframe: connection error

```text
+ Remote Connection -----------------------------------------------+
|                                                                   |
|  SSH  devbox                                                      |
|                                                                   |
|  Could not connect                                                |
|  +------------------------------------------------------------+   |
|  | Authentication failed for devbox.                           |   |
|  |                                                            |   |
|  | Nucleotide uses your existing SSH configuration. Confirm    |   |
|  | the host, key, agent and ProxyJump settings outside the     |   |
|  | app, then retry.                                           |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Retry ]  [ Edit target ]  [ Copy details ]     [ Cancel ]      |
|                                                                   |
+-------------------------------------------------------------------+
```

Errors should keep the selected protocol and server visible. The primary recovery path is `Retry` when the external condition changes, or `Edit target` when the input was wrong.

## Behaviour details

### Protocol switching

Changing the protocol should:

- Clear the current connection attempt and directory browser.
- Preserve typed text only when it still looks valid for the new protocol.
- Filter saved and recent entries by protocol.
- Change autocomplete labels and empty states.

### Server autocomplete

For `SSH`, suggestions should come from:

- Saved and recent remote connections.
- SSH config host aliases.
- Plain hostnames in `known_hosts`, when they are not hashed.
- Manual user input.

For `WSL`, suggestions should come from:

- Saved and recent WSL connections.
- Installed WSL distributions.
- The default distro, when the platform can identify one.

Each suggestion should show its source. Source labels help users distinguish a configured SSH alias from an incidental host key entry.

### Directory browsing

Browsing should use remote-side directory listing. The host must not probe `\\wsl$`, `\\wsl.localhost`, SSHFS mounts or other remote filesystem paths.

The browser needs a read-only browse session before the final workspace root is chosen. That session can be scoped to the remote home directory. When the user opens a workspace, Nucleotide should start the normal workspace backend rooted at the selected directory.

The browser should:

- List directories in one remote request per opened folder.
- Batch or prefetch visible child metadata when the transport can do it cheaply.
- Avoid recursive scanning before the user opens a folder.
- Keep stale responses from updating the UI after protocol, host or path changes.
- Show permission errors inline for individual directories.

### Opening the workspace

Opening should construct the same canonical target used by direct remote paths:

- `ssh://host/path` for SSH.
- A WSL display path for WSL, using the selected distro and Linux path.

After the workspace opens, the existing remote backend owns project detection, environment capture, Language Server Protocol startup, VCS, file tree, search and terminals.

### Save behaviour

The manager should keep the existing saved and recent model but move it from command syntax into UI controls:

- `Save this connection` stores protocol, server, path, display name and last-opened time.
- Recent entries record successful opens automatically.
- Saved entries can be renamed or removed from the manager.
- Existing string-only entries can be shown by parsing their target path, then migrated later.

### Keyboard behaviour

- On Windows, `Tab` switches protocols in the host step. On other platforms, it does not switch to WSL.
- `Up` and `Down` move through suggestions or directory rows.
- `Enter` connects to the selected or typed target in the host step, and opens the selected folder in the folder step.
- `Left` and `Right` collapse and expand directories.
- `Esc` closes an open protocol menu, or closes the manager if no menu is open.

## UI states

| State | Description | Primary actions |
| --- | --- | --- |
| Empty | No protocol-specific target selected | Choose protocol, type server, choose saved entry |
| Suggesting | User is typing in the server field | Accept suggestion, continue typing |
| Connecting | Transport and helper checks are in progress | Cancel |
| Browsing | Remote home directory is loaded | Enter folder, choose root, open |
| Opening | Final workspace backend is starting | Cancel |
| Error | Connection or browse failed | Retry, edit target, copy details |

## Design notes

- The UI should not ask for a project path before connecting. The directory browser provides the path.
- The first directory view should be the remote home directory. A manual path field can still support paste-in paths.
- SSH should lean on OpenSSH configuration instead of duplicating SSH settings in Nucleotide.
- WSL should prefer Linux filesystem paths. Windows-mounted paths inside WSL can work, but they should not be promoted as the fast path.
- The browse session should not evaluate `.envrc`, Nix flakes or shell hooks. Those belong to the chosen project root.

## Open questions

- Should the browser allow jumping above the home directory, or require manual path entry for that?
- Should saved connections open immediately, or always show the confirmation browser first?
- Should the manager expose helper install details by default or hide them behind "Copy details"?
- Should SSH config parsing include wildcard `Host` entries, or only concrete aliases?
- Should WSL distro state show running and stopped status if that requires a slower probe?

## Related documents

- [Remote workspace feature gaps](remote_workspace_feature_gaps.md)
- [Remote workspace reference design](research/2026-07-01-remote-workspace-reference-design.md)
- [SSH deployment strategies for nucleotide-remote](research/2026-07-02-ssh-nucleotide-remote-deployment.md)
- [GPUI Kit Stepper](https://gpui-kit.com/component/stepper/) — visual reference for the progress header; Nucleotide uses its own token-styled implementation.
