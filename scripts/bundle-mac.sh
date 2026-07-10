#!/bin/bash

# ABOUTME: Script to create macOS .app bundle for nucleotide with embedded runtime files
# ABOUTME: This bundles all Helix runtime files (grammars, themes, queries) inside the .app

set -euo pipefail

APP_NAME="Nucleotide"
BUNDLE_NAME="${APP_NAME}.app"
EXECUTABLE_NAME="nucl"
BUNDLE_ID="org.spiralpoint.nucleotide"
BINARY_PATH="${NUCL_BINARY:-target/release/${EXECUTABLE_NAME}}"
BINARY_PATH_EXPLICIT="${NUCL_BINARY:+1}"
if [ -n "${NUCL_GRAMMAR_TOOL:-}" ]; then
    GRAMMAR_TOOL_PATH="${NUCL_GRAMMAR_TOOL}"
    GRAMMAR_TOOL_PATH_EXPLICIT=1
elif [ -n "${BINARY_PATH_EXPLICIT}" ]; then
    # Target-specific builds place all binaries together. Keep the grammar
    # helper in the same toolchain environment instead of rebuilding it with
    # a host-level Cargo invocation that may not have Zig available.
    GRAMMAR_TOOL_PATH="$(dirname "${BINARY_PATH}")/nucl-grammar"
    GRAMMAR_TOOL_PATH_EXPLICIT=1
else
    GRAMMAR_TOOL_PATH="target/release/nucl-grammar"
    GRAMMAR_TOOL_PATH_EXPLICIT=""
fi
REMOTE_HELPER_DIR="${NUCL_REMOTE_HELPER_DIR:-target/remote-helpers}"
REMOTE_HELPERS_REQUIRED="${NUCL_REQUIRE_REMOTE_HELPERS:-0}"
GRAMMAR_LIB_EXTENSION="dylib"
HELIX_REV="$(sed -n 's/.*helix-loader = .*rev = "\([0-9a-f]*\)".*/\1/p' Cargo.toml | head -n 1)"
HELIX_REV_SHORT="${HELIX_REV:0:7}"

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
if [ ! -f "${BINARY_PATH}" ]; then
    if [ -n "${BINARY_PATH_EXPLICIT}" ]; then
        echo -e "${RED}Error: NUCL_BINARY does not exist: ${BINARY_PATH}${NC}"
        exit 1
    fi

    echo -e "${GREEN}Building release binary...${NC}"
    cargo build --release
    
    # Check again after build
    if [ ! -f "${BINARY_PATH}" ]; then
        echo -e "${RED}Error: Binary ${BINARY_PATH} not found${NC}"
        exit 1
    fi
else
    echo -e "${GREEN}Using existing binary at ${BINARY_PATH}${NC}"
fi

if [ ! -f "${GRAMMAR_TOOL_PATH}" ]; then
    if [ -n "${GRAMMAR_TOOL_PATH_EXPLICIT}" ]; then
        echo -e "${RED}Error: NUCL_GRAMMAR_TOOL does not exist: ${GRAMMAR_TOOL_PATH}${NC}"
        exit 1
    fi

    echo -e "${GREEN}Building grammar helper...${NC}"
    cargo build --release --bin nucl-grammar

    if [ ! -f "${GRAMMAR_TOOL_PATH}" ]; then
        echo -e "${RED}Error: grammar helper ${GRAMMAR_TOOL_PATH} not found${NC}"
        exit 1
    fi
else
    echo -e "${GREEN}Using existing grammar helper at ${GRAMMAR_TOOL_PATH}${NC}"
fi

# Create bundle directory structure
echo -e "${GREEN}Creating bundle structure...${NC}"
mkdir -p "${BUNDLE_NAME}/Contents/MacOS"
mkdir -p "${BUNDLE_NAME}/Contents/Resources"

