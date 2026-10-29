# Nucleotide

**A Native GUI for Helix**

Nucleotide is a high-performance graphical interface for the [Helix](https://helix-editor.com/) modal editor, bringing the power of terminal-based modal editing to a modern native GUI.

## Built on Giants

Nucleotide wouldn't exist without these incredible projects:

- **[Helix](https://helix-editor.com/)** - The powerful modal editor that powers our editing engine
- **[GPUI](https://github.com/zed-industries/zed)** - Zed's blazing-fast GPU-accelerated UI framework  
- **[helix-gpui](https://github.com/polachok/helix-gpui)** - The original project we forked from, created by @polachok

We are deeply grateful to these projects and their maintainers for making Nucleotide possible.

## Features

Currently, Nucleotide provides a native GUI wrapper around Helix with:
- Native macOS/Linux/Windows support
- GPU-accelerated rendering via GPUI
- File tree sidebar
- Integrated terminal (planned)
- Full Helix keybinding support

## Installation

### From Source

Install Rust stable and Zig 0.17.0 first. Zig is required to build the
Ghostty-backed integrated terminal.

The Rust bindings are pinned to an upstream Git revision with Zig 0.17 support
until that update is released on crates.io. The Nix Ghostty source pin matches
the bindings' native library revision; update them together.

```bash
cargo build --release
./target/release/nucl
```

The repo-local Cargo config disables Helix's automatic grammar fetch during
builds. Use `nucl --grammar fetch` / `nucl --grammar build` or the bundle script
when you need to update packaged runtime grammars.

### macOS Bundle

```bash
./scripts/bundle-mac.sh
open Nucleotide.app
```

### Velopack Packages

Nucleotide publishes Velopack installers and update feeds for macOS. On Windows,
the only install artifact is a portable `*-Portable.zip` bundle; extract the
entire bundle to a writable directory and launch `Nucleotide.exe`. The release
also includes the feed and packages used by the in-app updater.

Install Rust stable, Zig 0.17.0, the .NET 8 SDK, and the Velopack CLI before
building a Windows portable package locally.

```powershell
cargo build --release -p nucleotide --bins
.\scripts\clone-helix-runtime.ps1 -Destination helix-temp
try {
  .\scripts\setup-windows-runtime.cmd -RuntimeSource helix-temp\runtime -NuclExe target\release\nucl-grammar.exe
} finally {
  Remove-Item -LiteralPath helix-temp -Recurse -Force
}
dotnet tool update -g vpk --version 1.2.161
.\scripts\package-velopack.ps1 -RequireRemoteHelpers
```

See `docs/windows_install.md` for local Windows Velopack build and install
notes. See `docs/application_updates.md` for the in-app update flow and release
pipeline.

## Development Setup

### Install Git Hooks

To ensure consistent code formatting, install the pre-commit hooks:

```bash
./scripts/install-hooks.sh
```

This will set up automatic `cargo fmt` checks before each commit.

## Configuration

Nucleotide uses Helix's platform config directory. It looks for
`nucleotide.toml` there for GUI settings and loads Helix's `config.toml`
separately for editor settings and keybindings.

- Linux/macOS: commonly `~/.config/helix/nucleotide.toml`
- Windows: `%APPDATA%\helix\nucleotide.toml`

Copy the [complete annotated example](docs/examples/nucleotide.example.toml)
to that location for all supported GUI parameters, descriptions, accepted
values and defaults. Optional overrides remain commented out to preserve
platform defaults. Settings that are parsed but not yet wired to runtime
behaviour are labelled in the example. Save the file, then choose
**Settings > Reload Configuration** (under **Nucleotide** on macOS/Linux or
**File** on Windows), or restart Nucleotide.

## License

MPL-2.0 (same as Helix)
