# nucleotide-workspace

Workspace management for Nucleotide editor (Layer 5).

## Purpose

This crate manages workspace-level concerns including tabs, layout, and window management. It coordinates between editor state and UI components without creating circular dependencies.

## Public API

- **Workspace management**: `WorkspaceManager`
- **Tab management**: `Tab`, `TabManager`
- **Layout**: `Layout`, `LayoutDirection`, `Panel`

## Dependencies

- `nucleotide-core`: For capability traits
- `nucleotide-ui`: For UI components
- `gpui`: For UI framework
- `helix-*`: For editor integration