# Copy the executable
echo -e "${GREEN}Copying executable...${NC}"
cp "${BINARY_PATH}" "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"
chmod +x "${BUNDLE_NAME}/Contents/MacOS/${APP_NAME}"

# Copy Linux remote helper binaries for SSH auto-upload. They live beside the
# app executable because helper discovery checks the current executable's
# directory before falling back to PATH or explicit config.
REMOTE_HELPERS=(
    "nucleotide-remote-linux-x86_64"
    "nucleotide-remote-linux-aarch64"
)
REMOTE_HELPERS_COPIED=0
REMOTE_HELPERS_CAN_COPY=1
REMOTE_HELPERS_AUTO_BUILD="${NUCL_BUILD_REMOTE_HELPERS:-auto}"
REMOTE_HELPER_SOURCE_PATHS=(
    "Cargo.toml"
    "Cargo.lock"
    "crates/nucleotide-remote"
    "crates/nucleotide-workspace"
    "crates/nucleotide-env"
    "crates/nucleotide-logging"
    "crates/nucleotide-process"
)

remote_helpers_need_rebuild() {
    if [ ! -d "${REMOTE_HELPER_DIR}" ]; then
        return 0
    fi

    for helper in "${REMOTE_HELPERS[@]}"; do
        local helper_path="${REMOTE_HELPER_DIR}/${helper}"
        if [ ! -f "${helper_path}" ]; then
            return 0
        fi

        for source_path in "${REMOTE_HELPER_SOURCE_PATHS[@]}"; do
            if [ ! -e "${source_path}" ]; then
                continue
            fi

            if [ -n "$(find "${source_path}" -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) -newer "${helper_path}" -print -quit)" ]; then
                return 0
            fi
        done
    done

    return 1
}

refresh_remote_helpers_if_needed() {
    if ! remote_helpers_need_rebuild; then
        return
    fi

    case "${REMOTE_HELPERS_AUTO_BUILD}" in
        1|true|yes|auto)
            if command -v cargo-zigbuild >/dev/null 2>&1; then
                echo -e "${GREEN}Building SSH remote helpers...${NC}"
                NUCL_REMOTE_HELPER_DIR="${REMOTE_HELPER_DIR}" ./scripts/build-remote-helpers.sh
                return
            fi

            if command -v nix >/dev/null 2>&1; then
                echo -e "${GREEN}Building SSH remote helpers with Nix...${NC}"
                NUCL_REMOTE_HELPER_DIR="${REMOTE_HELPER_DIR}" nix develop -c build-remote-helpers
                return
            fi

            if [ "${REMOTE_HELPERS_REQUIRED}" = "1" ]; then
                echo -e "${RED}Error: SSH remote helpers are stale or missing, and cargo-zigbuild/Nix are not available${NC}"
                echo "Install cargo-zigbuild or Nix, then rerun the bundle script."
                exit 1
            fi

            echo -e "${YELLOW}Warning: SSH remote helpers are stale or missing; skipping them because cargo-zigbuild/Nix are not available${NC}"
            REMOTE_HELPERS_CAN_COPY=0
            ;;
        0|false|no)
            if [ "${REMOTE_HELPERS_REQUIRED}" = "1" ]; then
                echo -e "${RED}Error: SSH remote helpers are stale or missing and NUCL_BUILD_REMOTE_HELPERS=0${NC}"
                exit 1
            fi

            echo -e "${YELLOW}Warning: SSH remote helpers are stale or missing; skipping them because NUCL_BUILD_REMOTE_HELPERS=0${NC}"
            REMOTE_HELPERS_CAN_COPY=0
            ;;
        *)
            echo -e "${RED}Error: invalid NUCL_BUILD_REMOTE_HELPERS value: ${REMOTE_HELPERS_AUTO_BUILD}${NC}"
            echo "Use auto, 1, true, yes, 0, false, or no."
            exit 1
            ;;
    esac
}

refresh_remote_helpers_if_needed

