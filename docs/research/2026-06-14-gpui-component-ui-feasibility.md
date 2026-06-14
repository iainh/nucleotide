# Research: gpui-component UI Replacement Feasibility

**Date**: 2026-06-14
**Question**: Can Nucleotide replace custom non-editor UI components with
`gpui-component`, and what can we learn from Hummingbird's polished GPUI UI?
**Status**: Complete

## Context

Nucleotide has a growing `nucleotide-ui` crate plus several app-level UI
surfaces outside the native editor component. The largest custom surfaces are
not simple widgets: `PickerView`, `OverlayView`, `FileTreeView`, diagnostics,
tabs, split panes, titlebar/window chrome, completion UI, and notifications all
carry behavior, focus routing, editor integration, or application state.

The workspace already depends on `gpui-component`:

- `Cargo.toml` declares `gpui-component = "0.5.0"`.
- `Cargo.lock` resolves `gpui-component` to `0.5.1`.
- `crates/nucleotide-ui/src/lib.rs` calls `gpui_component::init(cx)` inside
  `nucleotide_ui::init`, so startup integration already exists.

This means adoption is mainly about API, theme, and behavior compatibility, not
about adding a new dependency from scratch.

## Findings

### gpui-component Has Good Coverage For Common Widgets

The installed `gpui-component-0.5.1` crate exposes reusable modules for:

- `button`, `input`, `list`, `tree`, `tab`, `table`, `menu`, `popover`,
  `notification`, `sidebar`, `dialog`, `sheet`, `resizable`, `dock`,
  `scroll`, `text`, `tooltip`, and `title_bar`.
- It also has themed `Root`, `WindowBorder`, `VirtualList`, LSP-aware input
  popovers, markdown/simple HTML rendering, and charting.

The upstream README describes "60+ cross-platform desktop UI components",
stateless `RenderOnce` components, a built-in theme system, virtualized list
and table components, dock layout, markdown/HTML rendering, and an editor with
LSP features.

Relevant installed source:

- `~/.cargo/registry/src/.../gpui-component-0.5.1/src/lib.rs`
- `src/button/button.rs`
- `src/input/input.rs`
- `src/input/state.rs`
- `src/list/list.rs`
- `src/tree.rs`
- `src/tab/tab.rs`
- `src/tab/tab_bar.rs`
- `src/table/mod.rs`
- `src/notification.rs`
- `src/popover.rs`
- `src/resizable/panel.rs`
- `src/sidebar/mod.rs`

### Theme Integration Is The First Real Adapter

`gpui-component` has its own `theme::Theme` global and `ActiveTheme::theme()`
extension trait. Nucleotide has `nucleotide_ui::Theme` and
`nucleotide_ui::ThemedContext::theme()`. The method names overlap, but the
types are distinct.

This is manageable if adoption goes through a small adapter layer and avoids
importing both theme extension traits in the same module. The more important
work is visual: mapping Nucleotide `DesignTokens` to `gpui-component` theme
colors so introduced widgets do not look like a different design system.

### Some Components Are Direct Replacement Candidates

Our custom `Button` and `ListItem` are substantial:

- `crates/nucleotide-ui/src/button.rs`: 976 lines.
- `crates/nucleotide-ui/src/list_item.rs`: 868 lines.

`gpui-component::button::Button` already supports variants, sizes, icons,
loading state, selected state, tooltips, tab stops, compact mode, and custom
colors. `gpui-component::list` and `gpui-component::tree` provide selectable,
keyboard-navigable list/tree state.

These are feasible to replace behind compatibility wrappers, but a direct
rename-level swap would break API and visual assumptions.

### Input Is A High-Value Replacement Candidate

`crates/nucleotide-ui/src/input.rs` is only a styled display wrapper around a
value and placeholder. It does not own editing state, and its `ComponentFactory`
implementation is `unimplemented!("Input requires FocusHandle")`.

`gpui-component::input::Input` binds to `InputState` and provides real editing:
cursor movement, selection, copy/cut/paste, undo/redo, multiline behavior,
scrollbars, keybindings, search, context menu, LSP-related popovers, and
`InputEvent` emission.

This makes it a strong candidate for command prompts, picker filter inputs,
settings inputs, and other non-editor text fields. It should not be treated as
a replacement for the Helix-backed editor view.

### Whole-Surface Replacement Is Not Equally Feasible

Several Nucleotide surfaces include domain behavior that no generic component
can replace directly:

