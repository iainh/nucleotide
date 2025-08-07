# Helix-GPUI View Hierarchy and Communication Patterns

## Overview

Helix-GPUI implements a layered view architecture built on top of the GPUI framework, bridging Helix's terminal-based editor with a native GUI. This document describes the component hierarchy, communication patterns, and data flow throughout the application.

## View Hierarchy

### 1. Window Structure

```
Window
└── Workspace (root view)
    ├── TitleBar (optional, platform-dependent)
    │   └── PlatformTitleBar
    │       ├── Window Controls (minimize, maximize, close)
    │       └── Title Text (current filename)
    │
    ├── Content Area (horizontal flex)
    │   ├── FileTreeView (optional, resizable)
    │   │   ├── File Tree List (uniform_list)
    │   │   └── Scrollbar
    │   │
    │   ├── Resize Handle (4px draggable divider)
    │   │
    │   └── Main Content (vertical flex)
    │       ├── Document Area
    │       │   └── DocumentView(s) (one per split)
    │       │       └── DocumentElement (text rendering)
    │       │
    │       ├── NotificationView (stacked notifications)
    │       │   └── Individual Notification Items
    │       │
    │       ├── OverlayView (modal layer)
    │       │   ├── PromptView (command palette, input prompts)
    │       │   ├── PickerView (file/buffer picker)
    │       │   └── CompletionView (LSP completions)
    │       │
    │       ├── InfoBoxView (contextual information)
    │       │
    │       └── KeyHintView (key binding hints)
```

### 2. Component Responsibilities

#### **Workspace** (`src/workspace.rs`)
- Root orchestrator for all UI components
- Manages document view lifecycle
- Handles window-level events and actions
- Controls layout (file tree visibility, splits)
- Focus management and restoration
- Theme application

#### **DocumentView** (`src/document.rs`)
- Individual editor view for each split
- Text rendering via DocumentElement
- Scroll handling
- Cursor and selection display
- Syntax highlighting
- Diagnostics display

#### **FileTreeView** (`src/file_tree/view.rs`)
- File system navigation
- VCS status integration
- Keyboard and mouse interaction
- Lazy loading of directories
- File watching for updates

#### **OverlayView** (`src/overlay.rs`)
- Modal overlay management
- Command palette (PromptView)
- File/buffer picker (PickerView)
- Completion popups (CompletionView)
- Focus trapping when active

#### **NotificationView** (`src/notification.rs`)
- Status messages
- Error alerts
- Progress indicators
- Auto-dismissal with timers

#### **TitleBar** (`src/titlebar/mod.rs`)
- Custom window decorations
- Platform-specific controls
- Dynamic title updates

## Communication Patterns

### 1. Event Bridge System

The application uses a channel-based event bridge to forward Helix events to GPUI:

```
Helix Core Events
    ↓
Event Bridge (src/event_bridge.rs)
    ↓ (via mpsc channel)
BridgedEvent enum
    ↓
Application::handle_bridged_event
    ↓
Update Event emission
    ↓
Workspace/View handlers
```

#### Bridged Event Types:
- `DocumentChanged` - Text content modifications
- `SelectionChanged` - Cursor/selection updates
- `ModeChanged` - Normal/Insert/Visual mode switches
- `DiagnosticsChanged` - LSP diagnostic updates
- `DocumentOpened/Closed` - Buffer lifecycle
- `ViewFocused` - Split focus changes
- `LanguageServerInitialized/Exited` - LSP lifecycle
- `CompletionRequested` - Trigger completions

### 2. Update Event Flow

```rust
// Central update enum propagated through the view hierarchy
pub enum Update {
    Overlays(Vec<Overlay>),
    Prompt(Prompt),
    Native(PickerOverlay),
    NativePrompt { /* ... */ },
    Completion(CompletionOverlay),
    // ... other update types
}
```

**Event Flow:**
1. Input received by `Input` entity
2. Processed by `Application` (Core)
3. Helix editor processes command
4. Events generated and bridged
5. `Update` events emitted
6. Workspace receives and distributes
7. Views update reactively

### 3. Entity Subscription Model

Components use GPUI's entity subscription system:

```rust
// Example: Workspace subscribing to Core updates
cx.subscribe(&core, |workspace, _core, event: &Update, cx| {
    workspace.handle_event(event, cx);
}).detach();

// Example: Workspace subscribing to FileTree events
cx.subscribe(&file_tree, |workspace, _file_tree, event, cx| {
    workspace.handle_file_tree_event(event, cx);
}).detach();
```

### 4. Focus Management

Focus is managed through GPUI's `FocusHandle` system:

