# V2 Event System Migration Plan

## Overview

This document provides a comprehensive implementation plan to perform a significant refactor of Nucleotide's internal communication, replacing the legacy channel-based system with a modern, domain-driven event architecture. This effort will eliminate the majority of channel usage, improving modularity, testability, and maintainability.

This is a **breaking change** with no backward compatibility. The entire migration will be developed on a feature branch.

## Guiding Principles

This migration is guided by three core architectural principles:

### 1. Redesign Events Around Bounded Contexts
We will deprecate and remove monolithic event enums like `CoreEvent` and `UiEvent`. The new architecture will be built on a hierarchy of event modules, each corresponding to a clear "bounded context" within the application. This enforces strong domain boundaries and prevents God-object enums.

*   **Example:**
    *   `events::document::Event`: `Created`, `TextChanged`, `Saved`, `Closed`
    *   `events::view::Event`: `SelectionChanged`, `CursorMoved`, `Scrolled`
    *   `events::app::Event`: `ThemeChanged`, `ConfigChanged`, `WindowFocused`

### 2. Design Immutable, Fact-Based Events
Every event will be an immutable struct representing a *fact* of something that *has happened* in the system. We will avoid events that act as commands. This makes event handlers easier to reason about, as they only react to state changes, and makes the event log a true source of truth for debugging.

*   **Example:** `document::Event::TextChanged { doc_id, new_text, old_text_range }` is a fact. This is preferable to a command-like event such as `RequestTextChange`.

### 3. Modular Architecture Through File Decomposition
The massive `Application.rs` (3,253 lines) and `Workspace.rs` (5,369 lines) files will be decomposed into focused, domain-specific modules that align perfectly with the new event boundaries. Each extracted module will implement exactly one `EventHandler<DomainEvent>`, creating clean separation of concerns and eliminating cross-cutting complexity.

*   **Example:**
    *   `application/document_manager.rs` implements `EventHandler<document::Event>`
    *   `workspace/view_manager.rs` implements `EventHandler<view::Event>`
    *   `application/lsp_coordinator.rs` implements `EventHandler<lsp::Event>`

---

## Phase 1: Core Application & UI Events + File Decomposition

This phase combines event system migration with strategic file decomposition, implementing both architectural improvements in parallel for maximum efficiency.

### Task 1.1: Event Bridge System Migration + Application Core Extraction
**Goal**: Replace `BridgedEvent` with new, domain-specific events while extracting core application components.

#### 1.1.1: Create Domain-Specific Document and View Events
- **File**: `crates/nucleotide-events/src/document.rs`, `crates/nucleotide-events/src/view.rs`
- **Action**: Create new, focused event enums. Deprecate `CoreEvent`.
```rust
// In document.rs
pub enum Event {
    TextChanged { doc_id: DocumentId, new_text: String, old_text_range: Range },
    Saved { doc_id: DocumentId },
    Closed { doc_id: DocumentId },
}

// In view.rs
pub enum Event {
    SelectionChanged { doc_id: DocumentId, view_id: ViewId, new_selection: Range },
    CursorMoved { doc_id: DocumentId, view_id: ViewId, new_position: Position },
    Scrolled { doc_id: DocumentId, view_id: ViewId, first_visible_line: usize },
}
```

#### 1.1.2: Extract Application Core Components
- **New File**: `crates/nucleotide/src/application/app_core.rs` (~400 lines)
- **Action**: Extract core Helix integration (Editor, Compositor, Jobs) from `Application.rs`
```rust
pub struct AppCore {
    pub editor: Editor,
    pub compositor: Compositor,
    pub jobs: Jobs,
    pub config: Config,
    pub project_directory: Option<PathBuf>,
}

impl EventHandler<app::Event> for AppCore {
    fn handle_event(&mut self, event: &app::Event, cx: &mut WindowContext<'_>) {
        match event {
            app::Event::ThemeChanged { theme_name } => self.handle_theme_change(theme_name, cx),
            app::Event::ConfigurationChanged { key, value } => self.handle_config_change(key, value, cx),
            // ... other app lifecycle events
        }
    }
}
```

