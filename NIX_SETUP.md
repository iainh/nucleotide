# Nix Development Environment for Helix GPUI

This project uses Nix flakes to provide a reproducible development environment with all required dependencies.

## Prerequisites

1. **Install Nix** (we recommend Determinate Nix for better defaults):
   ```bash
   # Install Determinate Nix (recommended - includes flakes by default)
   curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install
   
   # Or traditional Nix installer with manual flake setup
   sh <(curl -L https://nixos.org/nix/install) --daemon
   # Then enable flakes manually:
   mkdir -p ~/.config/nix
   echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
   ```

2. **Install direnv** (optional but recommended):
   ```bash
   # macOS with Homebrew
   brew install direnv
   
   # Add to your shell (e.g., ~/.zshrc or ~/.bashrc)
   eval "$(direnv hook zsh)"  # or bash
   ```

## Quick Start

### With direnv (Recommended)

1. Allow direnv in this project:
   ```bash
   direnv allow
   ```

2. The environment will automatically load when you enter the directory.

### Without direnv

1. Enter the development shell:
   ```bash
   nix develop
   ```

2. Or run commands directly:
   ```bash
   nix develop -c cargo build --release
   ```

## Building

### Development Build
```bash
nix develop -c cargo build
```

### Release Build
```bash
nix develop -c cargo build --release
```

### Nix Package Build
```bash
# Build the package
nix build

# Run directly
nix run

# On macOS, the app bundle is created at:
# result/Applications/Helix GPUI.app
```

## Features

- **Latest Stable Rust**: Uses Fenix to provide the latest stable Rust toolchain
- **Platform Support**: Automatically configures dependencies for macOS and Linux
- **Development Tools**: Includes rust-analyzer, cargo-watch, clippy, and rustfmt
- **macOS App Bundle**: Automatically creates a .app bundle on macOS builds
- **Reproducible**: Ensures all developers use the same toolchain and dependencies

## Platform-Specific Notes

### macOS
- Metal framework and related dependencies are automatically included
- App bundle is created with proper Info.plist
- Minimum macOS version: 10.15

### Linux
- X11 and Wayland dependencies are included
- Vulkan support for GPU rendering
- Required: libxkbcommon for keyboard input

## Troubleshooting

### Direnv not loading
```bash
# Reload direnv
direnv reload
```

### Flake inputs out of date
```bash
# Update flake inputs
nix flake update
```

### Clean build
```bash
# Remove build artifacts
rm -rf target/
nix develop -c cargo clean
```

## CI Integration

The flake includes checks that can be run in CI:

```bash
# Run all checks
nix flake check

# Run specific checks
nix build .#checks.x86_64-darwin.helix-gpui-clippy
nix build .#checks.x86_64-darwin.helix-gpui-fmt
```