if [ "${REMOTE_HELPERS_CAN_COPY}" = "1" ] && [ -d "${REMOTE_HELPER_DIR}" ]; then
    echo -e "${GREEN}Copying SSH remote helpers from ${REMOTE_HELPER_DIR}...${NC}"
    for helper in "${REMOTE_HELPERS[@]}"; do
        helper_path="${REMOTE_HELPER_DIR}/${helper}"
        if [ -f "${helper_path}" ]; then
            cp "${helper_path}" "${BUNDLE_NAME}/Contents/MacOS/${helper}"
            chmod +x "${BUNDLE_NAME}/Contents/MacOS/${helper}"
            REMOTE_HELPERS_COPIED=$((REMOTE_HELPERS_COPIED + 1))
            echo "  - ${helper}"
        elif [ "${REMOTE_HELPERS_REQUIRED}" = "1" ]; then
            echo -e "${RED}Error: required SSH remote helper not found: ${helper_path}${NC}"
            exit 1
        fi
    done
elif [ "${REMOTE_HELPERS_REQUIRED}" = "1" ]; then
    echo -e "${RED}Error: required SSH remote helper directory not found: ${REMOTE_HELPER_DIR}${NC}"
    exit 1
else
    echo -e "${YELLOW}Warning: SSH remote helper directory not found at ${REMOTE_HELPER_DIR}${NC}"
fi

if [ "${REMOTE_HELPERS_REQUIRED}" = "1" ] && [ "${REMOTE_HELPERS_COPIED}" -ne "${#REMOTE_HELPERS[@]}" ]; then
    echo -e "${RED}Error: expected ${#REMOTE_HELPERS[@]} SSH remote helpers, copied ${REMOTE_HELPERS_COPIED}${NC}"
    exit 1
fi

# Copy the icon file
echo -e "${GREEN}Copying icon...${NC}"
if [ -f "crates/nucleotide/assets/nucleotide.icns" ]; then
    cp "crates/nucleotide/assets/nucleotide.icns" "${BUNDLE_NAME}/Contents/Resources/"
else
    echo -e "${YELLOW}Warning: Icon file not found at crates/nucleotide/assets/nucleotide.icns${NC}"
fi

# Find Helix runtime directory
HELIX_RUNTIME_SOURCE=""

is_complete_runtime() {
    local candidate="$1"
    [ -d "${candidate}/queries" ] && [ -d "${candidate}/themes" ]
}

grammar_source_count() {
    local grammar_dir="$1"
    if [ ! -d "${grammar_dir}/sources" ]; then
        echo 0
        return
    fi

    find "${grammar_dir}/sources" -mindepth 2 -path "*/src/parser.c" | wc -l
}

# Check multiple possible locations for runtime files
# 1. Local runtime directory (prepared by CI or manual setup)
if [ -d "runtime" ]; then
    if is_complete_runtime "runtime"; then
        HELIX_RUNTIME_SOURCE="runtime"
        echo -e "${GREEN}Found Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
    else
        echo -e "${YELLOW}Warning: local runtime directory is incomplete; looking for another Helix runtime${NC}"
    fi
# 2. Try to find the Helix checkout pinned by this workspace
fi

if [ -z "${HELIX_RUNTIME_SOURCE}" ] && [ -n "${HELIX_REV_SHORT}" ] && [ -d "$HOME/.cargo/git/checkouts" ]; then
    while IFS= read -r candidate; do
        if is_complete_runtime "${candidate}"; then
            HELIX_RUNTIME_SOURCE="${candidate}"
            break
        fi
    done < <(find "$HOME/.cargo/git/checkouts" -path "*/${HELIX_REV_SHORT}/runtime" -type d)
    if [ -n "${HELIX_RUNTIME_SOURCE}" ]; then
        echo -e "${GREEN}Found pinned Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
    fi
fi