#### 1.1.3: Extract Document Manager
- **New File**: `crates/nucleotide/src/application/document_manager.rs` (~500 lines)
- **Action**: Extract document operations and lifecycle management
```rust
pub struct DocumentManager {
    shell_env_cache: Arc<Mutex<ShellEnvironmentCache>>,
    project_environment: Arc<ProjectEnvironment>,
    open_documents: HashMap<DocumentId, DocumentInfo>,
}

impl EventHandler<document::Event> for DocumentManager {
    fn handle_event(&mut self, event: &document::Event, cx: &mut WindowContext<'_>) {
        match event {
            document::Event::Saved { doc_id } => self.handle_document_saved(*doc_id, cx),
            document::Event::Closed { doc_id } => self.handle_document_closed(*doc_id, cx),
            // ... other document event handlers
        }
    }
}
```

#### 1.1.4: Refactor Event Emitters
- **File**: `crates/nucleotide-core/src/event_bridge.rs`
- **Action**: Remove all `BridgedEvent` code, route to domain-specific EventHandlers
```rust
// Remove: pub fn send_bridged_event(...)
// Events now route directly to appropriate domain managers:
EventBus::dispatch(document::Event::Saved { doc_id });
EventBus::dispatch(view::Event::SelectionChanged { doc_id, view_id, new_selection });
```

#### 1.1.5: Update Application as Thin Coordinator
- **File**: `crates/nucleotide/src/application.rs` (reduced to ~500 lines)
- **Action**: Transform Application into coordination layer between domain managers
```rust
pub struct Application {
    app_core: AppCore,
    document_manager: DocumentManager,
    // Remove direct event handling - delegate to domain managers
}

// Application coordinates between managers via event emission
impl Application {
    pub fn handle_user_action(&mut self, action: UserAction, cx: &mut WindowContext<'_>) {
        match action {
            UserAction::SaveDocument => {
                EventBus::dispatch(document::Event::SaveRequested { doc_id: self.current_doc_id() });
            }
            // Coordinator delegates to domain events
        }
    }
}
```

### Task 1.2: GPUI-Helix Bridge Migration + LSP Extraction
**Goal**: Replace `GpuiToHelixEvent` with domain-specific events while extracting LSP coordination.

#### 1.2.1: Create Domain-Specific Application Events
- **File**: `crates/nucleotide-events/src/app.rs`
- **Action**: Create a new `app::Event` enum. Deprecate `UiEvent`.
```rust
pub enum Event {
    WindowResized { width: u32, height: u32 },
    WindowFocused,
    WindowBlurred,
    ThemeChanged { theme_name: String },
    FontSizeChanged { size: f32 },
    ConfigurationChanged { key: String, value: serde_json::Value },
}
```

#### 1.2.2: Extract LSP Coordinator
- **New File**: `crates/nucleotide/src/application/lsp_coordinator.rs` (~800 lines)
- **Action**: Extract LSP management and integration from `Application.rs`
```rust
pub struct LspCoordinator {
    lsp_manager: LspManager,
    project_lsp_manager: Arc<RwLock<Option<ProjectLspManager>>>,
    helix_lsp_bridge: Arc<RwLock<Option<HelixLspBridge>>>,
    server_states: HashMap<ServerId, ServerState>,
}

impl EventHandler<lsp::Event> for LspCoordinator {
    fn handle_event(&mut self, event: &lsp::Event, cx: &mut WindowContext<'_>) {
        match event {
            lsp::Event::ServerStarted { workspace_root, server_id } => {
                self.handle_server_started(workspace_root, *server_id, cx);
            },
            lsp::Event::ServerStopped { server_id } => {
                self.handle_server_stopped(*server_id, cx);
            },
            // ... other LSP event handlers
        }
    }
}
```

#### 1.2.3: Extract View Manager from Workspace
- **New File**: `crates/nucleotide/src/workspace/view_manager.rs` (~1,200 lines)
- **Action**: Extract document view coordination and focus management
```rust
pub struct ViewManager {
    documents: HashMap<ViewId, Entity<DocumentView>>,
    focused_view_id: Option<ViewId>,
    document_order: Vec<DocumentId>,
    focus_handle: FocusHandle,
}

impl EventHandler<view::Event> for ViewManager {
    fn handle_event(&mut self, event: &view::Event, cx: &mut ViewContext<Self>) {
        match event {
            view::Event::SelectionChanged { doc_id, view_id, new_selection } => {
                self.handle_selection_changed(*doc_id, *view_id, new_selection, cx);
            },
            view::Event::CursorMoved { doc_id, view_id, new_position } => {
                self.handle_cursor_moved(*doc_id, *view_id, *new_position, cx);
            },
            // ... other view event handlers
        }
    }
}
```

