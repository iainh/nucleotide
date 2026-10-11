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
