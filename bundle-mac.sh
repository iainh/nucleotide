#!/bin/bash

# ABOUTME: Script to create macOS .app bundle for helix-gpui with embedded runtime files
# ABOUTME: This bundles all Helix runtime files (grammars, themes, queries) inside the .app

set -euo pipefail

APP_NAME="Helix"
BUNDLE_NAME="${APP_NAME}.app"
EXECUTABLE_NAME="hxg"
BUNDLE_ID="com.helix-editor.helix-gpui"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Building Helix GPUI macOS App Bundle${NC}"

# Clean up existing bundle
if [ -d "${BUNDLE_NAME}" ]; then
    echo -e "${YELLOW}Removing existing ${BUNDLE_NAME}${NC}"
    rm -rf "${BUNDLE_NAME}"
fi

# Build the release binary
echo -e "${GREEN}Building release binary...${NC}"
cargo build --release

# Check if binary exists
if [ ! -f "target/release/${EXECUTABLE_NAME}" ]; then
    echo -e "${RED}Error: Binary target/release/${EXECUTABLE_NAME} not found${NC}"
    exit 1
fi

# Create bundle directory structure
echo -e "${GREEN}Creating bundle structure...${NC}"
mkdir -p "${BUNDLE_NAME}/Contents/MacOS"
mkdir -p "${BUNDLE_NAME}/Contents/Resources"

# Copy the executable
echo -e "${GREEN}Copying executable...${NC}"
cp "target/release/${EXECUTABLE_NAME}" "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"
chmod +x "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"

# Find Helix runtime directory
HELIX_RUNTIME_SOURCE=""
HELIX_CHECKOUT_RUNTIME="/Users/iheggie/.cargo/git/checkouts/helix-b99af130ded19729/a05c151/runtime"

if [ -d "${HELIX_CHECKOUT_RUNTIME}" ]; then
    HELIX_RUNTIME_SOURCE="${HELIX_CHECKOUT_RUNTIME}"
    echo -e "${GREEN}Found Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
else
    echo -e "${RED}Error: Helix runtime directory not found${NC}"
    echo "Expected location: ${HELIX_CHECKOUT_RUNTIME}"
    exit 1
fi

# Copy runtime files to both Resources (macOS standard) and MacOS (where Helix expects)
echo -e "${GREEN}Copying runtime files...${NC}"
# Use rsync to handle symlinks and missing files gracefully
rsync -a --exclude='grammars/sources' "${HELIX_RUNTIME_SOURCE}/" "${BUNDLE_NAME}/Contents/Resources/runtime/"
rsync -a --exclude='grammars/sources' "${HELIX_RUNTIME_SOURCE}/" "${BUNDLE_NAME}/Contents/MacOS/runtime/"

# Verify runtime files were copied
RUNTIME_DEST="${BUNDLE_NAME}/Contents/Resources/runtime"
RUNTIME_MACOS_DEST="${BUNDLE_NAME}/Contents/MacOS/runtime"
if [ -d "${RUNTIME_DEST}/grammars" ] && [ -d "${RUNTIME_DEST}/themes" ] && [ -d "${RUNTIME_DEST}/queries" ] && \
   [ -d "${RUNTIME_MACOS_DEST}/grammars" ] && [ -d "${RUNTIME_MACOS_DEST}/themes" ] && [ -d "${RUNTIME_MACOS_DEST}/queries" ]; then
    GRAMMAR_COUNT=$(find "${RUNTIME_DEST}/grammars" -name "*.so" | wc -l)
    THEME_COUNT=$(find "${RUNTIME_DEST}/themes" -name "*.toml" | wc -l)
    QUERY_COUNT=$(find "${RUNTIME_DEST}/queries" -mindepth 1 -type d | wc -l)
    echo -e "${GREEN}Runtime files copied successfully to both locations:${NC}"
    echo "  - ${GRAMMAR_COUNT} grammar files"
    echo "  - ${THEME_COUNT} theme files"
    echo "  - Query files: ${QUERY_COUNT} languages"
    echo "  - Tutor file: $([ -f "${RUNTIME_DEST}/tutor" ] && echo "✓" || echo "✗")"
    echo "  - MacOS location: $([ -d "${RUNTIME_MACOS_DEST}" ] && echo "✓" || echo "✗")"
else
    echo -e "${RED}Error: Runtime files not copied correctly to both locations${NC}"
    exit 1
fi

# Create Info.plist
echo -e "${GREEN}Creating Info.plist...${NC}"
cat > "${BUNDLE_NAME}/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>Helix</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSRequiresAquaSystemAppearance</key>
    <false/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Text File</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.text</string>
                <string>public.plain-text</string>
                <string>public.source-code</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
EOF

# Get bundle size
BUNDLE_SIZE=$(du -sh "${BUNDLE_NAME}" | cut -f1)

echo -e "${GREEN}✓ Successfully created ${BUNDLE_NAME}${NC}"
echo -e "${GREEN}  Bundle size: ${BUNDLE_SIZE}${NC}"
echo -e "${GREEN}  Location: $(pwd)/${BUNDLE_NAME}${NC}"
echo ""
echo -e "${YELLOW}To test the bundle:${NC}"
echo "  open ${BUNDLE_NAME}"
echo ""
echo -e "${YELLOW}To run from command line:${NC}"
echo "  ${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"