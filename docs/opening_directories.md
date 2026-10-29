# Opening directories

Opening a directory replaces the current project and closes its open file
buffers, including buffers shown in other editor panes. Image tabs are closed
too. Scratch buffers (buffers without a file path) stay open, with their contents
unchanged. If no scratch buffer exists, the editor creates an empty one.

If a file buffer has unsaved changes, Nucleotide asks before closing any buffers
or switching directories. Choose **Close Without Saving** to discard those file
changes and open the selected directory. Cancelling keeps the current directory
and all buffers open. Unsaved scratch contents do not require confirmation
because scratch buffers are preserved.

Opening an individual file does not close existing buffers.

When detecting a project from a file or the startup directory, Nucleotide checks
ancestor `Cargo.toml` files for a workspace table. The nearest Cargo workspace
takes precedence over a member crate's manifest or other project markers. Without
a Cargo workspace, file-based detection uses the nearest project marker; startup
directory detection uses the nearest VCS marker or the startup directory itself.

## Dragging files into the window

Drag one or more files from your file manager into the Nucleotide window to
open them. This works on Windows, macOS and Linux (Wayland). Existing buffers
stay open, and the last dropped file becomes active with editor focus. Dropping
an already open file selects its existing buffer instead of creating a duplicate.

Files open only when you release the drag inside the window. Cancelling the drag
doesn't open anything. Directories and paths that no longer exist are ignored;
use **Open Directory** to switch projects. Local file drops are not supported in
remote workspaces, where Nucleotide shows a warning instead.
