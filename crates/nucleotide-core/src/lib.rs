// ABOUTME: Core event bridges between Helix and GPUI
// ABOUTME: Provides the fundamental event translation layer

pub mod capabilities;
pub mod command_system;
pub mod core_event;
pub mod document_manager;
pub mod editor_capabilities;
pub mod event_aggregator;
pub mod event_bridge;
pub mod fs;
pub mod gpui_to_helix_bridge;
pub mod preview_tracker;
pub mod snippets;
pub mod utils;

// Capability traits exports
pub use capabilities::{
    BufferStore, CapabilityProvider, CommandExecutor, EditorReadAccess, EditorState,
    EditorWriteAccess, EventEmitter, EventSubscriber, JobSystemAccess, LineCache, LineLayoutInfo,
    OverlayProvider, ScrollManager, SubscriptionId, ThemeProvider, ViewStore,
};

// Extended editor capabilities exports
pub use editor_capabilities::{
    CommandExecution, DocumentAccess, EditorAccess, EditorCapabilities, StatusInfo, ViewManagement,
    WeakEditorCapabilities,
};

// Re-export event types from nucleotide-events
pub use nucleotide_events::{
    EventBus, EventHandler, LspEvent, integration::Event as IntegrationEvent,
};

// Re-export V2 domain events
pub use nucleotide_events::v2::{
    document::Event as DocumentEvent, editor::Event as EditorEvent, lsp::Event as LspV2Event,
    ui::Event as UiEvent, workspace::Event as WorkspaceEvent,
};

// Event aggregator exports (includes V2 AppEvent)
pub use event_aggregator::{AppEvent, EventAggregator, EventAggregatorHandle};

// Core event exports
pub use core_event::CoreEvent;

// Event bridge exports
pub use event_bridge::{
    BridgedEvent, BridgedEventReceiver, CompletionTrigger, LspCommandDispatcher,
    create_bridge_channel, initialize_bridge, register_event_hooks, send_bridged_event,
};

// GPUI to Helix bridge exports
pub use gpui_to_helix_bridge::{
    GpuiToHelixEvent, MemoryPressureLevel, create_gpui_to_helix_channel,
    handle_gpui_event_in_helix, initialize_gpui_to_helix_bridge, register_gpui_event_handlers,
    send_gpui_event_to_helix,
};

// Document manager exports
pub use document_manager::{DocumentManager, DocumentManagerMut};

// Command system exports
pub use command_system::{Command, ParsedCommand};

// Snippet parsing exports
pub use snippets::{SnippetParseError, SnippetTemplate, Tabstop, TextPart};

// Re-export types from nucleotide-types for backward compatibility
pub use nucleotide_types::{EditorFontConfig, EditorStatus, FontSettings, Severity, UiFontConfig};