#### 1.2.4: Refactor Bridge to Use Application Events
- **File**: `crates/nucleotide-core/src/gpui_to_helix_bridge.rs`
- **Action**: Replace channel calls with event dispatch to AppCore
```rust
// Replace: self.tx.send(GpuiToHelixEvent::WindowResized { ... }).ok();
// With:
EventBus::dispatch(app::Event::WindowResized { width, height });
```

### Task 1.3: Completion Events + UI Component Extraction  
**Goal**: Replace `CompletionResult` channel with events while extracting UI rendering components.

#### 1.3.1: Create Domain-Specific Completion Events
- **File**: `crates/nucleotide-events/src/completion.rs`
- **Action**: Create a new `completion::Event` enum.
```rust
pub enum Event {
    Shown {
        items: Vec<CompletionItem>,
        cursor_pos: Position,
        doc_id: DocumentId,
        view_id: ViewId,
    },
    Hidden,
    Updated {
        items: Vec<CompletionItem>,
        selected_index: Option<usize>,
    },
}
```

#### 1.3.2: Extract Completion Coordinator  
- **New File**: `crates/nucleotide/src/application/completion_coordinator.rs` (~600 lines)
- **Action**: Extract completion system processing from `Application.rs`
```rust
pub struct CompletionCoordinator {
    completion_channels: CompletionChannels,
    lsp_completion_channels: LspCompletionChannels,
    active_completions: HashMap<DocumentId, CompletionState>,
}

impl EventHandler<completion::Event> for CompletionCoordinator {
    fn handle_event(&mut self, event: &completion::Event, cx: &mut WindowContext<'_>) {
        match event {
            completion::Event::Shown { items, cursor_pos, doc_id, view_id } => {
                self.handle_completion_shown(items, *cursor_pos, *doc_id, *view_id, cx);
            },
            completion::Event::Hidden => self.handle_completion_hidden(cx),
            // ... other completion event handlers
        }
    }
}
```

#### 1.3.3: Extract UI Renderer from Workspace
- **New File**: `crates/nucleotide/src/workspace/ui_renderer.rs` (~1,800 lines) 
- **Action**: Extract UI rendering logic (overlays, notifications, hints)
```rust
pub struct UiRenderer {
    overlay: Entity<OverlayView>,
    info: Entity<InfoBoxView>,
    key_hints: Entity<KeyHintView>,
    notifications: Entity<NotificationView>,
    completion_view: Option<Entity<CompletionView>>,
}

impl EventHandler<completion::Event> for UiRenderer {
    fn handle_event(&mut self, event: &completion::Event, cx: &mut ViewContext<Self>) {
        match event {
            completion::Event::Shown { items, cursor_pos, .. } => {
                self.show_completions(items.clone(), *cursor_pos, cx);
            },
            completion::Event::Hidden => self.hide_completions(cx),
            // ... other UI event handlers
        }
    }
}
```

#### 1.3.4: Update Workspace as UI Composition Layer
- **File**: `crates/nucleotide/src/workspace.rs` (reduced to ~800 lines)
- **Action**: Transform Workspace into UI composition and coordination
```rust
pub struct Workspace {
    view_manager: ViewManager,
    ui_renderer: UiRenderer,
    file_system_manager: FileSystemManager,
    // Workspace coordinates UI components via event emission
}

impl Workspace {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .child(self.view_manager.render(cx))
            .child(self.ui_renderer.render(cx))
            .child(self.file_system_manager.render_file_tree(cx))
    }
}
```

### Task 1.4: Integration Testing for Phase 1
**Goal**: Validate extracted modules work together through event system.

#### 1.4.1: Unit Test Domain Modules
- **Action**: Test each EventHandler implementation independently
- **Files**: `tests/application/`, `tests/workspace/`
- **Focus**: Event handling logic without cross-dependencies

#### 1.4.2: Integration Test Event Flows
- **Action**: Test end-to-end event flows between extracted modules  
- **Scenarios**: Document save → LSP notification → UI update
- **Validation**: Event ordering and data consistency

---

## Phase 2: Service & File System Events

