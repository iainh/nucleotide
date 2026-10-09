# GPUI Interaction Patterns

This guide defines the default path for Nucleotide UI work on top of GPUI. The
goal is to make common UI behaviour predictable: contributors should describe a
menu, modal, list, input, panel, or resize handle without rebuilding focus and
event plumbing at each call site.

## Just Works Contract

The `nucleotide-ui` layer is the compatibility boundary that makes GPUI feel
like an application toolkit. A `nucleotide-ui` component should "just work"
when a caller uses the wrapper and calls `nucleotide_ui::init(...)` during app
startup:

- standard key bindings are installed with the component's key context
- focus handles, tab stops, and focus restoration are owned by the wrapper
- common mouse lifecycles such as light-dismiss and drag cleanup are handled
- text fields provide cursor movement, selection, clipboard, and IME hooks
- layout-affecting interaction state is reset consistently after actions
- GPUI tests cover the wrapper contract before app features depend on it

App-owned components outside `nucleotide-ui` should follow the same shape with
a local `init(cx)` function, a single exported key-context constant where useful,
and startup wiring near the component owner. Call sites should supply domain data
and callbacks. They should not need to remember the GPUI event recipe for
ordinary prompts, pickers, menus, modals, text fields, or resize handles.

## Default Rule

Use `nucleotide-ui` wrappers for common application UI. Reach for raw GPUI
keyboard, mouse, focus, or layout APIs only when building a low-level custom
surface such as the editor, terminal, or a new reusable wrapper.

## Keyboard Input

Prefer GPUI actions over raw key matching:

- Define app commands with `actions!`.
- Bind keys with `KeyBinding::new(..., Some(context))` when a shortcut belongs
  to a focused surface.
- Put `.key_context(...)` on the component that owns the interaction.
- Handle commands with `.on_action(...)`.
- Keep the focused surface's standard key map beside the component, usually in
  that component's `init(cx)` function. Reserve `main.rs` for global shortcuts
  and the editor/Helix bridge.

Raw `.on_key_down(...)` is reserved for:

- Helix editor input translation.
- Terminal byte translation.
- Text input internals.
- Low-level components that are intentionally building a new reusable input
  abstraction.

Do not add new workspace-level `match ev.keystroke.key.as_str()` blocks for
menus, pickers, prompts, modals, or list navigation. Move those behaviours into
the component that owns the focused surface.

`InputCoordinator` is the app/Helix bridge for workspace-level contexts such as
the editor, file tree, and overlays. It should not grow per-widget navigation
rules that belong in a focused component's actions.

The old `nucleotide_ui::global_input` dispatcher has been removed. Do not add a
second shortcut registry, dismiss-handler registry, or focus-group manager for
app UI. Terminal byte translation uses `nucleotide_ui::terminal_keys` instead.

## Focus

Focusable components should expose or own a `FocusHandle` and render it with
`.track_focus(...)`.

Use GPUI tab primitives for ordinary traversal:

- `.tab_stop(true)` for focusable stops.
- `.tab_index(...)` when explicit ordering is required.
- `.tab_group()` for nested traversal groups.
- `window.focus_next(cx)` and `window.focus_prev(cx)` for Tab and Shift-Tab
  actions.

Components that own a focus scope should bind
`nucleotide_ui::actions::focus::{FocusNext, FocusPrevious}` in their key
context and wrap their content in `nucleotide_ui::FocusTraversal` so the
component gets standard `window.focus_next(cx)` and `window.focus_prev(cx)`
behaviour. `FocusTraversal` installs a default `FocusTraversal` key context for
Tab and Shift-Tab; use `FocusTraversal::key_context(...)` when a component owns
a more specific context, or `FocusTraversal::without_key_context()` when the
parent surface owns all key routing. Do not add new Tab handling to
`InputCoordinator`; Helix, terminals, and focused components own their own Tab
semantics.

Buttons that need to participate in traversal should use
`Button::focus_handle(...)`. The shared button owns focus-visible styling and
keyboard activation for focused buttons, so dialogs and forms should not add
local Space/Enter handlers for ordinary button clicks.

`nucleotide_ui::FocusCoordinator` lives in the `focus` module and should be used
as a role registry for major surfaces such as the editor, terminal, picker,
prompt, diagnostics, and file tree. It should not become a second per-widget
navigation system.

GPUI owns keyboard focus. Helix's active view, mirrored by `ViewManager` and
`DocumentView::is_focused`, identifies the active editor split, not the focused
UI surface. A document's GPUI focus subscription activates its Helix split.
Helix split transitions move GPUI focus only while the previous editor surface
still owns it; they must not take focus from the tree, terminal or an overlay.