- `PickerView` handles Helix-style picker semantics, filtering, keyboard
  routing, previews, file/buffer-specific columns, preview document cleanup,
  and editor preview rendering.
- `OverlayView` anchors completions, prompts, hover docs, picker previews, and
  diagnostics to editor/workspace coordinates and manages focus restoration.
- `FileTreeView` handles async expansion, file watching, VCS status overlays,
  focus coordinator integration, context menu events, and workspace commands.
- `DiagnosticsPanel` flattens LSP state, sorts/filter diagnostics, paints
  severity icons, and emits editor jump events.

For these, the likely win is replacing subcomponents and layout primitives,
not replacing the whole view in one pass.

### Hummingbird Shows A Different Kind Of Polish

Hummingbird is a GPUI music player, currently mirrored on GitHub and migrated
to Codeberg. It does not appear to use `gpui-component`; it builds a focused
component layer under `src/ui/components`. Its value for Nucleotide is in the
patterns:

- Thin semantic components over GPUI primitives: `button`, `sidebar`,
  `popover`, `modal`, `window_chrome`, `table`, `menu`, and input controls.
- A single app theme global with explicit semantic colors for every state.
- Polished window chrome isolated in one component, including client-side
  resize hitboxes, rounding, shadows, borders, text defaults, and tiling
  behavior.
- Tables cache per-row views, persist column widths/order/view mode, support
  grid/list modes, and prune cached views.
- Popovers and modals have small, predictable APIs and own occlusion/dismissal
  behavior.
- Sidebar items handle collapsed states and hover labels as component
  behavior, not one-off layout code.

Relevant Hummingbird files inspected through the GitHub mirror:

- `src/ui/components/button.rs`
- `src/ui/components/sidebar.rs`
- `src/ui/components/popover.rs`
- `src/ui/components/modal.rs`
- `src/ui/components/table.rs`
- `src/ui/components/window_chrome.rs`
- `src/ui/theme.rs`

## Replacement Matrix

| Nucleotide surface | gpui-component candidate | Feasibility | Recommendation |
| --- | --- | --- | --- |
| `nucleotide_ui::Button` | `button::Button` | High | Replace behind a compatibility wrapper. Map Nucleotide variants/sizes to gpui-component variants/sizes first. |
| `nucleotide_ui::ListItem` | `list::ListItem`, `list::ListState` | High for row styling, medium for state | Replace row rendering first. Defer list state until picker/file-tree behavior is adapted. |
| `nucleotide_ui::Input` | `input::Input`, `input::InputState` | High | Prioritize. It fixes the current static-input limitation and unlocks prompt/filter simplification. |
| `PromptView` | `input::Input`, `list`, `popover` | Medium-high | Rebuild around `InputState`; keep Helix command history/completion adapter in Nucleotide. |
| `NotificationView` | `notification::Notification` | High | Replace. gpui-component provides IDs, autohide, dismissal animation, icons, and optional actions. |
| `split.rs` | `resizable::ResizablePanelGroup`, `dock` | Medium-high | Replace generic split mechanics. Preserve persisted sidebar/bottom-panel sizes through adapter state. |
| `scrollbar.rs` | `scroll::Scrollbar` | Medium | Try after split/list adoption. Keep tests for GPUI negative scroll offsets and terminal/editor custom handles. |
| `Tab`/`TabBar` | `tab::Tab`, `tab::TabBar` | Medium | Use gpui-component tab bar as a presentational base. Keep document IDs, VCS/modified markers, close behavior, and overflow policy in Nucleotide. |
| `FileTreeView` | `tree`, `list`, `sidebar` | Medium | Do not replace whole view initially. Use gpui-component tree/list/sidebar primitives after adapting async/VCS/context behavior. |
| `DiagnosticsPanel` | `table`, `list`, `dialog`/`popover` | Medium | Convert to table/list primitives only if keyboard and jump behavior stay exact. Severity icon rendering remains local. |
| `PickerView` | `input`, `list`, `popover`, maybe `table` | Low-medium | Replace internals gradually. Whole replacement is risky because previews and Helix-like picker semantics are app-specific. |
| Completion popup | `input` completion popovers | Low-medium | gpui-component completion UI is tied to its input/LSP model. Reuse ideas or parts after prompt/input migration. |
| Titlebar/window controls | `title_bar`, `WindowBorder`; Hummingbird `window_chrome` pattern | Medium | Prefer a Nucleotide wrapper inspired by Hummingbird/gpui-component. Linux-specific controls likely remain local. |
| Settings/about/forms | `setting`, `form`, `dialog`, `sheet` | High for future work | Good candidates when settings UI is revisited. |

