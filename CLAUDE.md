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

## Logging System

Nucleotide uses a centralized structured logging system built on `tokio-tracing` instead of the standard `log` crate. This provides better observability, performance monitoring, and debugging capabilities.

### Architecture

- **`nucleotide-logging` crate**: Centralized logging infrastructure with structured tracing
- **File logging**: Daily rotating logs with configurable retention
- **Console output**: Pretty-printed structured logs for development
- **Hot reloading**: Runtime log level updates without restart
- **Performance monitoring**: Built-in timing and profiling capabilities

### Log File Locations

Logs are written to platform-specific directories:
- **macOS**: `~/Library/Application Support/nucleotide/nucleotide.log.YYYY-MM-DD`
- **Linux**: `~/.config/nucleotide/nucleotide.log.YYYY-MM-DD`
- **Windows**: `%APPDATA%/nucleotide/nucleotide.log.YYYY-MM-DD`

Note: Files include date suffixes due to daily rotation (e.g., `nucleotide.log.2025-08-13`).

### Usage in Code

**Always use structured logging with fields instead of format strings:**

```rust
// ✅ Correct - structured logging
use nucleotide_logging::{debug, info, warn, error, instrument};

info!(file_path = %path.display(), "Opening document");
warn!(error = %e, retry_count = retries, "Failed to connect, retrying");
error!(doc_id = ?doc_id, line = line_num, "Invalid cursor position");

// ✅ Function instrumentation for automatic tracing
#[instrument(skip(self, large_param))]
pub fn process_document(&self, doc_id: DocumentId, large_param: &LargeStruct) {
    // Function entry/exit automatically logged with arguments
}

// ❌ Incorrect - avoid format strings
info!("Opening document: {}", path.display()); // Don't do this
```

**Field formatting guidelines:**
- `%` for Display formatting: `%path.display()`, `%error`
- `?` for Debug formatting: `?doc_id`, `?selection`
- Direct values for primitives: `count = 42`, `enabled = true`

### Performance Monitoring

Use the built-in performance monitoring for critical operations:

```rust
use nucleotide_logging::{timed, PerfTimer};

// Automatic timing with warning threshold
fn process_large_file(&self, path: &Path) -> Result<()> {
    timed!("process_large_file", warn_threshold: Duration::from_millis(100), {
        // Your code here
        self.do_expensive_operation(path)
    })
}

// Manual timing with custom fields
fn complex_operation(&self) -> Result<()> {
    let _timer = PerfTimer::new("complex_operation")
        .with_field("items", self.items.len())
        .start();
    
    // Your code here
    Ok(())
}
```

### Environment Configuration

Control logging behavior with environment variables:
- `NUCLEOTIDE_LOG=debug` - Set global log level
- `RUST_LOG=nucleotide_core=trace,nucleotide_lsp=debug` - Module-specific levels
- `NUCLEOTIDE_LOG_NO_FILE=1` - Disable file logging
- `NUCLEOTIDE_LOG_NO_CONSOLE=1` - Disable console output
- `NUCLEOTIDE_LOG_JSON=1` - Output structured JSON logs

### Migration from log:: crate

When updating existing code:
1. Replace `log::{debug, info, warn, error}` imports with `nucleotide_logging::{debug, info, warn, error}`
2. Convert format strings to structured fields
3. Add `#[instrument]` to important functions
4. Use performance monitoring for expensive operations

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