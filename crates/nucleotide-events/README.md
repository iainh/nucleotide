# nucleotide-events

Event system definitions for Nucleotide editor (Layer 2).

## Purpose

This crate defines data-only domain event types shared by Nucleotide components.

## Public API

- **Domain modules**: `document`, `ui`, `workspace`, `lsp_events`, `completion`, `run`, `terminal`
- **Application integration**: the `nucleotide` crate embeds these events directly in its `Update` enum

## Dependencies

- `nucleotide-types`: For shared type definitions
- `serde`: For serialization
- `helix-view`, `helix-lsp`: For editor integration
