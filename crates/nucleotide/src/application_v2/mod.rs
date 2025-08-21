// ABOUTME: Application module decomposition for V2 event system migration
// ABOUTME: Contains domain-specific handlers replacing monolithic Application.rs

pub mod document_handler;

pub use document_handler::DocumentHandler;