### Task 2.1: LSP Event Broadcasting Migration
**Goal**: Redesign `LspEvent` and `ProjectLspEvent` into a single, cohesive domain.

#### 2.1.1: Redesign LSP Events
- **File**: `crates/nucleotide-events/src/lsp.rs`
- **Action**: Consolidate and redesign all LSP-related events into a single `lsp::Event` enum, following the bounded context principle.
```rust
pub enum Event {
    ServerStarted { workspace_root: PathBuf, server_id: ServerId },
    ServerStopped { server_id: ServerId },
    Notification(lsp_types::Notification),
    // ... other lsp events
}
```

### Task 2.2: File System Events + Final UI Extraction
**Goal**: Atomic file system events and complete Workspace decomposition.

#### 2.2.1: Create Atomic File System Events
- **File**: `crates/nucleotide-events/src/fs.rs`
- **Action**: Create atomic `fs::Event` for all file system changes.
```rust
pub enum EventKind {
    Created,
    Modified,
    Deleted,
    Renamed { from: PathBuf, to: PathBuf },
}

pub struct Event {
    pub path: PathBuf,
    pub kind: EventKind,
}
```

#### 2.2.2: Extract File System Manager
- **New File**: `crates/nucleotide/src/workspace/file_system_manager.rs` (~600 lines)
- **Action**: Extract file tree, project root detection, VCS integration
```rust
pub struct FileSystemManager {
    file_tree: Option<Entity<FileTreeView>>,
    show_file_tree: bool,
    current_project_root: Option<PathBuf>,
    vcs_info: Option<VcsInfo>,
    watcher: FileWatcher,
}

impl EventHandler<fs::Event> for FileSystemManager {
    fn handle_event(&mut self, event: &fs::Event, cx: &mut ViewContext<Self>) {
        match event.kind {
            fs::EventKind::Created => self.handle_file_created(&event.path, cx),
            fs::EventKind::Modified => self.handle_file_modified(&event.path, cx),
            fs::EventKind::Deleted => self.handle_file_deleted(&event.path, cx),
            fs::EventKind::Renamed { ref from, ref to } => {
                self.handle_file_renamed(from, to, cx);
            },
        }
    }
}
```

#### 2.2.3: Extract Input Coordinator
- **New File**: `crates/nucleotide/src/workspace/input_coordinator.rs` (~400 lines)
- **Action**: Extract focus management and input handling
```rust
pub struct InputCoordinator {
    focus_handle: FocusHandle,
    input_coordinator: Arc<InputCoordinator>,
    needs_focus_restore: bool,
    key_mapping: HashMap<KeyBinding, Action>,
}

impl EventHandler<input::Event> for InputCoordinator {
    fn handle_event(&mut self, event: &input::Event, cx: &mut ViewContext<Self>) {
        match event {
            input::Event::FocusChanged { old_focus, new_focus } => {
                self.handle_focus_change(old_focus, new_focus, cx);
            },
            input::Event::KeyPressed { key, modifiers } => {
                self.handle_key_press(key, modifiers, cx);
            },
            // ... other input event handlers
        }
    }
}
```

#### 2.2.4: Update Watcher Integration
- **File**: `crates/nucleotide/src/file_tree/watcher.rs`
- **Action**: Convert `notify::Event`s to atomic `fs::Event`s
```rust
impl FileWatcher {
    fn process_notify_events(&mut self, events: Vec<notify::Event>) {
        for notify_event in events {
            let fs_events = self.convert_to_fs_events(notify_event);
            for fs_event in fs_events {
                EventBus::dispatch(fs_event);
            }
        }
    }
}
```

---

## Phase 3: Integration and Cleanup

### Task 3.1: Remove Obsolete Infrastructure
- **Action**: Remove all legacy channel and event infrastructure, including unused `tokio::sync::mpsc` imports, old event enums (`CoreEvent`, `UiEvent`, etc.), and all channel-based communication logic.

### Task 3.2: Harden the EventBus
- **File**: `crates/nucleotide-core/src/event_bridge.rs`
- **Action**: Build performance-critical features directly into the `EventBus`.
- **Implementation**:
  - **Coalescing**: The `EventBus` should intelligently coalesce high-frequency events. For example, multiple `view::Event::Scrolled` events for the same view within a single frame should be reduced to a single dispatch of the latest event.
  - **Throttling/Debouncing**: Implement strategies for events like `document::Event::TextChanged` to prevent flooding consumers during rapid typing.

