# nucleotide-logging

Centralized logging infrastructure for the Nucleotide editor using `tokio-tracing`.

## Overview

This crate provides a comprehensive logging solution that replaces the basic `log` crate usage throughout the Nucleotide workspace with structured tracing using `tokio-tracing`.

## Features

- **Structured logging** with contextual spans and fields
- **Multiple output targets**: console, file (`~/.config/nucleotide/nucleotide.log`), JSON
- **Configurable filtering** per component and log level  
- **Performance optimized** with minimal overhead
- **Environment variable support** (`NUCLEOTIDE_LOG`, `RUST_LOG`)

## Usage

```rust
use nucleotide_logging::init_logging;

fn main() -> anyhow::Result<()> {
    // Initialize logging with default configuration
    init_logging()?;
    
    // Use tracing macros throughout your code
    tracing::info!("Application starting");
    tracing::debug!(component = "ui", "Rendering main window");
    
    Ok(())
}
```

## Architecture

- `config.rs` - Configuration structures and environment parsing
- `subscriber.rs` - Tracing subscriber setup and layer composition  
- `layers.rs` - Custom layer implementations for different output formats
- `lib.rs` - Public API and re-exports