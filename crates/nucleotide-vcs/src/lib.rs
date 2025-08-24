// ABOUTME: Version control system integration crate for centralized VCS monitoring
// ABOUTME: Provides caching, bulk operations, and event-driven VCS status updates

pub mod vcs_service;

// Re-export main types for easy access
pub use vcs_service::{CacheStats, VcsConfig, VcsEvent, VcsService, VcsServiceHandle};

// Re-export VCS types from nucleotide-types
pub use nucleotide_types::{DiffChangeType, DiffHunkInfo, VcsStatus};
