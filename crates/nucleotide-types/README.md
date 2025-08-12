# nucleotide-types

Core type definitions for Nucleotide editor (Layer 1).

## Purpose

This crate provides pure data structures and type definitions used throughout the Nucleotide editor. It has minimal dependencies and can be compiled without any heavy GUI or editor dependencies.

## Features

- `gpui-bridge`: Enable conversions to/from GPUI types
- `helix-bridge`: Enable conversions to/from Helix types

## Public API

- **Font types**: `Font`, `FontStyle`, `FontWeight`, `FontSettings`
- **Configuration**: `FontConfig`, `EditorFontConfig`, `UiFontConfig`
- **Editor types**: `EditorStatus`, `Severity`
- **Completion**: `CompletionTrigger`

## Dependencies

- Core: `serde` only
- Optional: `gpui`, `helix-core` (behind feature flags)