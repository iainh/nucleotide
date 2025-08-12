// ABOUTME: Core event bridges between Helix and GPUI
// ABOUTME: Provides the fundamental event translation layer

pub mod capabilities;
pub mod command_system;
pub mod document_manager;
pub mod editor_capabilities;
pub mod event_aggregator;
pub mod event_bridge;
pub mod gpui_to_helix_bridge;
pub mod preview_tracker;
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
    AppEvent, CoreEvent, EventBus, EventHandler, LspEvent, MessageSeverity, PanelType, PickerType,
    SplitDirection, UiEvent, WorkspaceEvent,
};

// Event aggregator exports
pub use event_aggregator::{EventAggregator, EventAggregatorHandle};

// Event bridge exports
pub use event_bridge::{
    create_bridge_channel, initialize_bridge, register_event_hooks, send_bridged_event,
    BridgedEvent, BridgedEventReceiver, CompletionTrigger,
};

// GPUI to Helix bridge exports
pub use gpui_to_helix_bridge::{
    create_gpui_to_helix_channel, handle_gpui_event_in_helix, initialize_gpui_to_helix_bridge,
    register_gpui_event_handlers, send_gpui_event_to_helix, GpuiToHelixEvent, MemoryPressureLevel,
};

// Document manager exports
pub use document_manager::{DocumentManager, DocumentManagerMut};

// Command system exports
pub use command_system::{Command, ParsedCommand};

// Re-export types from nucleotide-types for backward compatibility
pub use nucleotide_types::{
    CoreEntity, EditorFontConfig, EditorStatus, FontSettings, Severity, UiFontConfig,
};
