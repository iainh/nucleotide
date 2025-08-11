# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is **Nucleotide**, a native GUI implementation of the Helix modal text editor built with GPUI (Zed's GPU-accelerated UI framework). It's a Rust project that wraps Helix's terminal components in a native GUI while maintaining full compatibility with Helix's configuration and runtime.

## Key Architecture

### Core Components

- **`src/application.rs`**: The heart of the application - wraps Helix's `Editor`, `Compositor`, and `Jobs` system. Handles the main event loop and bridges between GPUI events and Helix commands.

- **`src/workspace.rs`**: Main UI container that manages document views, file tree, overlays, and layout. Coordinates between different UI components and handles focus management.

- **`src/document.rs`**: Renders the text editor view using GPUI, translating Helix's terminal-based rendering to GPU-accelerated graphics. Manages scroll state and text layout.

- **Event Bridge System**:
  - `src/event_bridge.rs`: Sends Helix events to GPUI components
  - `src/gpui_to_helix_bridge.rs`: Converts GPUI inputs to Helix commands
  - These bridges enable bi-directional communication between the terminal-based Helix core and the GUI layer

### Dependency Architecture

- **Helix Integration**: Uses official helix crates (v25.07.1) for all editor functionality
- **GPUI Framework**: Zed's UI framework for GPU-accelerated rendering
- **Async Runtime**: Tokio for handling async operations alongside GPUI's reactive system

## Development Commands

### Building & Running

```bash
# Debug build
cargo build
cargo run

# Release build  
cargo build --release
./target/release/nucl

# macOS app bundle
./bundle-mac.sh
open Nucleotide.app

# With Nix (recommended for reproducible builds)
nix develop              # Enter dev shell
cargo build --release
make-macos-bundle        # macOS
make-linux-package       # Linux
```

### Code Quality

```bash
# Format code
cargo fmt

# Run linter
cargo clippy

# Type checking
cargo check

# Run tests
cargo test

# Run a single test
cargo test test_name
```

### Configuration

Nucleotide looks for configuration in this order:
1. `~/.config/helix/nucleotide.toml` - GUI-specific settings (fonts, UI preferences)
2. `~/.config/helix/config.toml` - Falls back to standard Helix configuration

## Critical Implementation Details

### Modal Editing Flow

1. GPUI captures keyboard/mouse events in `workspace.rs`
2. Events are converted to Helix `KeyEvent`s via `gpui_to_helix_bridge`
3. Helix processes the command and updates its internal state
4. Changes are communicated back via `event_bridge` 
5. UI components update reactively through GPUI's entity system

### Scroll Synchronization

The `ScrollManager` in `src/scroll_manager.rs` maintains scroll state between Helix's viewport and GPUI's rendering. It must stay synchronized with Helix's `view_offset` to ensure the cursor remains visible during editing.

### File Tree Integration

The file tree (`src/file_tree/`) is a custom implementation that:
- Uses `notify` for file system watching
- Integrates with Helix's file opening commands
- Maintains its own selection state separate from the editor

### Runtime Files

Helix runtime files (grammars, themes, queries) must be available at:
- Development: Fetched from cargo's git checkout directory
- App bundle: Embedded in `Contents/MacOS/runtime/`
- The `helix_loader` crate handles runtime file discovery

## Platform-Specific Considerations

### macOS
- Uses native titlebar with traffic light controls
- Bundle identifier: `org.spiralpoint.nucleotide`
- Requires code signing for distribution

### Linux  
- Requires `libxkbcommon-dev` and `libxkbcommon-x11-dev`
- Uses client-side window decorations

## Testing Approach

Tests are embedded in source files using `#[cfg(test)]` modules. Key test areas:
- Command system parsing (`command_system.rs`)
- Configuration loading (`config.rs`)
- File tree operations (`file_tree/`)
- UI component behavior (`ui/`)

## Important Notes

- **Never modify helix-* dependencies** - All editor logic comes from upstream Helix
- **Preserve Helix compatibility** - Configuration and keybindings must work identically
- **Event-driven architecture** - Use GPUI's reactive entity system, not polling
- **Focus management** - Critical for modal editing; handled primarily in `workspace.rs`