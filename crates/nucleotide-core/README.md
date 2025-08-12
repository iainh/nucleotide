# nucleotide-core

Core event bridges and capabilities for Nucleotide editor (Layer 3).

## Purpose

This crate provides the fundamental event translation layer between Helix and GPUI, along with capability traits that enable loose coupling between layers.

## Public API

### Capability Traits
- `EditorState`, `EditorAccess`, `EditorCapabilities`
- `EventEmitter`, `EventSubscriber`
- `ThemeProvider`, `OverlayProvider`
- `ScrollManager`, `LineCache`

### Event Bridges
- `event_bridge`: Helix → GPUI event translation
- `gpui_to_helix_bridge`: GPUI → Helix event translation
- `EventAggregator`: Central event routing

### Command System
- `Command`, `ParsedCommand`: Command parsing and execution

## Dependencies

- Lower layers: `nucleotide-types`, `nucleotide-events`
- External: `gpui`, `helix-*` crates, `tokio`