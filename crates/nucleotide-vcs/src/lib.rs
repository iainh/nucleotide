// ABOUTME: Version control system integration crate for centralized VCS monitoring
// ABOUTME: Provides caching, bulk operations, and event-driven VCS status updates

pub mod vcs_service;

// Re-export main types for easy access
pub use vcs_service::{CacheStats, VcsConfig, VcsEvent, VcsService, VcsServiceHandle};

// Re-export VcsStatus from nucleotide-ui for convenience
pub use nucleotide_ui::VcsStatus;
