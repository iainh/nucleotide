# GPUI Interaction Patterns

This guide defines the default path for Nucleotide UI work on top of GPUI. The
goal is to make common UI behaviour predictable: contributors should describe a
menu, modal, list, input, panel, or resize handle without rebuilding focus and
event plumbing at each call site.

## Just Works Contract

The `nucleotide-ui` layer is the compatibility boundary that makes GPUI feel
like an application toolkit. A component should "just work" when a caller uses
the wrapper and calls `nucleotide_ui::init(...)` during app startup:

- standard key bindings are installed with the component's key context
- focus handles, tab stops, and focus restoration are owned by the wrapper
- common mouse lifecycles such as light-dismiss and drag cleanup are handled
- text fields provide cursor movement, selection, clipboard, and IME hooks
- layout-affecting interaction state is reset consistently after actions
- GPUI tests cover the wrapper contract before app features depend on it

Call sites should supply domain data and callbacks. They should not need to
remember the GPUI event recipe for ordinary prompts, pickers, menus, modals,
text fields, or resize handles.

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

`nucleotide_ui::global_input::GlobalInputDispatcher` is legacy scaffolding and
is not wired into the app. Do not register new shortcuts, dismiss handlers, or
focus groups there.

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
`nucleotide_ui::actions::focus::{FocusNext, FocusPrevious}` in their key context
and handle those actions with `window.focus_next(cx)` and `window.focus_prev(cx)`.
Do not add new Tab handling to `InputCoordinator`; Helix, terminals, and focused
components own their own Tab semantics.

Buttons that need to participate in traversal should use
`Button::focus_handle(...)`. The shared button owns focus-visible styling and
keyboard activation for focused buttons, so dialogs and forms should not add
local Space/Enter handlers for ordinary button clicks.

`FocusCoordinator` should be used as a role registry for major surfaces such as
the editor, terminal, picker, prompt, diagnostics, and file tree. It should not
become a second per-widget navigation system.

## Lists And Menus

Use `nucleotide_ui::Navigable` for action-driven focus traversal in list-like
surfaces. It handles `menu::SelectDown` and `menu::SelectUp` actions, moves
focus to the next or previous `NavigableEntry`, and scrolls to an optional
`ScrollAnchor`.

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
pickers, and manager panels that need full-window occlusion, light-dismiss, and
click containment but are still hosted by an app-specific overlay controller.
The caller supplies the domain view and dismiss callback; the wrapper owns the
GPUI event recipe.

## Text Input

Non-editor text fields should use `nucleotide_ui::TextInput`. It owns editing
state, cursor movement, selection, clipboard actions, marked text / IME,
submit/cancel events, and token styling.

The Helix-backed editor remains on the Helix input path. The terminal remains on
the terminal byte path.

## Resize And Drag

New resizing behaviour should use `nucleotide_ui::ResizeDragController` or a
wrapper built on it. Resize code should keep ownership of:

- drag start state
- axis and cursor
- clamp rules
- drag updates
- mouse-up and mouse-up-outside cleanup
- optional double-click reset

Call sites should provide domain constraints and update callbacks, not rebuild
the GPUI drag lifecycle.

## Layout

Prefer semantic layout wrappers and token-based sizes over ad-hoc absolute
positioning. Use raw absolute coordinates only for anchored overlays, editor
geometry, hitboxes, or cases where a reusable layout component cannot express
the requirement yet.

Use `WorkspaceChrome`, `Panel`, `Toolbar`, and `StatusBar` for common app
surfaces before hand-building another token-styled shell from `div()`. These are
thin wrappers; call sites should still own domain content and any geometry that
is genuinely editor-specific.

Complex layout calculations should be extracted into testable structs that
produce constraints or rectangles independently from GPUI event handling.

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