### Task 3.3: Create Comprehensive Event System Documentation
- **File**: `docs/event_system.md` (new comprehensive guide)
- **Action**: Create gold-standard documentation of the domain-driven event architecture with citations to event system best practices.

#### 3.3.1: Document Architectural Patterns
- **Domain-Driven Event Design**: Document bounded context approach with references to:
  - Eric Evans' "Domain-Driven Design" event modeling patterns
  - Martin Fowler's Event Sourcing patterns (https://martinfowler.com/eaaDev/EventSourcing.html)
  - Greg Young's CQRS/Event Sourcing best practices
- **Immutable Event Facts**: Document fact-based vs command-based events with references to:
  - Event Store design patterns (https://eventstore.com/blog/what-is-event-sourcing/)
  - Axon Framework event modeling guidelines

#### 3.3.2: Document Implementation Patterns
- **EventHandler<T> Pattern**: Document single-responsibility event handlers with references to:
  - Single Responsibility Principle (Clean Architecture patterns)
  - Observer Pattern implementations (Gang of Four Design Patterns)
- **Event Bus Architecture**: Document centralized dispatch with coalescing/throttling with references to:
  - Reactive Streams specification (https://www.reactive-streams.org/)
  - Actor Model event processing patterns

#### 3.3.3: Document Testing Strategies
- **Event-Driven Testing**: Document testing approaches with references to:
  - Test-Driven Development with events (Kent Beck patterns)
  - Event sourcing testing strategies (Vaughn Vernon's "Implementing Domain-Driven Design")
  - Chaos engineering for event systems (Netflix engineering practices)

#### 3.3.4: Update All Architecture Documentation
- **Files**: `CLAUDE.md`, inline code comments, module documentation
- **Action**: Update existing docs to reflect new event-driven, modular architecture
- **Focus**: Ensure architectural decisions are well-documented with rationale

---

## Testing Strategy

Following event sourcing and domain-driven design testing best practices to ensure the new architecture is robust and maintainable.

### Event-Driven Testing Principles
- **Event Fact Validation**: Test that events represent immutable facts with complete context
- **Domain Boundary Testing**: Validate that events don't leak across bounded contexts  
- **Event Handler Isolation**: Test each `EventHandler<T>` implementation independently
- **Event Sequence Integrity**: Ensure critical event orderings are preserved

### Testing Implementation Approach

#### Unit Testing (Domain Module Level)
- **EventHandler Testing**: Test each domain module's event handling in isolation
- **Event Creation Testing**: Validate event factory methods produce well-formed events
- **Business Logic Testing**: Test domain logic triggered by events without event system overhead

#### Integration Testing (Event Flow Level)  
- **End-to-End Workflows**: Validate complete user scenarios through event system
- **Cross-Domain Integration**: Test event flows between different bounded contexts
- **Event Bus Performance**: Validate coalescing, throttling, and high-frequency event handling

#### Chaos Testing (System Resilience Level)
- **High-Frequency Event Floods**: Test rapid typing, scrolling, and file system changes
- **Out-of-Order Event Delivery**: Validate system handles events arriving in unexpected sequences  
- **Event Handler Failure**: Test system resilience when individual handlers fail
- **Resource Exhaustion**: Test behavior under memory pressure and high event volumes

#### Property-Based Testing (Event Invariants)
- **Event Immutability**: Verify events cannot be modified after creation
- **Event Completeness**: Ensure events contain all necessary context for handlers
- **Domain Model Consistency**: Test that event sequences maintain valid domain state

### Testing Tools and Frameworks
- **Event Generators**: Create realistic event sequences for testing
- **Event Assertions**: Custom test helpers for validating event properties and sequences
- **Performance Benchmarks**: Measure event throughput and latency under various conditions
- **Integration Test Harness**: Framework for testing complete event-driven workflows

---

## Rollback Strategy

Given this is a breaking change developed on a feature branch, the rollback strategy is simplified:

1.  **Development Branch**: The entire migration will be developed on a `feat/event-system-v2` branch.
2.  **Merge**: The branch will only be merged into `main` after all phases are complete and all tests are passing.
3.  **Rollback**: If critical issues are discovered after the merge, the rollback mechanism is a clean **`git revert`** of the merge commit. This removes the need for complex feature flags or maintaining parallel systems.

---

## Success Criteria

### Functional Requirements
- ✅ All existing functionality preserved
- ✅ UI responsiveness maintained or improved
- ✅ Event ordering preserved for critical sequences

### Performance Requirements
- ✅ Event throughput meets or exceeds channel throughput
- ✅ Memory usage reduced by eliminating channel buffers
- ✅ UI latency maintained under high event load

### Architecture Requirements
- ✅ **New**: Event system follows Domain-Driven Design bounded context principles
- ✅ **New**: All events are immutable facts following Event Sourcing patterns  
- ✅ **New**: EventHandler<T> implementations follow Single Responsibility Principle
- ✅ **New**: Event Bus implements Reactive Streams patterns with coalescing/throttling
- ✅ **New**: Gold-standard documentation with cited industry best practices
- ✅ Reduced global state and coupling through modular architecture
- ✅ Simplified debugging with unified, domain-driven event tracing and comprehensive logging

---

## File Structure After Migration

The migration will result in this modular architecture:

```
src/
├── application.rs                     (~500 lines - thin coordinator)
├── workspace.rs                       (~800 lines - UI composition)
├── application/
│   ├── mod.rs
│   ├── app_core.rs                    (~400 lines - Helix integration)
│   ├── document_manager.rs            (~500 lines - Document lifecycle)
│   ├── lsp_coordinator.rs             (~800 lines - LSP management)
│   └── completion_coordinator.rs      (~600 lines - Completion processing)
└── workspace/
    ├── mod.rs
    ├── view_manager.rs                (~1,200 lines - Document views, focus)
    ├── ui_renderer.rs                 (~1,800 lines - Overlays, notifications)
    ├── file_system_manager.rs         (~600 lines - File tree, VCS)
    └── input_coordinator.rs           (~400 lines - Focus, input handling)
```

**Before**: 2 files, 8,622 lines  
**After**: 10 files, ~6,700 lines + 2 thin coordinators  
**Result**: ~22% reduction in total code + massive improvement in maintainability

## Implementation Timeline

### Phase 0: 1 week
- **Design**: Finalize event module structure and file decomposition plan
- **Setup**: Create branch `feat/event-system-v2` and module structure

### Phase 1: 3-4 weeks  
- **Week 1**: Task 1.1 (Document/View Events + AppCore/DocumentManager extraction)
- **Week 2**: Task 1.2 (App Events + LSP Coordinator + ViewManager extraction) 
- **Week 3**: Task 1.3 (Completion Events + CompletionCoordinator + UiRenderer extraction)
- **Week 4**: Task 1.4 (Integration testing and validation of Phase 1)

### Phase 2: 1-2 weeks
- **Week 1**: Task 2.1 (LSP Event consolidation)
- **Week 2**: Task 2.2 (File System Events + FileSystemManager + InputCoordinator extraction)

### Phase 3: 1-2 weeks
- **Week 1**: Task 3.1-3.2 (Remove obsolete infrastructure, harden EventBus with performance optimizations)
- **Week 2**: Task 3.3 (Create comprehensive `docs/event_system.md` with industry best practice citations)
- **Validation**: Performance testing and final integration validation

**Total Estimated Timeline: 6-9 weeks**

## Documentation Deliverables

### docs/event_system.md Structure
```markdown
# Nucleotide Event System Architecture

## Overview
Gold-standard implementation of domain-driven event architecture

## Architectural Patterns
### Domain-Driven Event Design (Citations: Evans DDD, Fowler Event Sourcing)
### Immutable Event Facts (Citations: Event Store, Axon Framework)
### EventHandler<T> Pattern (Citations: GoF Observer, Clean Architecture SRP)
### Event Bus with Reactive Streams (Citations: Reactive Streams Spec, Actor Model)

## Implementation Guide  
### Bounded Context Design
### Event Modeling Best Practices
### Performance Optimization Patterns
### Testing Strategies

## API Reference
### Event Type Definitions
### EventHandler Interface
### EventBus Methods
### Performance Monitoring

## Migration Guide
### From Channel-Based to Event-Driven
### Code Examples and Patterns
### Common Pitfalls and Solutions
```

This documentation will serve as a reference implementation for event-driven architecture in Rust applications.