Perform focus changes in actions, lifecycle subscriptions and explicit opening
or dismissal transitions, never in `render`. Windowless core events defer their
transition until the current effect cycle ends. `Workspace` uses
`on_focus_lost` when a focused surface disappears, while `ModalLayer` restores
its captured previous focus. Dismissing a completion that never owned focus
must not move keyboard focus. Workspace mouse bubbling must not override a
child's focus choice.

## Native editor geometry

`NativeEditorView` prepares each frame in prepaint, once its pane bounds are
known, before constructing and prepainting `EditorSurface`. Preparation uses
`prepare_native_editor_frame` to synchronize gutters, soft wrap, scroll extent,
cursor reveal and surface metrics. `EditorDocumentElement` paints that prepared
frame with the same immutable layout. Do not synchronize geometry in paint or
schedule a second render to repair scrollbar geometry.

Scrollbar thumbs and markers use the current prepaint track bounds, not the
previous frame's stored bounds. The active Helix split publishes its shaped
cursor anchor during prepaint; completion overlays read it during their own
prepaint. Inactive splits must not overwrite the active split's anchor. Keep
per-pane native bounds local: Helix's tree remains authoritative for split
layout.

Completion content stays in layout flow so its anchor can measure the list and
documentation panel together. Use deferred drawing for the popup so window-edge
snapping does not leave it clipped by its editor container.

## Lists And Menus

Use `nucleotide_ui::Navigable` for action-driven focus traversal in list-like
surfaces. It installs a default `Navigable` key context for Up/Down and
Ctrl-P/Ctrl-N, handles `menu::SelectDown` and `menu::SelectUp` actions, moves
focus to the next or previous `NavigableEntry`, and scrolls to an optional
`ScrollAnchor`. Use `Navigable::key_context(...)` only when embedding it in a
surface that already owns compatible key bindings, or
`Navigable::without_key_context()` when a parent component must own all key
routing.

Use `PopupMenu` for menu-like controls instead of adding local arrow-key,
Enter, or Escape branches. Menus should own:

- selected item movement
- confirmation
- dismissal
- disabled item handling
- light-dismiss
- focus restoration
- accessibility roles
- command dispatch

The workspace should build menu data and domain handlers. It should not know how
to move menu selection.

Use `nucleotide_ui::PopupMenuSurface` when a popup menu needs full-window
occlusion, light-dismiss, anchored positioning, and window-edge snapping. The
menu owns keyboard selection and command dispatch; the surface owns the backdrop
recipe.

## Modals And Overlays

Modal and overlay implementations should own:

- previous focus capture
- focusing the active surface after mount
- Escape dismissal
- light-dismiss
- click occlusion to prevent fall-through
- optional focus-out dismissal
- focus restoration
- pre-dismiss checks for unsaved-change flows

Use `nucleotide_ui::ModalLayer` for modal surfaces that need dismissal policy,
background occlusion, and focus restoration.

Use `nucleotide_ui::OverlaySurface` for transient app overlays such as prompts,
pickers, and manager panels that need full-window occlusion, Escape dismissal,
light-dismiss, and click containment but are still hosted by an app-specific
overlay controller. The caller supplies the domain view and dismiss callbacks;
the wrapper owns the GPUI event recipe.

## Text Input

Non-editor text fields should use `nucleotide_ui::TextInput`. It owns editing
state, cursor movement, selection, clipboard actions, marked text / IME,
submit/cancel events, and token styling.

The Helix-backed editor registers `DocumentView` as an `EntityInputHandler`
through `ElementInputHandler` during document paint. Normal/select keys, insert
bindings, pending key sequences, callbacks and shortcuts remain on the Helix
command bridge. Unbound printable insert keys propagate to GPUI's committed-text
path; neither the document nor the workspace inserts their raw key a second time.
Native commits use Helix insertion primitives and retain auto-pairs, completion
hooks, multi-cursors, insert-session undo and counted dot-repeat. Replay records
which characters were committed text so it doesn't reinterpret them as bindings.
Committed characters check the workspace's completion commit-character path
before insertion. Raw preedit keys must not accept or filter a completion.
Completion sessions retain their original document and prefix range. Acceptance
may reuse a session after word-prefix typing/deletion only when all text outside
that prefix is unchanged; LSP edit ranges map through that prefix change. Other
edits and changes during asynchronous resolution retain strict version checks.
Helix macro recording/replay remains unsupported by this command bridge;
dot-repeat is not macro support.

