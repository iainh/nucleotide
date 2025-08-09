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


        # Native Rust toolchain
        rustToolchain = fenix.packages.${system}.stable.toolchain;

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
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain
            rustToolchain
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

            # Platform-specific tools
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.DarwinTools
            xcbuild
          ];

          buildInputs = allBuildInputs;

          # Development environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
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
            echo "Bundle creation:"
            echo "  make-macos-bundle            - Create macOS .app bundle"
            echo "  make-linux-package           - Create Linux distribution"
            echo ""
            echo "Nix packages:"
            echo "  nix build .#runtime          - Build runtime files"
            echo ""
            echo "Runtime files available at: $HELIX_RUNTIME"
          '';
        };
      });
}