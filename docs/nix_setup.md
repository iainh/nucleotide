# Nix Development Environment for Nucleotide

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

## Opening devenv projects in Nucleotide

Nucleotide also loads [devenv](https://devenv.sh/) project environments for local
projects and remote SSH or WSL workspaces. Install Nix and the `devenv` CLI on the
machine hosting the project. On Windows, use a WSL workspace; native Windows
does not support Nix or devenv.

A project containing `devenv.nix` works without direnv or an `.envrc`. If you
already use direnv, Nucleotide recognizes the standard bootstrap:

```bash
eval "$(devenv direnvrc)"
use devenv
```

Flags on `use devenv`, such as `--impure` and `--profile backend`, are passed to
devenv. `watch_file` directives are also supported. An explicit `use flake`
continues to select the flake environment, even when `devenv.nix` is present.
Other `.envrc` shell commands, including legacy `source_url` bootstraps, use the
existing shell/CLI fallback rather than being partially interpreted.

Nucleotide calls `devenv print-dev-env --json` to capture exported variables for
language servers and project processes. It does not run `enterShell` tasks or
start services. Use `devenv shell` or `devenv up` when you need those actions.
Like native flake loading, this evaluates project Nix code automatically: open
only projects you trust. This is not a replacement for direnv's approval model.

Devenv manages its own persistent evaluation cache. Nucleotide reuses the
captured environment in memory until configuration files, `watch_file` inputs
or files listed in `.devenv/input-paths.txt` change. Loading is cancellable and
has a 30-second timeout. For a cold environment with large downloads, run
`devenv print-dev-env --json` once in a terminal before opening the project.
Failures appear in environment diagnostics and fall back to the existing
CLI/shell environment.

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
# result/Applications/Nucleotide.app
```

## Features

- **Latest Stable Rust**: Uses rust-overlay to provide the latest stable Rust toolchain
- **Platform Support**: Automatically configures dependencies for macOS and Linux
- **Development Tools**: Includes rust-analyzer, cargo-watch, clippy, rustfmt, Zig 0.17.0, and cargo-zigbuild
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
nix build .#checks.x86_64-darwin.nucleotide-clippy
nix build .#checks.x86_64-darwin.nucleotide-fmt
```
