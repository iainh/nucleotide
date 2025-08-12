# nucleotide-events

Event system definitions for Nucleotide editor (Layer 2).

## Purpose

This crate defines the event types and event bus system used for communication between different components of the Nucleotide editor.

## Public API

- **Event types**: `AppEvent`, `CoreEvent`, `UiEvent`, `WorkspaceEvent`, `LspEvent`
- **Event bus**: `EventBus`, `EventHandler`
- **Enums**: `MessageSeverity`, `PanelType`, `PickerType`, `SplitDirection`

## Dependencies

- `nucleotide-types`: For shared type definitions
- `serde`: For serialization
- `helix-view`, `helix-lsp`: For editor integration