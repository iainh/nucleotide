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

The Helix-backed editor remains on the Helix input path. The terminal remains on
the terminal byte path.

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
