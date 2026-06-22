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

### Windows Package

Nucleotide publishes a Windows zip package from CI. Extract it to a stable
directory such as `%LOCALAPPDATA%\Programs\Nucleotide`, then run `nucl.exe`.
The package includes an optional per-user shell integration script:

```powershell
.\install-windows-context-menu.cmd
```

See `docs/windows_install.md` for Explorer, Open With, Start Menu, App Paths,
`nucleotide://`, Windows Installed apps, and terminal `PATH` options.

## Development Setup

### Install Git Hooks

To ensure consistent code formatting, install the pre-commit hooks:

```bash
./scripts/install-hooks.sh
```

This will set up automatic `cargo fmt` checks before each commit.

## Configuration

Nucleotide uses Helix's platform config directory. It looks for
`nucleotide.toml` there and falls back to Helix's `config.toml` for editor
settings.

- Linux/macOS: commonly `~/.config/helix/nucleotide.toml`
- Windows: `%APPDATA%\helix\nucleotide.toml`

See `docs/examples/nucleotide.example.toml` for a sample GUI configuration.

## License

MPL-2.0 (same as Helix)
