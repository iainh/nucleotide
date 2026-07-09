# nucleotide-core

Shared event bridges and editor utilities for Nucleotide (Layer 3).

## Purpose

This crate provides the Helix-to-GPUI event bridge and small shared utilities used by the application layer.

## Public API

### Event Bridges
- `event_bridge`: Helix → GPUI event translation
- `AppEvent`: active document, UI, and workspace events delivered to GPUI

### Editor Utilities
- `DocumentManager`, `DocumentManagerMut`: document access helpers
- `PickerCapability`: picker integration boundary
- `PreviewTracker`: preview document state
- `SnippetTemplate`: snippet parsing and tab stops

## Dependencies

- Lower layers: `nucleotide-types`, `nucleotide-events`
- External: `gpui`, `helix-*` crates, `tokio`
