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

### macOS Bundle

```bash
./bundle-mac.sh
open Nucleotide.app
```

## Configuration

Nucleotide looks for configuration in `~/.config/nucleotide/nucleotide.toml` and falls back to Helix configuration at `~/.config/helix/config.toml`.

## License

MPL-2.0 (same as Helix)