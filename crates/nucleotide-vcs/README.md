# nucleotide-vcs

Version control system integration for the Nucleotide editor.

This crate provides centralized VCS status monitoring with advanced caching and performance optimization:

## Features

- **Multi-VCS support**: Git, Mercurial, SVN, and other version control systems
- **Smart caching**: LRU cache with bulk operations and hit ratio tracking
- **Performance monitoring**: Built-in metrics for cache efficiency and operation timing  
- **Event-driven updates**: Broadcasts VCS status changes to interested components
- **Bulk operations**: Efficient querying of multiple file statuses at once
- **Background monitoring**: Non-blocking VCS status updates

## Architecture

- **VcsService**: Main service for monitoring repository status
- **VcsCache**: High-performance caching layer with statistics
- **VcsEvent**: Event system for broadcasting status changes
- **Bulk Operations**: Optimized multi-file status queries

## Usage

```rust
use nucleotide_vcs::{VcsService, VcsEvent};

// Create VCS service
let mut vcs_service = VcsService::new(config, cx);

// Start monitoring a repository
vcs_service.start_monitoring(repo_path, cx);

// Query file status
let status = vcs_service.get_status(&file_path);

// Bulk query multiple files
let statuses = vcs_service.get_bulk_status(&file_paths);
```