# 3. Fall back to any available Helix checkout
if [ -z "${HELIX_RUNTIME_SOURCE}" ] && [ -d "$HOME/.cargo/git/checkouts" ]; then
    while IFS= read -r candidate; do
        if is_complete_runtime "${candidate}"; then
            HELIX_RUNTIME_SOURCE="${candidate}"
            break
        fi
    done < <(find "$HOME/.cargo/git/checkouts" -name "runtime" -path "*/helix-*/runtime" -type d)
    if [ -n "${HELIX_RUNTIME_SOURCE}" ]; then
        echo -e "${GREEN}Found Helix runtime at: ${HELIX_RUNTIME_SOURCE}${NC}"
    fi
fi

# If we still haven't found runtime, error out
if [ -z "${HELIX_RUNTIME_SOURCE}" ] || [ ! -d "${HELIX_RUNTIME_SOURCE}" ]; then
    echo -e "${RED}Error: Helix runtime directory not found${NC}"
    echo "Please ensure runtime files are available in one of:"
    echo "  - ./runtime with queries/ and themes/ subdirectories (preferred for CI)"
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
# Use rsync to handle symlinks and missing files gracefully. Grammar sources
# are copied temporarily so the bundled runtime can build parser libraries.
rsync -a "${HELIX_RUNTIME_SOURCE}/" "${BUNDLE_NAME}/Contents/Resources/runtime/"

RUNTIME_DEST="${BUNDLE_NAME}/Contents/Resources/runtime"

if [ -f "${HELIX_RUNTIME_SOURCE}/languages.toml" ]; then
    cp "${HELIX_RUNTIME_SOURCE}/languages.toml" "${RUNTIME_DEST}/languages.toml"
elif [ -f "$(dirname "${HELIX_RUNTIME_SOURCE}")/languages.toml" ]; then
    cp "$(dirname "${HELIX_RUNTIME_SOURCE}")/languages.toml" "${RUNTIME_DEST}/languages.toml"
else
    echo -e "${YELLOW}Warning: Helix languages.toml not found next to runtime${NC}"
fi

