# Application menus

Nucleotide uses one menu definition for the native macOS menu bar and the
in-window Linux and Windows menu bars.

- **Nucleotide:** About, update checks, Settings and Quit. Hide commands appear
  only on macOS. Windows places Settings and Exit in File, and About and update
  checks in Help.
- **File:** New File, file/folder opening, remote connections, Save and Close File.
  New Window and Save As are omitted until their implementations are complete.
- **Edit:** History, clipboard operations, document/project search, completion,
  code actions and reverting the current change.
- **Selection:** Whole-document selection, syntax-aware expansion/shrinking,
  next/previous occurrences, splitting selections into lines and merging them.
  Occurrence commands use the current selections as a literal search pattern and
  add the next or previous match. They update Helix's search register.
- **View:** Panel visibility, Editor Layout splits, font sizing and preview tabs.
- **Go:** The Helix command prompt, file/buffer/symbol pickers, definitions,
  references and diagnostic navigation. Symbol and LSP actions use the same
  native request paths as editor input.
- **Run:** Runnable picker, nearest runnable, file tests and rerunning the last task.
- **Window:** Platform window operations.
- **Help:** Tutorial and documentation. Windows also includes About and update checks.

The developer-only Debug menu appears in builds with debug assertions, not in
normal release builds.

Settings opens the local `nucleotide.toml`, including from remote workspaces.
Reload Configuration applies changes without restarting. It is not a graphical
settings editor.

Menu activation restores the focus that existed before the menu opened. Shared
clipboard and Select All actions can therefore operate on focused text inputs;
editor-specific selection actions operate on the document view. Commands invoke
semantic actions rather than simulated Helix keystrokes, including in Insert mode.

In-window shortcut hints come from GPUI bindings in the original focus context.
Bindings handled exclusively by Helix, including modal sequences and fallback
shortcuts, are not advertised as universal menu accelerators. This does not
change their keyboard behaviour.
