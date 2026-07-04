# GPUI Interaction Patterns

This guide defines the default path for Nucleotide UI work on top of GPUI. The
goal is to make common UI behaviour predictable: contributors should describe a
menu, modal, list, input, panel, or resize handle without rebuilding focus and
event plumbing at each call site.

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

## Focus

Focusable components should expose or own a `FocusHandle` and render it with
`.track_focus(...)`.

Use GPUI tab primitives for ordinary traversal:

- `.tab_stop(true)` for focusable stops.
- `.tab_index(...)` when explicit ordering is required.
- `.tab_group()` for nested traversal groups.
- `window.focus_next(cx)` and `window.focus_prev(cx)` for Tab and Shift-Tab
  actions.

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

Until a shared modal/overlay layer is available, keep any new modal behaviour
small and isolated so it can migrate later.

## Text Input

Non-editor text fields should eventually use a GPUI-native `TextInput` wrapper
from `nucleotide-ui`. That wrapper should own editing state, cursor movement,
selection, clipboard actions, marked text / IME, submit/cancel events, and token
styling.

The Helix-backed editor remains on the Helix input path. The terminal remains on
the terminal byte path.

## Resize And Drag

New resizing behaviour should go through a shared resize/drag component once it
exists. A resize component should own:

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
