# nucleotide-lsp

Language Server Protocol support for Nucleotide editor (Layer 5).

## Purpose

This crate provides LSP integration, managing language servers and translating between LSP events and the editor's event system.

## Public API

- **LSP management**: `LspManager`, `LspState`
- **Document management**: `DocumentManager`
- **Server status**: `ServerStatus`, `LspStatus`

## Dependencies

- `nucleotide-core`: For event system
- `helix-lsp`: For LSP functionality
- `helix-view`: For document integration
- `tokio`: For async operations