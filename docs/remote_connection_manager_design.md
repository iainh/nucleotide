# Remote connection manager design

Status: draft

This document defines a UI design for opening remote workspaces through a dedicated connection manager. It is a design-only document. It does not prescribe implementation slices or make code changes.

The manager should preserve the current "just works" behaviour while replacing the command-style `remote:` prompt as the primary guided path. The prompt can remain as a quick entry path for users who already know the exact target.

## Goals

- Let users choose `SSH` or `WSL` from a protocol dropdown.
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
2. The user selects `SSH` or `WSL`.
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
|  Location                                                         |
|  +------------------------------------------------------------+   |
|  | ~ / projects / nucleotide                                  |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Directories                                                     |
|  +------------------------------------------------------------+   |
|  | ..                                         parent           |   |
|  | > crates                                  18 directories    |   |
|  | > docs                                     2 directories    |   |
|  | > runtime                                  3 directories    |   |
|  | > scripts                                  0 directories    |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  Workspace root                                                   |
|  +------------------------------------------------------------+   |
|  | /home/iheggie/projects/nucleotide                         |   |
|  +------------------------------------------------------------+   |
|                                                                   |
|  [ Use current folder ]                         [ Open ]          |
|                                                                   |
+-------------------------------------------------------------------+
```

`Use current folder` copies the current browser location into the workspace root field. Selecting a row and pressing `Open` should first enter the row if it is not already the chosen workspace root.

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

- `Tab` moves through protocol, server, suggestions, directory browser, workspace root and actions.
- `Up` and `Down` move through suggestions or directory rows.
- `Enter` accepts a suggestion, connects or enters the selected directory.
- `Cmd+Enter` on macOS or `Ctrl+Enter` on other platforms opens the selected workspace root.
- `Esc` cancels the current popup; a second `Esc` closes the manager.

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
