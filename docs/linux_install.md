Linux Installation Guide

Overview
- Builds Nucleotide on Linux using Rust and system libraries for windowing, GPU, and Git.
- Covers common distributions with package prerequisites, build steps, environment setup, and troubleshooting.

Prerequisites
- Rust toolchain (stable) via rustup
- Linker and C toolchain: clang + lld recommended
- System libraries (X11/Wayland, OpenGL/Vulkan, font, SSL/zlib for libgit2)

Debian/Ubuntu (22.04+)
- Install prerequisites:
  sudo apt update
  sudo apt install -y \
    build-essential pkg-config clang lld \
    libssl-dev zlib1g-dev \
    libxkbcommon-dev libwayland-dev \
    libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libegl1-mesa-dev mesa-common-dev \
    libfontconfig1-dev libfreetype6-dev

Fedora (38+)
- Install prerequisites:
  sudo dnf groupinstall -y "Development Tools" 
  sudo dnf install -y clang lld pkg-config \
    openssl-devel zlib-devel \
    libxkbcommon-devel wayland-devel \
    libX11-devel libxcb-devel libXrender-devel libXfixes-devel \
    mesa-libEGL-devel \
    fontconfig-devel freetype-devel

Arch Linux
- Install prerequisites:
  sudo pacman -S --needed base-devel clang lld pkgconf \
    openssl zlib \
    libxkbcommon wayland libx11 libxcb libxrender libxfixes \
    mesa egl-wayland \
    fontconfig freetype

Rust toolchain
- Install rustup if needed:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
- Ensure toolchain is up to date:
  rustup update

Build
- Clone and build:
  git clone https://github.com/<org>/nucleotide.git
  cd nucleotide
  cargo build --workspace --release
- Run the app:
  cargo run -p nucleotide
  # or run the built binary:
  ./target/release/nucl

Runtime assets and configuration
- Helix runtime (themes, grammars) is used by parts of the app. If Helix is installed from your distro, set:
  export HELIX_RUNTIME=/usr/share/helix/runtime
- If Helix is built from source, point to its runtime directory instead:
  export HELIX_RUNTIME=~/src/helix/runtime
- Optional logging:
  export RUST_LOG=info

Wayland/X11 notes
- The app supports Wayland and X11; if you hit issues on a given session:
  - Force X11: export WINIT_UNIX_BACKEND=x11
  - Force GL backend for wgpu (older GPUs): export WGPU_BACKEND=gl

Known issues on Linux
- Inline SVG path logging in window controls: you may see repeated lines like
  gpui: could not find asset at path "M5 12h14"
  These are cosmetic logs from the Linux titlebar icon renderer using inline SVG path data. They do not affect functionality. They will be silenced in a follow‑up patch. To hide them temporarily set:
  export RUST_LOG=warn

Common build errors
- Linker not found (lld): install lld and clang as above, or remove lld via a local override if necessary.
- openssl/libgit2 errors: ensure openssl and zlib dev packages are installed (see prerequisites).
- Wayland/X11 missing headers: install libxkbcommon-dev and wayland-devel (see distro sections).

Uninstall / Cleanup
- No system install is performed by default. Remove the repo directory to uninstall the built artifacts.

