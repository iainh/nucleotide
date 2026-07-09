// ABOUTME: Core event bridges between Helix and GPUI
// ABOUTME: Provides the fundamental event translation layer

pub mod core_event;
pub mod document_manager;
pub mod event_aggregator;
pub mod event_bridge;
pub mod picker_capability;
pub mod preview_tracker;
pub mod snippets;
pub mod utils;

pub use picker_capability::PickerCapability;

// Re-export event types from nucleotide-events
pub use nucleotide_events::{
    EventBus, EventHandler, LspEvent, integration::Event as IntegrationEvent,
};

// Re-export V2 domain events
pub use nucleotide_events::v2::{
    document::Event as DocumentEvent, editor::Event as EditorEvent, lsp::Event as LspV2Event,
    run::Event as RunEvent, ui::Event as UiEvent, workspace::Event as WorkspaceEvent,
};

// Event aggregator exports (includes V2 AppEvent)
pub use event_aggregator::{AppEvent, EventAggregator, EventAggregatorHandle};

// Core event exports
pub use core_event::CoreEvent;

// Event bridge exports
pub use event_bridge::{
    BridgedEvent, BridgedEventReceiver, create_bridge_channel, initialize_bridge,
    register_event_hooks, send_bridged_event,
};

// Document manager exports
pub use document_manager::{DocumentManager, DocumentManagerMut};

// Snippet parsing exports
pub use snippets::{SnippetParseError, SnippetTemplate, Tabstop, TextPart};

// Re-export types from nucleotide-types for backward compatibility
pub use nucleotide_types::{
    CompletionTrigger, EditorFontConfig, EditorStatus, FontSettings, Severity, UiFontConfig,
};