Native ranges use UTF-16 code units and convert through Rope character indices.
The insert cursor is advertised as a caret, not Helix's one-grapheme block
selection. Explicit replacement ranges replace text; ordinary commits keep
Helix's insert-at-each-cursor semantics. Marked text uses temporary Helix
transactions and a savepoint. Commit restores the savepoint before inserting
the final text; cancellation restores the original text and selections. Blur,
window deactivation, pointer selection and command keys cancel pending preedit.
Application-initiated cancellation also resets OS preedit by disabling native
input through a backend-observed frame before re-enabling it. Printable keys
stay on the Helix bridge during that reset. Capture-phase completion commands
must cancel preedit too, since they bypass the document's raw-key handler.
Candidate bounds and point queries use the current painted line cache, including
soft-wrap segments, display-byte maps, scrolling and pane bounds.

Test native composition with an OS input method as well as the handler contract.
On Linux, GPUI needs Wayland text-input-v3; a nested compositor that exposes only
text-input-v1 cannot verify this path. Nested Sway with Fcitx5 Pinyin provides a
working native control. Check saved files and protocol events independently of
screenshots, and allow key hints and selection paint to settle before captures.
The terminal remains on the terminal byte path.

## Resize And Drag

New resizing behaviour should use `nucleotide_ui::ResizeDragController`,
`nucleotide_ui::resize_handle`, `nucleotide_ui::resize_capture_area`, or a
wrapper built on them. Resize code should keep ownership of:

- drag start state
- axis and cursor
- clamp rules
- drag updates
- mouse-up and mouse-up-outside cleanup
- optional double-click reset

Call sites should provide domain constraints and update callbacks, not rebuild
the GPUI drag lifecycle.

Use `resize_handle` for transparent split hitboxes when domain geometry already
exists, such as editor pane dividers. Use `resize_capture_area` on the surface
that should receive active drag move/up cleanup while a resize is in progress.
Use higher-level wrappers like `sidebar_split`, `right_sidebar_split`, and
`bottom_panel_split` when the component can own both layout and resize
mechanics.

Paint resize handles after both panes so the full hitbox stays reachable and
drag initiation runs before editor selection handlers. Measure drag displacement
from the original mouse-down position, including movement before GPUI's drag
threshold. Cover both sides of the divider and a first move that leaves its
hitbox in interaction tests.

Editor selection drags stay with the pane that accepted the initial left-button
press, even when the pointer leaves its bounds or crosses another pane. GPUI's
hover-only `on_mouse_move` is not sufficient for this lifecycle: `EditorSurface`
registers window-level move/up listeners during paint and gates them on its
per-pane drag state. Other buttons and presses begun elsewhere do not arm it.

After movement begins, holding the pointer near or beyond an edge scrolls the
existing viewport on a 16 ms executor timer. Speed scales with overshoot and
elapsed time; viewport limits still apply. Re-entering the centre pauses scroll
without dropping the selection anchor. Each tick hit-tests the painted line
cache before scrolling for the next frame, preserving same-frame geometry.
Release (including outside), editor blur, window deactivation, and overlay focus
transfer end the drag. A new press is required to resume after cancellation.

## Layout

Prefer semantic layout wrappers and token-based sizes over ad-hoc absolute
positioning. Use raw absolute coordinates only for anchored overlays, editor
geometry, hitboxes, or cases where a reusable layout component cannot express
the requirement yet.

Use `AppShell`, `WorkspaceChrome`, `EditorPaneGrid`, `Panel`, `Toolbar`,
`BottomPanel`, and `StatusBar` for common app surfaces before hand-building
another token-styled shell from `div()`. These are thin wrappers; call sites
should still own domain content and any geometry that is genuinely
editor-specific.

Complex layout calculations should be extracted into testable structs that
produce constraints or rectangles independently from GPUI event handling.
Use `PanelLayout` for reusable split-panel size constraints and reset values.
Workspace-local geometry such as `EditorPaneLayout` can stay close to the Helix
view model while still exposing panes, resize handles, and visual divider lines
as one tested contract.

## Testing

Add GPUI tests for interaction contracts when adding or changing wrappers:

- key action dispatch
- focus movement and restoration
- menu navigation and dismissal
- text input editing
- resize clamping and cleanup

The contract should be proven at the wrapper level first. Feature code can then
depend on that behaviour instead of retesting every GPUI event branch locally.

Use `nucleotide_ui::ComponentGallery` for quick visual coverage of shared UI
primitives. New reusable components should get a focused GPUI test first, then a
small gallery section when visual state or composition matters.
