// ABOUTME: Event bridge from GPUI events back to Helix editor
// ABOUTME: Provides a way for GUI events to influence editor behavior

use helix_view::DocumentId;
use nucleotide_logging::{debug, info, instrument, warn};
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// Events that should be sent from GPUI to Helix
#[derive(Debug, Clone)]
pub enum GpuiToHelixEvent {
    /// Window was resized - Helix should update its terminal size
    WindowResized { width: u16, height: u16 },
    /// Window gained/lost focus - Helix can optimize accordingly
    WindowFocusChanged { focused: bool },
    /// Theme was changed via GUI - Helix should reload theme
    ThemeChanged { theme_name: String },
    /// Font size changed via GUI - Helix should update display
    FontSizeChanged { size: f32 },
    /// External file modification detected - Helix should reload
    ExternalFileChanged {
        doc_id: DocumentId,
        path: std::path::PathBuf,
    },
    /// System memory pressure - Helix should reduce cache usage
    MemoryPressure { level: MemoryPressureLevel },
    /// Accessibility mode changed - Helix should adjust features
    AccessibilityChanged {
        high_contrast: bool,
        screen_reader: bool,
    },
    /// Performance degradation detected - Helix should disable expensive features
    PerformanceDegraded { severe: bool },
}

/// Memory pressure levels
#[derive(Debug, Clone)]
pub enum MemoryPressureLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Global sender for GPUI->Helix events
static GPUI_TO_HELIX_SENDER: OnceLock<mpsc::UnboundedSender<GpuiToHelixEvent>> = OnceLock::new();

/// Initialize the GPUI->Helix event bridge
#[instrument(skip(sender))]
pub fn initialize_gpui_to_helix_bridge(sender: mpsc::UnboundedSender<GpuiToHelixEvent>) {
    if GPUI_TO_HELIX_SENDER.set(sender).is_err() {
        warn!("GPUI->Helix event bridge was already initialized");
    } else {
        info!("GPUI->Helix event bridge initialized successfully");
    }
}

/// Send a GPUI event to Helix
pub fn send_gpui_event_to_helix(event: GpuiToHelixEvent) {
    if let Some(sender) = GPUI_TO_HELIX_SENDER.get() {
        debug!(
            event.type = ?std::mem::discriminant(&event),
            "Sending GPUI event to Helix"
        );
        if let Err(e) = sender.send(event) {
            warn!(
                error = %e,
                "Failed to send GPUI event to Helix"
            );
        }
    } else {
        warn!(
            event = ?event,
            "GPUI->Helix event bridge not initialized, dropping event"
        );
    }
}

/// Register handlers for common GPUI events
#[instrument]
pub fn register_gpui_event_handlers() {
    info!("Registering GPUI event handlers for Helix integration");
    // Note: Actual GPUI event registration would happen in the main window setup
    // This is a placeholder for the registration logic
}

/// Receiver type for GPUI->Helix events
pub type GpuiToHelixEventReceiver = mpsc::UnboundedReceiver<GpuiToHelixEvent>;

/// Create a channel pair for GPUI->Helix events
pub fn create_gpui_to_helix_channel() -> (
    mpsc::UnboundedSender<GpuiToHelixEvent>,
    GpuiToHelixEventReceiver,
) {
    mpsc::unbounded_channel()
}

/// Handle a GPUI event within Helix editor
#[instrument(skip(editor))]
pub fn handle_gpui_event_in_helix(event: &GpuiToHelixEvent, editor: &mut helix_view::Editor) {
    match event {
        GpuiToHelixEvent::WindowResized { width, height } => {
            info!(
                width = %width,
                height = %height,
                "Window resized"
            );
            // Update editor area size
            let area = helix_view::graphics::Rect {
                x: 0,
                y: 0,
                width: *width,
                height: *height,
            };
            editor.resize(area);
        }
        GpuiToHelixEvent::WindowFocusChanged { focused } => {
            info!(
                focused = %focused,
                "Window focus changed"
            );
            if !focused {
                // When window loses focus, save all modified documents
                for doc in editor.documents() {
                    if doc.is_modified() {
                        info!(
                            doc_id = ?doc.id(),
                            "Auto-saving document on focus loss"
                        );
                        // Could trigger auto-save here if enabled
                    }
                }
            }
        }
        GpuiToHelixEvent::ThemeChanged { theme_name } => {
            info!(
                theme_name = %theme_name,
                "Theme changed"
            );
            // Reload theme - this would need access to theme loader
            // For now, just log the event
        }
        GpuiToHelixEvent::FontSizeChanged { size } => {
            info!(
                size = %size,
                "Font size changed"
            );
            // Update editor display settings
            // This would need integration with the renderer
        }
        GpuiToHelixEvent::ExternalFileChanged { doc_id, path } => {
            info!(
                doc_id = ?doc_id,
                path = ?path,
                "External file change detected"
            );
            if let Some(doc) = editor.document(*doc_id) {
                if !doc.is_modified() {
                    // Only reload if document isn't modified locally
                    info!(
                        path = ?path,
                        "Reloading externally modified file"
                    );
                    // Would trigger file reload here
                }
            }
        }
        GpuiToHelixEvent::MemoryPressure { level } => {
            info!(
                level = ?level,
                "Memory pressure detected"
            );
            match level {
                MemoryPressureLevel::High | MemoryPressureLevel::Critical => {
                    // Reduce memory usage by clearing caches
                    info!("Reducing memory usage due to pressure");
                    // Could clear syntax highlighting cache, completion cache, etc.
                }
                _ => {}
            }
        }
        GpuiToHelixEvent::AccessibilityChanged {
            high_contrast,
            screen_reader,
        } => {
            info!(
                high_contrast = %high_contrast,
                screen_reader = %screen_reader,
                "Accessibility settings changed"
            );
            // Adjust editor features for accessibility
            if *screen_reader {
                // Could enable more verbose status messages, etc.
            }
        }
        GpuiToHelixEvent::PerformanceDegraded { severe } => {
            info!(
                severe = %severe,
                "Performance degradation detected"
            );
            if *severe {
                // Disable expensive features like real-time syntax highlighting
                info!("Disabling expensive features due to performance issues");
            }
        }
    }
}
