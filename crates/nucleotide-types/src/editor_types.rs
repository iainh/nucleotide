// ABOUTME: Core editor data types
// ABOUTME: Pure data structures for editor state

use helix_core::diagnostic::Severity;

/// Editor status information
#[derive(Debug, Clone)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}
