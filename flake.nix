{
  description = "Helix GPUI - A GUI implementation of the Helix text editor built with GPUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    # Helix repository for runtime files
    helix = {
      url = "github:helix-editor/helix/25.07.1";
      flake = false;
    };
    
    # Zed repository for GPUI
    zed = {
      url = "github:zed-industries/zed/faa45c53d7754cfdd91d2f7edd3c786abc703ec7";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, fenix, flake-utils, helix, zed }:
    flake-utils.lib.eachSystem [ "x86_64-darwin" "aarch64-darwin" "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
          };
        };

        # Cross-compilation packages for Linux targets
        pkgsCrossLinux64 = if pkgs.stdenv.isDarwin then
          import nixpkgs {
            inherit system;
            crossSystem = {
              config = "x86_64-unknown-linux-gnu";
            };
          }
        else pkgs;

        pkgsCrossLinuxArm64 = if pkgs.stdenv.isDarwin then
          import nixpkgs {
            inherit system;
            crossSystem = {
              config = "aarch64-unknown-linux-gnu";
            };
          }
        else pkgs;

        # Native Rust toolchain
        rustToolchain = fenix.packages.${system}.stable.toolchain;
        
        # Cross-compilation Rust toolchain with Linux targets
        rustCrossToolchain = fenix.packages.${system}.combine [
          fenix.packages.${system}.stable.cargo
          fenix.packages.${system}.stable.rustc
          fenix.packages.${system}.stable.rust-src
          fenix.packages.${system}.stable.clippy
          fenix.packages.${system}.stable.rustfmt
          fenix.packages.${system}.targets.x86_64-unknown-linux-gnu.stable.rust-std
          fenix.packages.${system}.targets.aarch64-unknown-linux-gnu.stable.rust-std
        ];

        # Platform-specific dependencies
        darwinDeps = with pkgs; lib.optionals stdenv.isDarwin [
          libiconv
          darwin.apple_sdk.frameworks.Foundation
          darwin.apple_sdk.frameworks.AppKit
          darwin.apple_sdk.frameworks.CoreGraphics
          darwin.apple_sdk.frameworks.CoreServices
          darwin.apple_sdk.frameworks.CoreText
          darwin.apple_sdk.frameworks.IOKit
          darwin.apple_sdk.frameworks.Metal
          darwin.apple_sdk.frameworks.Security
          darwin.apple_sdk.frameworks.SystemConfiguration
          darwin.apple_sdk.frameworks.AVFoundation
          darwin.apple_sdk.frameworks.VideoToolbox
        ];

        linuxDeps = with pkgs; lib.optionals stdenv.isLinux [
          libxkbcommon
          xorg.libxcb
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
          vulkan-loader
          wayland
          libGL
          freetype
          fontconfig
        ];

        # Common dependencies
        commonDeps = with pkgs; [
          openssl
          pkg-config
          git
          curl
          sqlite
        ];

        # Build inputs for development and building
        allBuildInputs = commonDeps ++ darwinDeps ++ linuxDeps;

        # Version info
        version = "0.1.0";
        appName = "Helix";
        bundleId = "dev.plhk.helix-gpui";

        # Helix runtime files
        helixRuntime = pkgs.stdenv.mkDerivation {
          name = "helix-runtime";
          src = helix;

          buildPhase = ''
            # Ensure languages.toml exists for helix-loader
            if [ -f runtime/languages.toml ]; then
              cp runtime/languages.toml ./
            else
              echo "# Minimal languages.toml" > ./languages.toml
              echo "[[language]]" >> ./languages.toml
              echo 'name = "rust"' >> ./languages.toml
              echo 'scope = "source.rust"' >> ./languages.toml
            fi
          '';

          installPhase = ''
            mkdir -p $out
            if [ -d runtime ]; then
              cp -r runtime/* $out/
            fi
            
            # Ensure languages.toml exists
            if [ ! -f $out/languages.toml ]; then
              cp ./languages.toml $out/
            fi
            
            # Clean up source directories
            rm -rf $out/grammars/sources 2>/dev/null || true
          '';
        };

        # Cross-compile script for Linux x86_64
        crossBuildLinux64 = pkgs.writeScriptBin "cross-build-linux-x64" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          echo "Cross-compiling for Linux x86_64 using Nix toolchain..."
          
          # Set up cross-compilation environment
          export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${pkgsCrossLinux64.stdenv.cc}/bin/${pkgsCrossLinux64.stdenv.cc.targetPrefix}cc"
          export CC_x86_64_unknown_linux_gnu="${pkgsCrossLinux64.stdenv.cc}/bin/${pkgsCrossLinux64.stdenv.cc.targetPrefix}cc"
          export CXX_x86_64_unknown_linux_gnu="${pkgsCrossLinux64.stdenv.cc}/bin/${pkgsCrossLinux64.stdenv.cc.targetPrefix}c++"
          export AR_x86_64_unknown_linux_gnu="${pkgsCrossLinux64.stdenv.cc}/bin/${pkgsCrossLinux64.stdenv.cc.targetPrefix}ar"
          
          # PKG_CONFIG setup for cross compilation
          export PKG_CONFIG_ALLOW_CROSS=1
          export PKG_CONFIG_PATH="${pkgsCrossLinux64.openssl.dev}/lib/pkgconfig"
          export OPENSSL_DIR="${pkgsCrossLinux64.openssl.dev}"
          export OPENSSL_LIB_DIR="${pkgsCrossLinux64.openssl.out}/lib"
          export OPENSSL_INCLUDE_DIR="${pkgsCrossLinux64.openssl.dev}/include"
          
          # Ensure runtime directory exists with proper permissions
          rm -rf runtime 2>/dev/null || true
          mkdir -p runtime
          cp ${helixRuntime}/languages.toml runtime/languages.toml
          chmod -R u+w runtime
          
          echo "Building with Rust cross toolchain..."
          ${rustCrossToolchain}/bin/cargo build --release --target x86_64-unknown-linux-gnu
          
          if [ -f target/x86_64-unknown-linux-gnu/release/hxg ]; then
            echo "✓ Build successful!"
            echo "Binary: target/x86_64-unknown-linux-gnu/release/hxg"
            file target/x86_64-unknown-linux-gnu/release/hxg
          else
            echo "✗ Build failed"
            exit 1
          fi
        '';

        # Cross-compile script for Linux ARM64
        crossBuildLinuxArm64 = pkgs.writeScriptBin "cross-build-linux-arm64" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          echo "Cross-compiling for Linux aarch64 using Nix toolchain..."
          
          # Set up cross-compilation environment
          export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${pkgsCrossLinuxArm64.stdenv.cc}/bin/${pkgsCrossLinuxArm64.stdenv.cc.targetPrefix}cc"
          export CC_aarch64_unknown_linux_gnu="${pkgsCrossLinuxArm64.stdenv.cc}/bin/${pkgsCrossLinuxArm64.stdenv.cc.targetPrefix}cc"
          export CXX_aarch64_unknown_linux_gnu="${pkgsCrossLinuxArm64.stdenv.cc}/bin/${pkgsCrossLinuxArm64.stdenv.cc.targetPrefix}c++"
          export AR_aarch64_unknown_linux_gnu="${pkgsCrossLinuxArm64.stdenv.cc}/bin/${pkgsCrossLinuxArm64.stdenv.cc.targetPrefix}ar"
          
          # PKG_CONFIG setup for cross compilation
          export PKG_CONFIG_ALLOW_CROSS=1
          export PKG_CONFIG_PATH="${pkgsCrossLinuxArm64.openssl.dev}/lib/pkgconfig"
          export OPENSSL_DIR="${pkgsCrossLinuxArm64.openssl.dev}"
          export OPENSSL_LIB_DIR="${pkgsCrossLinuxArm64.openssl.out}/lib"
          export OPENSSL_INCLUDE_DIR="${pkgsCrossLinuxArm64.openssl.dev}/include"
          
          # Ensure runtime directory exists with proper permissions
          rm -rf runtime 2>/dev/null || true
          mkdir -p runtime
          cp ${helixRuntime}/languages.toml runtime/languages.toml
          chmod -R u+w runtime
          
          echo "Building with Rust cross toolchain..."
          ${rustCrossToolchain}/bin/cargo build --release --target aarch64-unknown-linux-gnu
          
          if [ -f target/aarch64-unknown-linux-gnu/release/hxg ]; then
            echo "✓ Build successful!"
            echo "Binary: target/aarch64-unknown-linux-gnu/release/hxg"
            file target/aarch64-unknown-linux-gnu/release/hxg
          else
            echo "✗ Build failed"
            exit 1
          fi
        '';

        # Package creator for cross-compiled Linux binaries
        packageCrossLinux = pkgs.writeScriptBin "package-cross-linux" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          echo "Creating Linux packages from cross-compiled binaries..."
          
          # Package x86_64 if it exists
          if [ -f target/x86_64-unknown-linux-gnu/release/hxg ]; then
            echo "Packaging Linux x86_64..."
            rm -rf helix-gpui-linux-x86_64
            mkdir -p helix-gpui-linux-x86_64/{bin,share/{applications,helix-gpui}}
            
            cp target/x86_64-unknown-linux-gnu/release/hxg helix-gpui-linux-x86_64/bin/
            
            # Copy runtime files
            ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
              ${helixRuntime}/ helix-gpui-linux-x86_64/share/helix-gpui/runtime/
            
            # Create desktop file
            cat > helix-gpui-linux-x86_64/share/applications/helix-gpui.desktop <<EOF
            [Desktop Entry]
            Name=Helix GPUI
            Comment=A post-modern text editor
            Exec=hxg %F
            Terminal=false
            Type=Application
            Icon=helix-gpui
            Categories=Development;TextEditor;
            MimeType=text/plain;
            EOF
            
            tar czf helix-gpui-linux-x86_64.tar.gz helix-gpui-linux-x86_64
            echo "✓ Created helix-gpui-linux-x86_64.tar.gz"
          fi
          
          # Package aarch64 if it exists
          if [ -f target/aarch64-unknown-linux-gnu/release/hxg ]; then
            echo "Packaging Linux aarch64..."
            rm -rf helix-gpui-linux-aarch64
            mkdir -p helix-gpui-linux-aarch64/{bin,share/{applications,helix-gpui}}
            
            cp target/aarch64-unknown-linux-gnu/release/hxg helix-gpui-linux-aarch64/bin/
            
            # Copy runtime files
            ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
              ${helixRuntime}/ helix-gpui-linux-aarch64/share/helix-gpui/runtime/
            
            # Create desktop file
            cat > helix-gpui-linux-aarch64/share/applications/helix-gpui.desktop <<EOF
            [Desktop Entry]
            Name=Helix GPUI
            Comment=A post-modern text editor
            Exec=hxg %F
            Terminal=false
            Type=Application
            Icon=helix-gpui
            Categories=Development;TextEditor;
            MimeType=text/plain;
            EOF
            
            tar czf helix-gpui-linux-aarch64.tar.gz helix-gpui-linux-aarch64
            echo "✓ Created helix-gpui-linux-aarch64.tar.gz"
          fi
          
          echo ""
          echo "Packages created:"
          ls -lh helix-gpui-linux-*.tar.gz 2>/dev/null || echo "No packages found"
        '';

        # Build script that produces the binary
        buildScript = pkgs.writeScriptBin "build-helix-gpui" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          export PATH="${rustToolchain}/bin:${pkgs.pkg-config}/bin:${pkgs.git}/bin:$PATH"
          export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig"
          export OPENSSL_NO_VENDOR=1
          export HELIX_RUNTIME="${helixRuntime}"
          
          # Platform-specific setup
          ${pkgs.lib.optionalString pkgs.stdenv.isDarwin ''
            export DYLD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath darwinDeps}:$DYLD_LIBRARY_PATH"
          ''}
          ${pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath linuxDeps}:$LD_LIBRARY_PATH"
          ''}
          
          # Set up build environment
          export HOME=$TMPDIR
          export CARGO_HOME=$TMPDIR/.cargo
          
          # Configure git
          git config --global url."https://github.com/".insteadOf "git@github.com:"
          git config --global init.defaultBranch main
          
          # Ensure runtime directory exists with proper permissions
          rm -rf runtime 2>/dev/null || true
          mkdir -p runtime
          cp ${helixRuntime}/languages.toml runtime/languages.toml
          chmod -R u+w runtime
          
          # Build the project
          cargo build --release
          
          # Copy binary to output
          mkdir -p $out/bin
          cp target/release/hxg $out/bin/
        '';

        # macOS app bundle creator
        makeMacOSBundle = pkgs.writeScriptBin "make-macos-bundle" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          if [ ! -f "target/release/hxg" ]; then
            echo "Error: Binary not found. Run 'nix develop --command cargo build --release' first"
            exit 1
          fi
          
          echo "Creating macOS app bundle..."
          
          # Clean up any existing bundle
          rm -rf Helix.app
          
          # Create app structure
          mkdir -p Helix.app/Contents/{MacOS,Resources}
          
          # Copy binary
          cp target/release/hxg Helix.app/Contents/MacOS/Helix
          
          # Copy runtime files (from Nix store to writable location)
          echo "Copying runtime files..."
          mkdir -p Helix.app/Contents/MacOS/runtime
          
          # Use rsync to properly copy from read-only Nix store
          ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
            ${helixRuntime}/ Helix.app/Contents/MacOS/runtime/
          
          # Ensure proper permissions
          chmod -R u+w Helix.app/Contents/MacOS/runtime
          
          # Create Info.plist
          cat > Helix.app/Contents/Info.plist <<EOF
          <?xml version="1.0" encoding="UTF-8"?>
          <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
          <plist version="1.0">
          <dict>
            <key>CFBundleExecutable</key>
            <string>Helix</string>
            <key>CFBundleIdentifier</key>
            <string>${bundleId}</string>
            <key>CFBundleName</key>
            <string>${appName}</string>
            <key>CFBundleDisplayName</key>
            <string>${appName}</string>
            <key>CFBundleVersion</key>
            <string>${version}</string>
            <key>CFBundleShortVersionString</key>
            <string>${version}</string>
            <key>CFBundlePackageType</key>
            <string>APPL</string>
            <key>CFBundleSignature</key>
            <string>????</string>
            <key>LSMinimumSystemVersion</key>
            <string>10.15</string>
            <key>NSHighResolutionCapable</key>
            <true/>
            <key>CFBundleDevelopmentRegion</key>
            <string>en</string>
            <key>CFBundleIconFile</key>
            <string>helix-gpui.icns</string>
            <key>LSApplicationCategoryType</key>
            <string>public.app-category.developer-tools</string>
          </dict>
          </plist>
          EOF
          
          # Copy icon if available
          if [ -f assets/helix-gpui.icns ]; then
            cp assets/helix-gpui.icns Helix.app/Contents/Resources/
          fi
          
          echo "✓ App bundle created at Helix.app"
        '';

        # Linux package creator
        makeLinuxPackage = pkgs.writeScriptBin "make-linux-package" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          if [ ! -f "target/release/hxg" ]; then
            echo "Error: Binary not found. Run 'nix develop --command cargo build --release' first"
            exit 1
          fi
          
          echo "Creating Linux package..."
          
          # Clean up any existing package
          rm -rf helix-gpui-linux helix-gpui-linux.tar.gz
          
          # Create directory structure
          mkdir -p helix-gpui-linux/{bin,share/{applications,helix-gpui}}
          
          # Copy binary
          cp target/release/hxg helix-gpui-linux/bin/
          
          # Copy runtime files (from Nix store to writable location)
          echo "Copying runtime files..."
          mkdir -p helix-gpui-linux/share/helix-gpui/runtime
          
          # Use rsync to properly copy from read-only Nix store
          ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
            ${helixRuntime}/ helix-gpui-linux/share/helix-gpui/runtime/
          
          # Ensure proper permissions
          chmod -R u+w helix-gpui-linux/share/helix-gpui/runtime
          
          # Create desktop file
          cat > helix-gpui-linux/share/applications/helix-gpui.desktop <<EOF
          [Desktop Entry]
          Name=Helix GPUI
          Comment=A post-modern text editor
          Exec=hxg %F
          Terminal=false
          Type=Application
          Icon=helix-gpui
          Categories=Development;TextEditor;
          MimeType=text/plain;
          EOF
          
          # Create tarball
          tar czf helix-gpui-linux.tar.gz helix-gpui-linux
          
          echo "✓ Linux package created at helix-gpui-linux.tar.gz"
        '';

      in
      {
        packages = {
          runtime = helixRuntime;
          buildScript = buildScript;
          makeMacOSBundle = makeMacOSBundle;
          makeLinuxPackage = makeLinuxPackage;
          crossBuildLinux64 = crossBuildLinux64;
          crossBuildLinuxArm64 = crossBuildLinuxArm64;
          packageCrossLinux = packageCrossLinux;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain with cross-compilation targets
            rustCrossToolchain
            rust-analyzer

            # Development tools
            cargo-watch
            cargo-edit
            cargo-outdated

            # For running the application
            ripgrep
            tree-sitter

            # Build helpers
            buildScript
            makeMacOSBundle
            makeLinuxPackage
            crossBuildLinux64
            crossBuildLinuxArm64
            packageCrossLinux

            # Platform-specific tools
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.DarwinTools
            xcbuild
          ];

          buildInputs = allBuildInputs ++ (if pkgs.stdenv.isDarwin then [
            # Cross-compilation dependencies for Linux targets
            pkgsCrossLinux64.stdenv.cc
            pkgsCrossLinuxArm64.stdenv.cc
          ] else []);

          # Development environment variables
          RUST_SRC_PATH = "${rustCrossToolchain}/lib/rustlib/src/rust/library";
          HELIX_RUNTIME = "${helixRuntime}";
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          OPENSSL_NO_VENDOR = 1;

          shellHook = ''
            echo "╔════════════════════════════════════════════════════════════════╗"
            echo "║         Welcome to helix-gpui development environment!         ║"
            echo "╚════════════════════════════════════════════════════════════════╝"
            echo ""
            echo "Available commands:"
            echo "  cargo build --release        - Build for native platform"
            echo "  cargo run                    - Run debug version"
            echo "  cargo test                   - Run tests"
            echo "  cargo clippy                 - Run linter"
            echo "  cargo fmt                    - Format code"
            echo ""
            echo "Cross-compilation (from macOS to Linux):"
            echo "  cross-build-linux-x64        - Build for Linux x86_64"
            echo "  cross-build-linux-arm64      - Build for Linux ARM64"
            echo "  package-cross-linux          - Package cross-compiled binaries"
            echo ""
            echo "Bundle creation:"
            echo "  make-macos-bundle            - Create macOS .app bundle"
            echo "  make-linux-package           - Create Linux distribution"
            echo ""
            echo "Nix packages:"
            echo "  nix build .#runtime          - Build runtime files"
            echo ""
            echo "Runtime files available at: $HELIX_RUNTIME"
            echo ""
            echo "Note: Cross-compilation targets are included in the Rust toolchain."
            echo "      No need for rustup!"
          '';
        };
      });
}