# Nix Build System for Nucleotide

## Overview

This project uses Nix for reproducible development environments and consistent builds across macOS and Linux. The Nix configuration provides all necessary dependencies and tooling.

## Architecture

The Nix build system consists of:
- **flake.nix**: Main configuration defining packages, development shell, and dependencies
- **Runtime derivation**: Pure Nix build for Helix runtime files (themes, grammars, queries)
- **Development shell**: Complete environment with Rust toolchain and dependencies
- **Build scripts**: Helper scripts for creating platform-specific packages

## Quick Start

### For macOS Users

```bash
# Quick build everything
./build-with-nix.sh
# This creates Nucleotide.app

# Or step by step:
nix develop                   # Enter dev shell
cargo build --release         # Build binary
make-macos-bundle            # Create .app bundle

# Run the app
open Nucleotide.app
# Or directly:
./Nucleotide.app/Contents/MacOS/Nucleotide
```

### For Linux Users

```bash
# Enter development shell and build
nix develop
cargo build --release
make-linux-package

# Extract and run
tar xzf nucleotide-linux.tar.gz
./nucleotide-linux/bin/nucl
```

## Important Notes

### Platform-Specific Builds

**Binaries are platform-specific!** You cannot:
- Run a Linux binary on macOS (causes "Exec format error")
- Run a macOS binary on Linux

Each platform must build its own binary natively:
- **macOS users**: Use `Nucleotide.app` bundle
- **Linux users**: Build on Linux or use CI artifacts

### Cross-Platform CI

GitHub Actions builds for both platforms natively:
- macOS runners build macOS bundles (x86_64 and ARM64)
- Linux runners build Linux packages (x86_64 and ARM64)

## Package Outputs

- **runtime**: Helix runtime files (themes, grammars, languages.toml) - works on all platforms
- **Nucleotide.app**: macOS application bundle (macOS only)
- **nucleotide-linux.tar.gz**: Linux distribution package (Linux only)

## Technical Details

### Why Not Pure Nix Builds?

Pure Nix builds for Rust projects with git dependencies face challenges:
1. **Git dependencies**: Cargo needs to fetch from GitHub during build
2. **Vendoring complexity**: Git dependencies can't be easily vendored
3. **helix-loader**: Requires languages.toml at build time

The current approach uses Nix for:
- Consistent development environment
- Dependency management
- Runtime file packaging
- Reproducible toolchain

While using traditional cargo for the actual build process.

### Dependencies

**Platform-agnostic:**
- Rust stable toolchain (via fenix)
- OpenSSL, pkg-config, git, curl, sqlite

**macOS-specific:**
- Apple frameworks (Foundation, AppKit, Metal, etc.)
- libiconv

**Linux-specific:**
- X11/Wayland libraries
- OpenGL, Vulkan
- Font rendering (freetype, fontconfig)

## GitHub Actions Integration

The CI workflows use Nix for consistent builds:

```yaml
- name: Install Nix
  uses: cachix/install-nix-action@v24
  
- name: Build with Nix environment
  run: |
    nix build .#runtime
    nix develop --command cargo build --release
    nix develop --command make-macos-bundle  # or make-linux-package
```

## Cachix Integration

For faster CI builds, configure Cachix:
1. Create account at cachix.org
2. Create cache named "nucleotide"
3. Add CACHIX_AUTH_TOKEN to GitHub secrets
4. Builds will cache and reuse Nix derivations

## Troubleshooting

### "Exec format error"
You're trying to run a binary built for a different platform. Ensure you're using the correct package for your OS.

### Build Hangs
If the build hangs, it's likely fetching dependencies. This is normal for the first build.

### Missing Runtime Files
Runtime files are built separately. Run `nix build .#runtime` first or use the complete build script.

### Framework Errors on macOS
Ensure Xcode Command Line Tools are installed:
```bash
xcode-select --install
```

## Development Tips

1. **Use direnv** for automatic shell activation:
   ```bash
   echo "use flake" > .envrc
   direnv allow
   ```

2. **Fast rebuilds**: The Nix shell caches dependencies, making rebuilds fast

3. **Platform testing**: Use CI to test builds on other platforms

4. **Tool versions**: All developers use identical tool versions via Nix

## Quick Commands

```bash
# Enter development environment
nix develop

# Build runtime files only
nix build .#runtime

# Build release version
cargo build --release

# Run tests
cargo test

# Format code
cargo fmt

# Run linter
cargo clippy

# Create macOS bundle (macOS only)
make-macos-bundle

# Create Linux package (Linux only)
make-linux-package

# Clean build
cargo clean
```