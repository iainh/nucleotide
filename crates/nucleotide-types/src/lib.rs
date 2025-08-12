// ABOUTME: Pure data types with no cross-crate dependencies
// ABOUTME: Foundation layer for all other nucleotide crates

pub mod completion;
pub mod config;
pub mod editor_types;
pub mod font_config;

// Re-export commonly used types
pub use completion::CompletionTrigger;
pub use config::{FontConfig, FontWeight};
pub use editor_types::EditorStatus;
pub use font_config::{EditorFontConfig, FontSettings, UiFontConfig};

// Placeholder type for Core during migration
// TODO: Replace with capability traits
pub type CoreEntity = gpui::Entity<()>;
