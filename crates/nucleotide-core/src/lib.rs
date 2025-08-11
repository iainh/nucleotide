// ABOUTME: Core event bridges between Helix and GPUI
// ABOUTME: Provides the fundamental event translation layer

pub mod event_bridge;
pub mod gpui_to_helix_bridge;

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
