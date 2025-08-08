# macOS App Bundle for Helix GPUI

This project includes a script to create a standalone macOS app bundle that includes all necessary Helix runtime files.

## Quick Start

To create the app bundle:

```bash
./bundle-mac.sh
```

This creates `Helix.app` with all runtime files embedded.

## What Gets Bundled

The script automatically bundles:
- **Tree-sitter Grammars**: 490+ compiled grammar files (.so) for syntax highlighting
- **Themes**: 172+ color themes (*.toml files)
- **Query Files**: 279+ language-specific tree-sitter queries
- **Tutor**: Interactive tutorial file
- **Main Executable**: The `hxg` binary renamed to `Helix`

## Bundle Structure

```
Helix.app/
├── Contents/
│   ├── Info.plist                 # App metadata
│   ├── MacOS/
│   │   ├── Helix                  # Main executable
│   │   └── runtime/               # Runtime files (where Helix expects them)
│   │       ├── grammars/          # Tree-sitter grammar files
│   │       ├── themes/            # Color themes
│   │       ├── queries/           # Language queries
│   │       └── tutor              # Tutorial file
│   └── Resources/
│       └── runtime/               # Runtime files (macOS standard location)
│           └── [same structure as above]
```

## How It Works

The bundle script:

1. **Builds** the release binary (`cargo build --release`)
2. **Creates** the proper macOS app bundle directory structure
3. **Copies** the executable to `Contents/MacOS/Helix`
4. **Locates** Helix runtime files from the forked dependency
5. **Duplicates** runtime files to both standard locations:
   - `Contents/Resources/runtime/` (macOS convention)
   - `Contents/MacOS/runtime/` (where Helix expects them)
6. **Generates** a proper `Info.plist` with app metadata

## Runtime File Discovery

Helix-loader automatically finds runtime files using this priority order:
1. Development directory (when using `cargo run`)
2. User config directory
3. `HELIX_RUNTIME` environment variable
4. Build-time `HELIX_DEFAULT_RUNTIME` path
5. **Directory next to executable** ← Our bundle uses this

By placing runtime files at `Contents/MacOS/runtime/`, the existing Helix runtime discovery logic works without any code modifications.

## Usage

### Running the Bundle

```bash
# Open with Finder
open Helix.app

# Or run from command line  
./Helix.app/Contents/MacOS/Helix [files...]
```

### Bundle Size

The complete bundle is approximately **460MB**, including:
- Executable: ~50MB
- Tree-sitter grammars: ~200MB
- Themes and queries: ~10MB
- Duplicated for both locations: ~200MB additional

## Development Workflow

1. **Develop** your changes normally with `cargo run`
2. **Test** that everything works as expected
3. **Bundle** with `./bundle-mac.sh`
4. **Test** the bundle with `open Helix.app` or direct execution
5. **Distribute** the self-contained `Helix.app`

## Customization

### Changing App Metadata

Edit these variables in `bundle-mac.sh`:

```bash
APP_NAME="Helix"           # Display name
BUNDLE_ID="com.helix-editor.helix-gpui"  # Bundle identifier
EXECUTABLE_NAME="hxg"      # Source binary name
```

### Adding Custom Runtime Files

To include additional runtime files, modify the rsync commands in the script:

```bash
# Add your custom files to the runtime directory
rsync -a --exclude='grammars/sources' "${CUSTOM_RUNTIME}/" "${BUNDLE_NAME}/Contents/Resources/runtime/"
```

## Troubleshooting

### "App is damaged" on first run

macOS may show security warnings for unsigned apps. To allow execution:

```bash
# Remove quarantine attribute
xattr -r -d com.apple.quarantine Helix.app
```

### Runtime files not found

If Helix can't find themes or grammars:

1. Verify the bundle contains runtime files:
   ```bash
   ls -la Helix.app/Contents/MacOS/runtime/
   ```

2. Check that the runtime directory path in the script matches your Helix checkout

### Large bundle size

The bundle includes all language grammars. To reduce size:
- Remove unused grammars from `runtime/grammars/` before bundling
- Use `--exclude` flags in the rsync commands for specific languages

## Technical Notes

- **No Code Changes Required**: Uses Helix's existing runtime discovery mechanism
- **Self-Contained**: Bundle includes everything needed to run
- **Standard Compliance**: Follows macOS app bundle conventions
- **Backward Compatible**: Works with existing Helix configurations

This approach provides a clean, distributable macOS application while maintaining full compatibility with Helix's runtime system.