```
Workspace (focus_handle)
    ├── Can focus DocumentView(s)
    ├── OverlayView takes focus when active
    ├── FileTreeView has independent focus
    └── Focus restoration after overlay dismissal
```

**Focus Flow:**
1. User action triggers focus change
2. Window.focus(&handle) called
3. Component receives focus
4. Key events routed to focused component
5. Visual feedback updated (cursors, selections)

## Data Flow Patterns

### 1. Input Processing Pipeline

```
KeyEvent/MouseEvent
    ↓
Workspace.handle_key() or mouse handlers
    ↓
Input.add_keypress() / mouse position updates
    ↓
Application polls input
    ↓
Helix processes commands
    ↓
Document/View state changes
    ↓
Events bridged back to UI
    ↓
Views re-render
```

### 2. Theme System

```
ThemeManager (global)
    ├── Helix themes (syntax highlighting)
    ├── UI themes (GPUI components)
    └── Theme conversion utilities

Access Pattern:
cx.global::<ThemeManager>().helix_theme()
```

### 3. File Operations

```
FileTreeView interaction
    ↓
FileTreeEvent::FileSelected
    ↓
Workspace.handle_file_tree_event()
    ↓
Application.open_file()
    ↓
Helix editor.open()
    ↓
Document created
    ↓
DocumentView instantiated
    ↓
View added to splits
```

### 4. Completion Flow

```
Character typed
    ↓
Helix triggers completion
    ↓
LSP request initiated
    ↓
CompletionRequested event bridged
    ↓
Update::Completion emitted
    ↓
OverlayView shows CompletionView
    ↓
User selects completion
    ↓
Text inserted via Helix
```

## Rendering Pipeline

### 1. Frame Generation

```
GPUI Frame Start
    ↓
Workspace.render() called
    ├── Focus restoration check
    ├── Document view collection
    ├── Theme application
    ├── Layout calculation
    └── Component tree assembly
        ↓
Child components render
    ├── TitleBar.render()
    ├── FileTreeView.render()
    ├── DocumentView.render()
    │   └── DocumentElement layout
    ├── OverlayView.render()
    └── Other views render
        ↓
GPUI composites and presents frame
```

### 2. Incremental Updates

- Views call `cx.notify()` to trigger re-render
- GPUI diffs component trees
- Only changed elements re-rendered
- Virtual scrolling in FileTreeView via `uniform_list`

## Extension Guidelines

### Adding a New View Component

1. Create struct implementing `Render` trait
2. Add to Workspace as Entity field
3. Subscribe to relevant events
4. Add to render hierarchy
5. Handle focus if interactive

### Adding New Event Types

1. Add variant to `BridgedEvent` enum
2. Register hook in `register_event_hooks()`
3. Handle in `Application::handle_bridged_event()`
4. Add Update variant if UI needs notification
5. Handle in relevant view components

### Communication Best Practices

1. **Use subscriptions for reactive updates** - Don't poll for changes
2. **Emit events at appropriate granularity** - Balance between too many and too few updates
3. **Handle cleanup in drop handlers** - Unsubscribe and release resources
4. **Use weak references to prevent cycles** - Especially for back-references to parent entities
5. **Keep rendering pure** - Don't modify state in render methods

## Performance Considerations

1. **Virtual scrolling** - FileTreeView uses `uniform_list` for large directories
2. **Lazy loading** - File tree loads directories on-demand
3. **Debounced file watching** - Prevents excessive updates
4. **Cached line layouts** - DocumentView caches text layouts
5. **Incremental syntax highlighting** - Only visible lines highlighted

## Debugging Tips

1. **Enable verbose logging**: Set `RUST_LOG=debug` or `trace`
2. **Monitor event flow**: Log in event bridge and handlers
3. **Check focus state**: Use `cx.focused()` to debug focus issues
4. **Inspect render calls**: Add logging to `render()` methods
5. **Use GPUI inspector**: Built-in tools for component tree inspection

## Architecture Decisions

### Why Event Bridge?

Helix's event system is callback-based, while GPUI is reactive. The bridge provides:
- Decoupling of core editor from UI
- Async event handling
- Batching and deduplication
- Clean separation of concerns

### Why Entity/Subscription Model?

- Natural fit for GPUI's reactive paradigm
- Automatic cleanup on entity drop
- Type-safe event handling
- Efficient update propagation

### Why Overlay System?

- Modal UI patterns (command palette, pickers)
- Focus management simplification
- Consistent dismiss behavior
- Clean visual hierarchy

## Future Enhancements

1. **Plugin System** - Allow third-party view components
2. **Split Management UI** - Visual split creation/management  
3. **Floating Windows** - Detachable panels and windows
4. **Enhanced Theming** - More granular UI theme control
5. **Performance Monitoring** - Built-in frame time analysis