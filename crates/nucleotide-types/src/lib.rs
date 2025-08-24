// ABOUTME: Pure data types with no cross-crate dependencies
// ABOUTME: Foundation layer for all other nucleotide crates

pub mod completion;
pub mod config;
pub mod editor_types;
pub mod font_config;
pub mod project_config;
pub mod vcs;

// Re-export commonly used types
pub use completion::CompletionTrigger;
pub use config::{FontConfig, FontWeight};
pub use editor_types::{EditorStatus, Severity};
pub use font_config::{EditorFontConfig, Font, FontSettings, FontStyle, UiFontConfig};
pub use project_config::{ProjectMarker, ProjectMarkersConfig, RootStrategy};
pub use vcs::{DiffChangeType, DiffHunkInfo, VcsStatus};
