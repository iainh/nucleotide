# nucleotide-project

Project detection and manifest provider system for Nucleotide, based on Zed's proven patterns.

## Features

- **ManifestProvider trait**: Language-agnostic interface for project detection
- **Built-in providers**: Support for major programming languages:
  - Rust (Cargo.toml, Cargo.lock)
  - Python (pyproject.toml, setup.py, requirements.txt)
  - TypeScript/Node.js (package.json, tsconfig.json)
  - Go (go.mod, go.sum)
  - Java (pom.xml, build.gradle, build.gradle.kts)
  - C# (*.csproj, *.sln)
- **Efficient traversal**: Optimized ancestor directory scanning
- **Provider registry**: Dynamic registration and lookup system
- **Extensible**: Easy to add new language providers

## Usage

```rust
use nucleotide_project::{ManifestProviders, providers::*};

// Register providers
let providers = ManifestProviders::new();
providers.register(Box::new(RustManifestProvider::new()));
providers.register(Box::new(PythonManifestProvider::new()));

// Detect project root
if let Some(project_root) = providers.detect_project_root(&file_path) {
    println!("Found project at: {}", project_root.display());
}
```