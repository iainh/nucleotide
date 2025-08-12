# Nucleotide Event System Architecture

## Overview

Nucleotide uses a hybrid event system that combines:
1. **Event-based messaging** for data-only events (via `AppEvent`)
2. **Direct component passing** for complex UI components with behavior

## Why a Hybrid Approach?

Pure event systems work well for data-only events but have limitations when dealing with UI components that contain behavior (closures, callbacks). Components like prompts and pickers need to carry their interaction logic with them, which cannot be easily serialized into events.

## Event Categories

### Data-Only Events (via `AppEvent`)

These events carry only data and are routed through the `Event(AppEvent)` variant:

- **CoreEvent**: Editor state changes (document changes, selection, mode, diagnostics)
- **WorkspaceEvent**: File/directory operations, tab management, splits
- **UiEvent**: Theme changes, UI state updates, simple notifications
- **LspEvent**: Language server lifecycle and progress

Example:
```rust
cx.emit(Update::Event(AppEvent::Core(CoreEvent::DocumentChanged { doc_id })));
```

### Complex UI Components (Direct Variants)

These components contain behavior (closures) and are passed directly:

- **Prompt**: Contains submit/cancel callbacks
- **Picker**: Contains selection callbacks
- **DirectoryPicker**: Contains directory selection callbacks
- **Completion**: Entity reference to completion view
- **Info**: Information display component

Example:
```rust
let prompt = Prompt::native("Search:", "", |input| {
    // Handle search submission
});
cx.emit(Update::Prompt(prompt));
```

## Migration Status

### Completed
- ✅ Core event system (`AppEvent`, `CoreEvent`, `UiEvent`, etc.)
- ✅ Transitional `Update` enum supporting both patterns
- ✅ Workspace handler for both event types
- ✅ Event emissions migrated where possible

### In Progress
- 🔄 Gradual migration of legacy Update variants to Event(AppEvent)
- 🔄 Identifying components that can be converted to data-only events

### Future Work
- 📋 Consider command pattern for behavior serialization
- 📋 Capability traits for component interfaces
- 📋 Further modularization of crates

## Event Flow

```
Application/Editor Events
         ↓
    Update Enum
    /         \
Event(AppEvent)  Direct Components
       |              |
   Data Events    UI with Behavior
       |              |
   Workspace      Overlay View
   Handler        Handler
```

## Guidelines for New Events

1. **Use `Event(AppEvent)`** when:
   - Event contains only data
   - No callbacks or closures needed
   - Event can be serialized

2. **Use Direct Update Variants** when:
   - Component needs callbacks
   - Component contains closures
   - Component has complex behavior

3. **Consider Alternatives** for:
   - Commands that could use IDs instead of closures
   - Components that could be refactored to separate data from behavior

## Example: Adding a New Event

### Data-Only Event
```rust
// 1. Add to events.rs
pub enum CoreEvent {
    // ...
    MyNewEvent { data: String },
}

// 2. Emit the event
cx.emit(Update::Event(AppEvent::Core(CoreEvent::MyNewEvent { 
    data: "example".to_string() 
})));

// 3. Handle in workspace
match event {
    AppEvent::Core(CoreEvent::MyNewEvent { data }) => {
        self.handle_my_new_event(data, cx);
    }
    // ...
}
```

### Component with Behavior
```rust
// 1. Keep as direct Update variant
pub enum Update {
    // ...
    MyComponent(MyComponentType),
}

// 2. Emit directly
cx.emit(Update::MyComponent(component));

// 3. Handle in appropriate view (usually OverlayView)
match ev {
    Update::MyComponent(component) => {
        self.handle_my_component(component, cx);
    }
    // ...
}
```

## Benefits of This Approach

1. **Flexibility**: Supports both simple data events and complex UI components
2. **Gradual Migration**: Allows incremental refactoring without breaking changes
3. **Type Safety**: Maintains Rust's type safety for all event types
4. **Performance**: Avoids unnecessary serialization of complex components
5. **Maintainability**: Clear separation between data and behavior

## Known Limitations

1. **Circular Dependencies**: Some modules must remain in the main crate due to dependencies
2. **Serialization**: Components with behavior cannot be easily serialized for IPC or network transport
3. **Testing**: Complex components with closures are harder to test in isolation

## Future Improvements

1. **Command Pattern**: Replace closures with command IDs for better serialization
2. **Capability Traits**: Define interfaces for components to enable better modularization
3. **Event Bus**: Consider a more sophisticated event bus with filtering and priority
4. **Async Events**: Support for async event handlers where appropriate