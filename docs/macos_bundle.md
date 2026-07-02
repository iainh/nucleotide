# macOS app bundle

Nucleotide includes a helper script for creating a standalone macOS `.app`
bundle with the Helix runtime files needed for syntax highlighting, themes,
queries and grammar loading.

## Build the bundle

Run the script from the repository root:

```bash
./scripts/build-remote-helpers.sh
./scripts/bundle-mac.sh
```

The script creates `Nucleotide.app`.

## What gets bundled

The bundle script:

- Builds `target/release/nucl` when it is missing
- Copies the executable to `Nucleotide.app/Contents/MacOS/Nucleotide`
- Copies Linux `nucleotide-remote` SSH helper artifacts from
  `target/remote-helpers` when present
- Copies `crates/nucleotide/assets/nucleotide.icns` into app resources
- Copies the Helix runtime into `Nucleotide.app/Contents/Resources/runtime`
- Builds missing tree-sitter grammars into the bundled runtime
- Copies Nucleotide theme files into the bundled runtime
- Writes `Nucleotide.app/Contents/Info.plist`

Set `NUCL_BINARY` to bundle an existing executable from another Cargo target
directory:

```bash
NUCL_BINARY=target/aarch64-apple-darwin/release/nucl ./scripts/bundle-mac.sh
```

## Runtime source

The script looks for Helix runtime files in this order:

1. `./runtime`, which CI and release jobs prepare before bundling
2. A Helix checkout under `~/.cargo/git/checkouts`

If no runtime directory is found, the script exits with setup instructions.

## Bundle structure

```text
Nucleotide.app/
└── Contents/
    ├── Info.plist
    ├── MacOS/
    │   └── Nucleotide
    └── Resources/
        ├── nucleotide.icns
        └── runtime/
            ├── grammars/
            ├── queries/
            ├── themes/
            └── tutor
```

## Run the bundle

```bash
open Nucleotide.app
```

Or run the binary directly:

```bash
./Nucleotide.app/Contents/MacOS/Nucleotide [files...]
```

## Customise bundle metadata

Edit these variables in `scripts/bundle-mac.sh`:

```bash
APP_NAME="Nucleotide"
BUNDLE_ID="org.spiralpoint.nucleotide"
EXECUTABLE_NAME="nucl"
```

## Troubleshooting

### "App is damaged" on first run

Unsigned local app bundles can trigger a macOS quarantine warning. Remove the
quarantine attribute for local testing:

```bash
xattr -r -d com.apple.quarantine Nucleotide.app
```

### Runtime files not found

Prepare a local runtime directory before bundling:

```bash
git clone --depth 1 --branch 25.07.1 https://github.com/helix-editor/helix.git helix-temp
cp -r helix-temp/runtime ./runtime
rm -rf helix-temp
```

Then run `./scripts/bundle-mac.sh` again.

### SSH remote helpers not bundled

Build the Linux helper artifacts before bundling:

```bash
./scripts/build-remote-helpers.sh
```

The Nix development shell includes the required Linux musl Rust targets, Zig,
and `cargo-zigbuild`.