# Build compiled tree-sitter grammars into the bundled runtime when the source
# runtime does not already provide them.
mkdir -p "${RUNTIME_DEST}/grammars"
GRAMMAR_COUNT=$(find "${RUNTIME_DEST}/grammars" -maxdepth 1 -name "*.${GRAMMAR_LIB_EXTENSION}" | wc -l)
if [ "${GRAMMAR_COUNT}" -eq 0 ]; then
    mkdir -p "${RUNTIME_DEST}/grammars/sources"

    SOURCE_COUNT=$(grammar_source_count "${RUNTIME_DEST}/grammars")
    if [ "${SOURCE_COUNT}" -eq 0 ]; then
        rm -rf "${RUNTIME_DEST}/grammars/sources"
        mkdir -p "${RUNTIME_DEST}/grammars/sources"
    fi

    if [ "${SOURCE_COUNT}" -eq 0 ]; then
        echo -e "${GREEN}Fetching grammar sources for bundled runtime...${NC}"
        # helix_loader writes grammars to the first runtime directory. Setting
        # CARGO_MANIFEST_DIR makes that first directory resolve to Resources/runtime.
        set +e
        CARGO_MANIFEST_DIR="$(pwd)/${BUNDLE_NAME}/Contents/Resources/nucleotide" \
            HELIX_RUNTIME="$(pwd)/${RUNTIME_DEST}" \
            "${GRAMMAR_TOOL_PATH}" fetch
        GRAMMAR_FETCH_STATUS=$?
        set -e

        SOURCE_COUNT=$(grammar_source_count "${RUNTIME_DEST}/grammars")
        if [ "${GRAMMAR_FETCH_STATUS}" -ne 0 ]; then
            if [ "${SOURCE_COUNT}" -gt 0 ]; then
                echo -e "${YELLOW}Warning: some grammars failed to fetch; attempting to build fetched sources${NC}"
            else
                echo -e "${RED}Error: grammar fetch failed and produced no usable grammar sources${NC}"
                echo "No fetched grammar checkout contains src/parser.c. Check network access"
                echo "to the grammar source hosts, then rerun the bundle script."
                exit "${GRAMMAR_FETCH_STATUS}"
            fi
        elif [ "${SOURCE_COUNT}" -eq 0 ]; then
            echo -e "${RED}Error: grammar fetch completed but produced no usable grammar sources${NC}"
            echo "No fetched grammar checkout contains src/parser.c."
            exit 1
        fi
    fi

    echo -e "${GREEN}Building compiled grammars for bundled runtime...${NC}"
    set +e
    CARGO_MANIFEST_DIR="$(pwd)/${BUNDLE_NAME}/Contents/Resources/nucleotide" \
        HELIX_RUNTIME="$(pwd)/${RUNTIME_DEST}" \
        "${GRAMMAR_TOOL_PATH}" build
    GRAMMAR_BUILD_STATUS=$?
    set -e
    GRAMMAR_COUNT=$(find "${RUNTIME_DEST}/grammars" -maxdepth 1 -name "*.${GRAMMAR_LIB_EXTENSION}" | wc -l)
    if [ "${GRAMMAR_BUILD_STATUS}" -ne 0 ]; then
        if [ "${GRAMMAR_COUNT}" -gt 0 ]; then
            echo -e "${YELLOW}Warning: some grammars failed to build; bundling ${GRAMMAR_COUNT} compiled grammar(s)${NC}"
        else
            echo -e "${RED}Error: grammar build failed and produced no compiled grammars${NC}"
            exit "${GRAMMAR_BUILD_STATUS}"
        fi
    elif [ "${GRAMMAR_COUNT}" -eq 0 ]; then
        echo -e "${RED}Error: grammar build completed but produced no compiled grammars${NC}"
        echo "Check that ${RUNTIME_DEST}/languages.toml contains grammar entries and that"
        echo "${RUNTIME_DEST}/grammars/sources contains fetched parser sources."
        exit 1
    fi
fi

# Do not ship grammar source checkouts in the final app bundle.
rm -rf "${RUNTIME_DEST}/grammars/sources"

# Copy custom Nucleotide themes
if [ -d "crates/nucleotide/assets/themes" ]; then
    echo -e "${GREEN}Copying custom Nucleotide themes...${NC}"
    cp -r crates/nucleotide/assets/themes/*.toml "${BUNDLE_NAME}/Contents/Resources/runtime/themes/" 2>/dev/null || true
    CUSTOM_THEME_COUNT=$(find crates/nucleotide/assets/themes -name "*.toml" 2>/dev/null | wc -l)
    if [ "${CUSTOM_THEME_COUNT}" -gt 0 ]; then
        echo -e "${GREEN}  - ${CUSTOM_THEME_COUNT} custom theme(s) copied${NC}"
    fi
fi

if [ -d "${RUNTIME_DEST}/grammars" ] && [ -d "${RUNTIME_DEST}/themes" ] && [ -d "${RUNTIME_DEST}/queries" ]; then
    GRAMMAR_COUNT=$(find "${RUNTIME_DEST}/grammars" -maxdepth 1 -name "*.${GRAMMAR_LIB_EXTENSION}" | wc -l)
    THEME_COUNT=$(find "${RUNTIME_DEST}/themes" -name "*.toml" | wc -l)
    QUERY_COUNT=$(find "${RUNTIME_DEST}/queries" -mindepth 1 -type d | wc -l)
    if [ "${GRAMMAR_COUNT}" -eq 0 ]; then
        echo -e "${RED}Error: no compiled grammar files were bundled${NC}"
        echo "Run the bundle again after fetching grammar sources, or inspect the"
        echo "grammar build output above for the failing parser."
        exit 1
    fi
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
