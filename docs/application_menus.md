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

Settings offers separate entries to open the local `nucleotide.toml` and Helix
`config.toml`, including from remote workspaces. The Helix entry opens the file
selected by `--config` when supplied. If it doesn't exist, it opens a new buffer
that you can save to create the file. Reload Configuration applies changes
without restarting. It is not a graphical settings editor.

Menu activation restores the focus that existed before the menu opened. Shared
clipboard and Select All actions can therefore operate on focused text inputs;
editor-specific selection actions operate on the document view. Commands invoke
semantic actions rather than simulated Helix keystrokes, including in Insert mode.

In-window shortcut hints come from GPUI bindings in the original focus context.
Bindings handled exclusively by Helix, including modal sequences and fallback
shortcuts, are not advertised as universal menu accelerators. This does not
change their keyboard behaviour.

## Windows popup material fallback

With `look = "system"`, Windows menus and popups use an intentional opaque
light/dark fill. This is a readability fallback, not acrylic or native material
parity. Theme-derived UI and other platforms retain their existing appearance.
The palette field named `acrylic` supplies a colour only; it does not enable a
compositor effect. The main window's native Mica backdrop is separate and unchanged.

GPUI currently renders the menu and underlying editor into one Direct3D swapchain
on one DirectComposition visual. Windows' documented
[`DWMSBT_TRANSIENTWINDOW`](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type)
provides Desktop Acrylic behind an entire HWND on Windows 11 build 22621 or later,
not behind a rectangle within that swapchain. Setting it on the main window would
replace the window backdrop without giving the menu its own material.

Native transient acrylic is deferred until GPUI can host a separate popup window
or composition layer while preserving menu focus, keyboard routing, submenu
placement, light dismiss, hit testing, clipping and resource lifetimes. A custom
framebuffer blur would instead be in-app acrylic, not Desktop Acrylic parity;
that renderer expansion is also deferred. See Microsoft's
[material distinctions](https://learn.microsoft.com/en-us/windows/apps/develop/ui/materials).
