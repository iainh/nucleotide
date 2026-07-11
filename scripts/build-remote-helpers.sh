#!/usr/bin/env bash

# ABOUTME: Builds Linux nucleotide-remote helper binaries for SSH and WSL auto-install
# ABOUTME: Produces the artifact names consumed by app and installer packaging

set -euo pipefail

REMOTE_HELPER_DIR="${NUCL_REMOTE_HELPER_DIR:-target/remote-helpers}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
PROFILE="${NUCL_REMOTE_HELPER_PROFILE:-release}"

if ! command -v cargo-zigbuild >/dev/null 2>&1; then
    echo "Error: cargo-zigbuild is required. Install it with 'cargo install --locked cargo-zigbuild' or enter the Nix dev shell." >&2
    exit 1
fi

case "${PROFILE}" in
    release)
        cargo_profile_args=(--release)
        target_profile_dir="release"
        ;;
    debug)
        cargo_profile_args=()
        target_profile_dir="debug"
        ;;
    *)
        cargo_profile_args=(--profile "${PROFILE}")
        target_profile_dir="${PROFILE}"
        ;;
esac

mkdir -p "${REMOTE_HELPER_DIR}"

build_helper() {
    local target="$1"
    local artifact="$2"
    local source
    local destination

    echo "Building ${artifact} for ${target}..."
    cargo zigbuild "${cargo_profile_args[@]}" -p nucleotide-remote --target "${target}"

    source="${CARGO_TARGET_DIR}/${target}/${target_profile_dir}/nucleotide-remote"
    destination="${REMOTE_HELPER_DIR}/${artifact}"

    if [ ! -f "${source}" ]; then
        echo "Error: expected helper binary not found: ${source}" >&2
        exit 1
    fi

    cp "${source}" "${destination}"
    chmod +x "${destination}"
    echo "  -> ${destination}"
}

build_helper "x86_64-unknown-linux-musl" "nucleotide-remote-linux-x86_64"
build_helper "aarch64-unknown-linux-musl" "nucleotide-remote-linux-aarch64"

case "$(uname -s):$(uname -m)" in
    Linux:x86_64 | Linux:amd64)
        "${REMOTE_HELPER_DIR}/nucleotide-remote-linux-x86_64" version --json
        ;;
    Linux:aarch64 | Linux:arm64)
        "${REMOTE_HELPER_DIR}/nucleotide-remote-linux-aarch64" version --json
        ;;
    *)
        echo "Skipping helper execution check on non-Linux host."
        ;;
esac
