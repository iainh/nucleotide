// ABOUTME: Core editor data types
// ABOUTME: Pure data structures for editor state

use serde::{Deserialize, Serialize};

/// Diagnostic severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

/// Editor status information
#[derive(Debug, Clone)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}

#[cfg(feature = "helix-bridge")]
impl From<helix_core::diagnostic::Severity> for Severity {
    fn from(s: helix_core::diagnostic::Severity) -> Self {
        match s {
            helix_core::diagnostic::Severity::Hint => Severity::Hint,
            helix_core::diagnostic::Severity::Info => Severity::Info,
            helix_core::diagnostic::Severity::Warning => Severity::Warning,
            helix_core::diagnostic::Severity::Error => Severity::Error,
        }
    }
}
