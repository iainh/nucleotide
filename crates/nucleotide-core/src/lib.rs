// ABOUTME: Core event bridges between Helix and GPUI
// ABOUTME: Provides the fundamental event translation layer

pub mod app_event;
pub mod document_manager;
pub mod event_bridge;
pub mod picker_capability;
pub mod preview_tracker;
pub mod snippets;
pub mod utils;

pub use picker_capability::PickerCapability;

// Re-export V2 domain events
pub use nucleotide_events::v2::{
    document::Event as DocumentEvent, ui::Event as UiEvent, workspace::Event as WorkspaceEvent,
};

pub use app_event::AppEvent;

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
