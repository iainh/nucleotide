// ABOUTME: Completion trigger types and related data structures
// ABOUTME: Pure data types for completion system

/// Completion trigger types
#[derive(Debug, Clone)]
pub enum CompletionTrigger {
    /// Triggered automatically (e.g., after typing '.')
    Automatic,
    /// Triggered manually by user (e.g., Ctrl+Space)
    Manual,
    /// Triggered by a specific character
    Character(char),
}
