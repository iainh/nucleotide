# Helix Event System Integration Analysis

## Overview

This document analyzes Helix's event system architecture and identifies opportunities to improve our integration in helix-gpui.

## Helix's Event System Architecture

### Core Components

1. **Event Declaration** (`helix-event` crate)
   - Uses `events!` macro to declare typed events with lifetime parameters
   - Events are structs with specific data payloads
   - Example: `DocumentDidChange<'a> { doc: &'a mut Document, view: ViewId, old_text: &'a Rope, changes: &'a ChangeSet, ghost_transaction: bool }`

2. **Event Registration**
   - Events must be registered via `helix_event::register_event::<EventType>()`
   - Registration happens in `helix_term::events::register()`

3. **Event Dispatch**
   - Events dispatched via `helix_event::dispatch(event)`
   - Happens at key points in editor lifecycle (document changes, mode switches, etc.)

4. **Hook System**
   - Synchronous hooks: `register_hook!(move |event: &mut EventType| { ... })`
   - Async hooks: `AsyncHook` for expensive background operations
   - Hooks can modify editor state immediately or send to async channels

### Key Events

From `helix_view::events`:
- `DocumentDidOpen` - When documents are opened
- `DocumentDidChange` - When document content changes
- `DocumentDidClose` - When documents are closed
- `SelectionDidChange` - When cursor/selection moves
- `DiagnosticsDidChange` - When LSP diagnostics update
- `LanguageServerInitialized/Exited` - LSP lifecycle
- `ConfigDidChange` - When configuration changes

From `helix_term::events`:
- `OnModeSwitch` - When editor mode changes (normal/insert/etc.)
- `PostInsertChar` - After character insertion
- `PostCommand` - After command execution

## Current helix-gpui Integration

### What We Do Correctly

1. ✅ **Event Registration**: We call `helix_term::events::register()` in `application.rs:884`
2. ✅ **Handler Setup**: We create `Handlers` struct with async channels
3. ✅ **Basic Integration**: We pass handlers to `Editor::new()`

### Critical Missing Pieces

1. ❌ **Handler Hook Registration**: We don't call `helix_view::handlers::register_hooks(&handlers)`
2. ❌ **Async Handler Spawning**: We don't spawn async handlers that listen to channels
3. ❌ **Event Bridging**: We don't bridge Helix events to GPUI events
4. ❌ **Handler-Specific Hooks**: We don't register hooks for completion, diagnostics, etc.

## Specific Improvement Opportunities

### 1. Missing Handler Registration (CRITICAL)

**Issue**: We create handlers but don't register their hooks.

**In helix-term**:
```rust
pub fn setup(config: Arc<ArcSwap<Config>>) -> Handlers {
    // ... create handlers ...
    
    helix_view::handlers::register_hooks(&handlers);
    completion::register_hooks(&handlers);
    signature_help::register_hooks(&handlers);
    auto_save::register_hooks(&handlers);
    diagnostics::register_hooks(&handlers);
    snippet::register_hooks(&handlers);
    document_colors::register_hooks(&handlers);
    
    handlers
}
```

**What we need**: Add similar registration calls in our `build_application()` function.

### 2. Event-Driven UI Updates

**Current**: We manually update UI in response to our custom `Update` enum.

**Improvement**: Listen to Helix events and automatically update UI:
- `DocumentDidChange` → Update document views
- `SelectionDidChange` → Update cursor position, status
- `DiagnosticsDidChange` → Update diagnostic displays  
- `OnModeSwitch` → Update mode indicator, key hints

### 3. Missing Async Handlers

**Issue**: We create channels but don't spawn the async handlers that process them.

**Need to add**:
```rust
// In application.rs, after creating handlers
let completion_handler = CompletionHandler::new(config).spawn();
let signature_handler = SignatureHelpHandler::new().spawn();
let auto_save_handler = AutoSaveHandler::new().spawn();
let diagnostics_handler = DiagnosticsHandler::new().spawn();
```

### 4. Event Bridging Pattern

**Concept**: Create a bridge between Helix events and GPUI events.

**Example Implementation**:
```rust
// Register hooks that emit GPUI events
register_hook!(move |event: &mut DocumentDidChange| {
    // Convert to GPUI event
    cx.emit(Update::EditorEvent(EditorEvent::DocumentChanged {
        doc_id: event.doc.id(),
        changes: event.changes.clone()
    }));
    Ok(())
});
```

### 5. Improved LSP Integration

**Current**: Basic LSP message handling in notifications.

**Improvement**: 
- Listen to `LanguageServerInitialized` to show connection status
- Listen to `DiagnosticsDidChange` for real-time diagnostic updates
- Use signature help and completion handlers properly

## Recommended Implementation Plan

### Phase 1: Core Handler Registration (High Priority)

1. Add handler hook registration calls to `build_application()`
2. Spawn async handlers to process channels
3. Test that basic LSP functionality works

### Phase 2: Event-Driven UI Updates (Medium Priority)

1. Create event bridge from Helix events to GPUI Update enum
2. Register hooks for key events (document changes, selection, mode switch)
3. Update UI components to respond to these events

### Phase 3: Advanced Features (Low Priority)

1. Implement proper completion UI integration
2. Add signature help display
3. Enhance diagnostic presentation
4. Add auto-save functionality

## Code Examples

### Missing Handler Registration Fix

```rust
// In build_application(), after creating handlers and editor:

// THIS IS WHAT WE'RE MISSING:
helix_view::handlers::register_hooks(&handlers);

// We should also spawn async handlers similar to helix-term:
// let completion_handler = CompletionHandler::new(config).spawn();
// let signature_handler = SignatureHelpHandler::new().spawn();
// etc.
```

### Event Bridge Example

```rust
// Add this after event registration:
register_hook!(move |event: &mut DocumentDidChange| {
    // Emit GPUI event when document changes
    // This would require access to GPUI context - needs more design
    Ok(())
});
```

## Impact Assessment

### Benefits of Proper Integration

1. **Automatic LSP Features**: Completion, diagnostics, signature help work out of the box
2. **Responsive UI**: UI updates immediately on editor state changes
3. **Feature Parity**: Closer to terminal helix functionality
4. **Maintainability**: Less custom event handling code

### Risks

1. **Complexity**: More moving parts to debug
2. **Performance**: Need to ensure event bridge doesn't cause performance issues
3. **GPUI Integration**: Need to carefully bridge async Helix events with GPUI's synchronous event model

## Conclusion

Our current integration captures basic Helix functionality but misses the rich event-driven architecture that enables advanced features. The most critical missing piece is handler hook registration, which would immediately enable LSP features like completion and diagnostics to work properly.

The recommended approach is to start with Phase 1 (handler registration) as it provides immediate benefits with minimal risk, then gradually enhance the event bridging for more responsive UI updates.