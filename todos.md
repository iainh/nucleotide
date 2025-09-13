# TODOs and Incomplete Items

This document summarizes TODO/FIXME-style markers and incomplete areas discovered across the codebase.

## Core App (`crates/nucleotide`)

- Cursor hidden variant: implement `CursorKind::Hidden` bounds (src/document.rs:3877).
- Save notification: status message overwritten by LSP (src/application/mod.rs:2123).
- Multi-server LSP: choose/merge results instead of first-only (src/application/mod.rs:3488).
- LSP event wiring: emit integration event for LSP server association (src/application/document_handler.rs:275).
- LSP handler routing: needs access to command sender to route to sync processor (src/application/lsp_handler.rs:365).
- View events unimplemented: cursor moved, split created, view closed (src/application/view_handler.rs:239/244/249).
- Completion/InputCoordinator integration: re-implement completion shortcuts and context management (src/workspace/mod.rs:3735, 3810).
- macOS window appearance: set NSWindow appearance to follow system (src/workspace/mod.rs:1667).
- Titlebar filename update: update with current filename on focus change (src/workspace/mod.rs:1908).
- Gutter/VCS integration events: UI updates based on VCS (src/workspace/mod.rs:2985, 2990).
- Status bar indicators: loading indicator and error notification (src/workspace/mod.rs:3586, 3610).
- File actions: implement Save As dialog and New Window action (src/workspace/mod.rs:5776, 5900).
- Testing scaffolding: extract real cursor position in events and reduce editor setup complexity for tests (src/application/app_core.rs:401/509/523/538).

## UI (`crates/nucleotide-ui`)

- Picker capability refactor: replace direct core references with a capability trait and integrate Nucleo properly (src/picker_view.rs:141, 232, 260, 270, 335, 548, 598, 675, 700, 1187).
- Completion UI notifications/retry: show user notifications and implement retry of failed operations (src/completion_v2.rs:1174, 1210).
- Completion cursor position: use actual document/workspace cursor for placement (src/completion_v2.rs:1554).
- Documentation panel: check async task completion and add scroll tracking when APIs available (src/completion_docs.rs:279, 573).
- Popup positioning/z-index: compute from real window bounds and apply proper z-index when API available (src/completion_popup.rs:389, 408).
- Focus/animations: consider animation support when GPUI APIs are available.
  Use global_input focus indicator config and tokens for styling rather than a dedicated focus_indicator module.
- Event bus: replace manual handling with an event bus subscription (src/info_box.rs:37).
- Linux integration: optional gsettings checks for button layout and theme (src/titlebar/linux_platform_detector.rs:224, 279); implement shading/always-on-top/context menu (src/titlebar/linux_window_controls.rs:307, 311, 315).

## LSP Layer (`crates/nucleotide-lsp`)

- Global env workaround: process-wide environment setting; seek better isolation (src/helix_lsp_bridge.rs:182).
- Integration test gap: `ServerLifecycleManager::start_server` not yet implemented for tests (src/integration_tests.rs:908).

## VCS (`crates/nucleotide-vcs`)

- Event payloads: populate real `doc_id`, base revision, actual HEAD, and add file status domain events (src/vcs_service.rs:69, 72, 79, 84).
- Background refresh: add periodic timer; currently on-demand only (src/vcs_service.rs:744).

## Workspace View Management

- Update per-view text styles when needed (crates/nucleotide/src/workspace/view_manager.rs:163).

## Themes/Assets/Queries (lower priority)

- Theme TODO notes in assets and runtime themes (e.g., crates/nucleotide/assets/themes/*, runtime/themes/sunset.toml).
- Tree-sitter queries: various TODO/FIXME comments under crates/nucleotide/runtime/queries/** pertaining to language highlight/indent rules.
