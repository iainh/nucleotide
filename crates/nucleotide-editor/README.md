# nucleotide-editor

Text rendering and editing logic for Nucleotide editor (Layer 4).

## Purpose

This crate handles text rendering, document display, scroll management, and line caching for the editor views. It bridges between the Helix editing engine and GPUI rendering.

## Public API

- **Document rendering**: `DocumentRenderer`, `LineLayout`
- **Editor view**: `EditorView`
- **Scroll management**: `ScrollManager`, `ScrollState`
- **Line caching**: `LineLayoutCache`, `LineCache`

## Dependencies

- `nucleotide-core`: For capability traits and event system
- `gpui`: For rendering
- `helix-*`: For editor functionality
- No dependency on `nucleotide-ui` (unidirectional dependency maintained)