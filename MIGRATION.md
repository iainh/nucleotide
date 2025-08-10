# Migration Guide: helix-gpui to Nucleotide

## For Users

### Command Changes
- Old command: `hxg`
- New command: `nucl`

### Configuration
- Old config: `~/.config/helix/ghx.toml`
- New config: `~/.config/helix/nucleotide.toml`

Nucleotide will automatically fall back to your Helix configuration if no Nucleotide-specific config exists.

### macOS Application
- Old app name: Helix.app
- New app name: Nucleotide.app

## For Developers

### Package Name
- Old: `helix-gpui`
- New: `nucleotide`

### Repository
The project has been renamed but maintains full compatibility with Helix internals.

## Compatibility

Nucleotide remains 100% compatible with:
- Helix configuration files
- Helix themes
- Helix language configurations
- Helix runtime files