## Options Considered

| Option | Pros | Cons | Effort |
| --- | --- | --- | --- |
| Replace all custom widgets directly | Maximum deletion if it works | High regression risk; theme mismatch; public APIs differ; app-specific behavior is mixed into views | High |
| Compatibility wrappers around gpui-component | Keeps call sites stable; lets us migrate one widget at a time | Some temporary adapter code; may hide differences until visual QA | Medium |
| Use gpui-component only for new UI | Low immediate risk | Does not reduce existing maintenance surface | Low |
| Adopt Hummingbird-style internal primitives instead | Full control; aligns with our token system | Reimplements what gpui-component already offers | Medium |

## Recommended Path

1. Add a `gpui_component_theme` adapter inside `nucleotide-ui` that derives a
   `gpui_component::theme::Theme`/theme config from `DesignTokens`.
2. Replace `nucleotide_ui::Input` first with a wrapper around
   `gpui_component::input::InputState`.
3. Rebuild `PromptView` around that input wrapper, keeping prompt history and
   command completion logic local.
4. Replace `NotificationView` with `gpui_component::notification` and wire
   save/LSP/editor statuses into notification IDs.
5. Replace split mechanics with `ResizablePanelGroup`, preserving current
   persisted widths/heights and double-click reset behavior if still desired.
6. Replace `Button` and `ListItem` behind compatibility wrappers once the theme
   adapter proves visually correct.
7. Evaluate `TabBar`, `FileTreeView`, `DiagnosticsPanel`, and `PickerView`
   surface-by-surface. Treat gpui-component as a toolkit for their internals,
   not as a one-shot replacement.

## Hummingbird Ideas Worth Borrowing

- Encapsulate window chrome as one component, including resize hitboxes,
  rounding, shadows, tiled-window adjustments, font defaults, and root
  background.
- Keep small semantic component APIs. Hummingbird's `button()`, `popover()`,
  `modal()`, and `sidebar_item()` are thin, but they make layout code read in
  domain terms.
- Persist user layout choices for complex views. Hummingbird's table stores
  column widths, hidden columns, order, sort, and list/grid mode; similar
  patterns could help diagnostics, picker columns, file tree width, and panel
  sizing.
- Use row/entity caching for expensive virtualized views. This is relevant for
  diagnostics, picker preview lists, and any future file-tree scaling work.
- Prefer explicit semantic state colors over ad-hoc lightening/darkening at
  call sites. Nucleotide already has tokens, but the app-level views still mix
  direct token lookup, provider lookup, and local color math.

## Open Questions

- Should Nucleotide expose `gpui-component` types in public APIs, or keep them
  behind `nucleotide-ui` wrappers?
- Do we want `gpui-component`'s theme to become the base theme, or should
  Nucleotide remain token-first and generate a compatible gpui-component theme?
- Should `PickerView` remain a Nucleotide-specific high-level component with
  gpui-component internals, or should picker behavior be decomposed into
  reusable input/list/preview primitives first?
- How much visual drift is acceptable while replacing primitives? Screenshot
  comparison would be needed before merging broad UI swaps.

## References

- Nucleotide `Cargo.toml`
- Nucleotide `Cargo.lock`
- `crates/nucleotide-ui/src/lib.rs`
- `crates/nucleotide-ui/src/input.rs`
- `crates/nucleotide-ui/src/button.rs`
- `crates/nucleotide-ui/src/list_item.rs`
- `crates/nucleotide-ui/src/picker_view.rs`
- `crates/nucleotide-ui/src/prompt_view.rs`
- `crates/nucleotide-ui/src/notification.rs`
- `crates/nucleotide-ui/src/split.rs`
- `crates/nucleotide-ui/src/scrollbar.rs`
- `crates/nucleotide/src/file_tree/view.rs`
- `crates/nucleotide/src/tab.rs`
- `crates/nucleotide/src/diagnostics_panel.rs`
- `crates/nucleotide/src/overlay.rs`
- `https://github.com/longbridge/gpui-component`
- `https://github.com/hummingbird-player/hummingbird`
- `https://codeberg.org/hummingbird/hummingbird`
