#!/bin/bash

# ABOUTME: Script to create macOS .app bundle for nucleotide with embedded runtime files
# ABOUTME: This bundles all Helix runtime files (grammars, themes, queries) inside the .app

set -euo pipefail

APP_NAME="Nucleotide"
BUNDLE_NAME="${APP_NAME}.app"
EXECUTABLE_NAME="nucl"
BUNDLE_ID="org.spiralpoint.nucleotide"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Building Nucleotide macOS App Bundle${NC}"

# Clean up existing bundle
if [ -d "${BUNDLE_NAME}" ]; then
    echo -e "${YELLOW}Removing existing ${BUNDLE_NAME}${NC}"
    rm -rf "${BUNDLE_NAME}"
fi

# Check if binary exists, build if not
if [ ! -f "target/release/${EXECUTABLE_NAME}" ]; then
    echo -e "${GREEN}Building release binary...${NC}"
    cargo build --release
    
    # Check again after build
    if [ ! -f "target/release/${EXECUTABLE_NAME}" ]; then
        echo -e "${RED}Error: Binary target/release/${EXECUTABLE_NAME} not found${NC}"
        exit 1
    fi
else
    echo -e "${GREEN}Using existing binary at target/release/${EXECUTABLE_NAME}${NC}"
fi

# Create bundle directory structure
echo -e "${GREEN}Creating bundle structure...${NC}"
mkdir -p "${BUNDLE_NAME}/Contents/MacOS"
mkdir -p "${BUNDLE_NAME}/Contents/Resources"

# Copy the executable
echo -e "${GREEN}Copying executable...${NC}"
cp "target/release/${EXECUTABLE_NAME}" "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"
chmod +x "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"

# Copy the icon file
echo -e "${GREEN}Copying icon...${NC}"
if [ -f "assets/nucleotide.icns" ]; then
    cp "assets/nucleotide.icns" "${BUNDLE_NAME}/Contents/Resources/"
else
    echo -e "${YELLOW}Warning: Icon file not found at assets/nucleotide.icns${NC}"
fi

# Find Helix runtime directory
HELIX_RUNTIME_SOURCE=""

# Check multiple possible locations for runtime files
# 1. Local runtime directory (prepared by CI or manual setup)
if [ -d "runtime" ]; then
    HELIX_RUNTIME_SOURCE="runtime"
    echo -e "${GREEN}Found Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
# 2. Try to find any helix checkout in cargo
elif [ -d "$HOME/.cargo/git/checkouts" ]; then
    HELIX_RUNTIME_SOURCE=$(find "$HOME/.cargo/git/checkouts" -name "runtime" -path "*/helix-*/runtime" -type d | head -1)
    if [ -n "${HELIX_RUNTIME_SOURCE}" ]; then
        echo -e "${GREEN}Found Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
    fi
fi

# If we still haven't found runtime, error out
if [ -z "${HELIX_RUNTIME_SOURCE}" ] || [ ! -d "${HELIX_RUNTIME_SOURCE}" ]; then
    echo -e "${RED}Error: Helix runtime directory not found${NC}"
    echo "Please ensure runtime files are available in one of:"
    echo "  - ./runtime (preferred for CI)"
    echo "  - ~/.cargo/git/checkouts/helix-*/runtime"
    echo ""
    echo "You can clone helix and copy the runtime directory:"
    echo "  git clone --depth 1 --branch 25.07.1 https://github.com/helix-editor/helix.git helix-temp"
    echo "  cp -r helix-temp/runtime ./runtime"
    echo "  rm -rf helix-temp"
    exit 1
fi

# Copy runtime files to Resources (macOS standard location)
echo -e "${GREEN}Copying runtime files...${NC}"
# Use rsync to handle symlinks and missing files gracefully
rsync -a --exclude='grammars/sources' "${HELIX_RUNTIME_SOURCE}/" "${BUNDLE_NAME}/Contents/Resources/runtime/"

# Copy custom Nucleotide themes
if [ -d "assets/themes" ]; then
    echo -e "${GREEN}Copying custom Nucleotide themes...${NC}"
    cp -r assets/themes/*.toml "${BUNDLE_NAME}/Contents/Resources/runtime/themes/" 2>/dev/null || true
    CUSTOM_THEME_COUNT=$(find assets/themes -name "*.toml" 2>/dev/null | wc -l)
    if [ "${CUSTOM_THEME_COUNT}" -gt 0 ]; then
        echo -e "${GREEN}  - ${CUSTOM_THEME_COUNT} custom theme(s) copied${NC}"
    fi
fi

# Verify runtime files were copied
RUNTIME_DEST="${BUNDLE_NAME}/Contents/Resources/runtime"
if [ -d "${RUNTIME_DEST}/grammars" ] && [ -d "${RUNTIME_DEST}/themes" ] && [ -d "${RUNTIME_DEST}/queries" ]; then
    GRAMMAR_COUNT=$(find "${RUNTIME_DEST}/grammars" -name "*.so" | wc -l)
    THEME_COUNT=$(find "${RUNTIME_DEST}/themes" -name "*.toml" | wc -l)
    QUERY_COUNT=$(find "${RUNTIME_DEST}/queries" -mindepth 1 -type d | wc -l)
    echo -e "${GREEN}Runtime files copied successfully:${NC}"
    echo "  - ${GRAMMAR_COUNT} grammar files"
    echo "  - ${THEME_COUNT} theme files"
    echo "  - Query files: ${QUERY_COUNT} languages"
    echo "  - Tutor file: $([ -f "${RUNTIME_DEST}/tutor" ] && echo "✓" || echo "✗")"
else
    echo -e "${RED}Error: Runtime files not copied correctly${NC}"
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
    <string>Nucleotide</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleIconFile</key>
    <string>nucleotide.icns</string>
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
            <string>Text Document</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.text</string>
                <string>public.plain-text</string>
                <string>public.utf8-plain-text</string>
                <string>public.utf16-plain-text</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>nucleotide.icns</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Source Code</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.source-code</string>
                <string>public.c-source</string>
                <string>public.c-plus-plus-source</string>
                <string>public.c-header</string>
                <string>public.shell-script</string>
                <string>public.python-script</string>
                <string>public.ruby-script</string>
                <string>public.perl-script</string>
                <string>com.sun.java-source</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>nucleotide.icns</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Rust Source</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>rs</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>nucleotide.icns</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Markdown Document</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>md</string>
                <string>markdown</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>nucleotide.icns</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Configuration File</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>toml</string>
                <string>yaml</string>
                <string>yml</string>
                <string>json</string>
                <string>xml</string>
                <string>ini</string>
                <string>cfg</string>
                <string>conf</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>nucleotide.icns</string>
        </dict>
    </array>
    <key>NSSupportsAutomaticTermination</key>
    <false/>
    <key>NSSupportsSuddenTermination</key>
    <false/>
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