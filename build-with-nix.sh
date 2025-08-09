#!/usr/bin/env bash
# Build script that uses Nix for environment and dependencies

set -e

echo "Building helix-gpui with Nix environment..."

# Build runtime files
echo "Building runtime files..."
nix build '.#runtime' --print-build-logs

# Build the binary
echo "Building binary..."
nix develop --command cargo build --release

# Create bundle if on macOS
if [[ "$OSTYPE" == "darwin"* ]]; then
  echo "Creating macOS app bundle..."
  nix develop --command make-macos-bundle
  echo "✓ App bundle created at Helix.app"
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
  echo "Creating Linux package..."
  nix develop --command make-linux-package
  echo "✓ Linux package created at helix-gpui-linux.tar.gz"
fi

echo ""
echo "Build complete!"
echo "Binary location: target/release/hxg"