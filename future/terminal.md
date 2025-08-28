# Implementation Plan: Adding Embedded Terminal to Nucleotide

## Overview
Add an embedded terminal to Nucleotide similar to Zed's implementation, using `alacritty_terminal` as the foundation for terminal emulation while integrating with GPUI for rendering and Helix's modal paradigm.

## Phase 1: Core Terminal Infrastructure

### 1.1 Add Terminal Dependencies
- Add `alacritty_terminal` to Cargo.toml dependencies
- Add platform-specific PTY dependencies (libc for Unix, windows for Windows)
- Add required async/channel dependencies

### 1.2 Create Terminal Module Structure
```
src/terminal/
├── mod.rs              # Main terminal module
├── terminal.rs         # Core Terminal struct wrapping alacritty_terminal
├── pty.rs             # PTY management and process spawning
├── event_bridge.rs    # Terminal <-> GPUI event translation
└── mappings.rs        # Key/mouse event mapping
```

### 1.3 Implement Core Terminal Component
- Create `Terminal` struct wrapping `alacritty_terminal::Term`
- Implement PTY spawning using `alacritty_terminal::tty`
- Set up event loop for terminal I/O processing
- Handle terminal resize events

## Phase 2: GPUI Terminal View

### 2.1 Create Terminal View Component
```
src/terminal_view/
├── mod.rs                    # Terminal view module
├── terminal_view.rs          # Main TerminalView implementing Render
├── terminal_element.rs       # GPUI element for terminal rendering
└── terminal_renderer.rs      # Optimized text batching and rendering
```

### 2.2 Implement Terminal Rendering
- Create `TerminalElement` for GPUI rendering
- Implement text batching (like Zed's `BatchedTextRun`)
- Handle ANSI colors and text styles
- Implement cursor rendering with blink support
- Add scrollbar support

### 2.3 Input Event Handling
- Map GPUI keyboard events to terminal input
- Handle mouse events (selection, scrolling, clicks)
- Support copy/paste operations
- Integrate with Helix's modal system (Normal/Insert modes)

## Phase 3: Terminal Panel Integration

### 3.1 Create Terminal Panel
```
src/terminal_panel/
├── mod.rs          # Terminal panel module
├── panel.rs        # TerminalPanel struct
└── actions.rs      # Terminal-specific actions
```

### 3.2 Workspace Integration
- Add terminal panel as a dockable panel (bottom/right)
- Implement panel resizing and toggling
- Add keybindings for terminal operations
- Support multiple terminal tabs

### 3.3 Helix Integration
- Add terminal commands to Helix command system
- Support `:terminal` command to open terminal
- Add terminal-specific keymaps (e.g., `<space>t` for terminal toggle)
- Handle focus switching between editor and terminal

## Phase 4: Advanced Features

### 4.1 Shell Configuration
- Support custom shell selection
- Environment variable management
- Working directory detection from current file
- Shell integration for better CWD tracking

### 4.2 Terminal Features
- Search within terminal output
- Hyperlink detection and clicking
- Vi mode support for terminal navigation
- Split terminal support

### 4.3 Helix-Specific Enhancements
- Send selected text to terminal
- Run current file in terminal
- Task runner integration (cargo commands, etc.)
- Terminal command history

## Implementation Strategy

1. **Start Small**: Begin with basic terminal rendering and PTY management
2. **Incremental Integration**: Add features progressively while maintaining stability
3. **Platform Support**: Focus on macOS/Linux first, then Windows
4. **Performance**: Use text batching and GPU acceleration from the start
5. **Testing**: Add tests for terminal operations and event handling

## Key Technical Decisions

- Use `alacritty_terminal` for proven terminal emulation
- Leverage GPUI's text rendering system for performance
- Keep terminal as a separate module for maintainability
- Integrate with Helix's existing command and modal systems
- Store terminal configuration in Nucleotide's config file

## Estimated Complexity

- **Phase 1**: Medium - Core terminal infrastructure setup
- **Phase 2**: High - GPUI rendering and event handling
- **Phase 3**: Medium - Workspace integration
- **Phase 4**: Low-Medium - Additional features

## Technical Details from Zed Analysis

### Key Dependencies from Zed
- `alacritty_terminal` - Core terminal emulation engine
- Platform-specific PTY handling via `alacritty_terminal::tty`
- Event loop management for terminal I/O

### Architecture Insights
1. **Two-crate design**: Separate `terminal` (core) and `terminal_view` (UI) crates
2. **Event Bridge Pattern**: Bidirectional event translation between terminal and UI
3. **Text Batching**: Combine adjacent cells with same style for efficient rendering
4. **GPU Acceleration**: Leverage GPUI's text rendering for performance

### Critical Implementation Points
- Terminal state wrapped in `Arc<FairMutex<Term>>` for thread safety
- Event loop runs on separate thread, communicates via channels
- Scroll synchronization between terminal viewport and UI
- Modal editing considerations for terminal focus/input modes

This plan provides a solid foundation for adding terminal support to Nucleotide while maintaining compatibility with Helix's modal editing paradigm and leveraging GPUI's rendering capabilities.