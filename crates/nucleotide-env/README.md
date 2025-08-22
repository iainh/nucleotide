# nucleotide-env

Shell environment management for the Nucleotide editor.

This crate provides comprehensive environment handling following Zed's architecture:
- CLI environment inheritance
- Directory-specific shell environment capture
- LSP environment injection
- Shell-specific command generation (bash, zsh, fish, etc.)
- Environment caching and performance optimization

## Features

- **Multi-shell support**: bash, zsh, fish, tcsh, csh
- **Environment inheritance**: Preserves CLI-launched environments
- **Directory-specific environments**: Captures environments for specific directories
- **LSP integration**: Provides environments for language servers
- **Caching system**: Optimizes repeated environment queries
- **Error recovery**: Graceful fallbacks when shell execution fails

## Usage

```rust
use nucleotide_env::{ShellEnvironmentProvider, EnvironmentProvider};

// Create environment provider
let provider = ShellEnvironmentProvider::new();

// Get environment for a specific directory
let env = provider.get_directory_environment(&path).await?;

// Use with LSP
let lsp_env = provider.get_lsp_environment(&workspace_root).await?;
```