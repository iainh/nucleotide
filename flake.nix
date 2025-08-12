{
  description = "Nucleotide - A Native GUI for the Helix editor";

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
        appName = "Nucleotide";
        bundleId = "org.spiralpoint.nucleotide";

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
        buildScript = pkgs.writeScriptBin "build-nucleotide" ''
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
          cp target/release/nucl $out/bin/
        '';

        # macOS app bundle creator
        makeMacOSBundle = pkgs.writeScriptBin "make-macos-bundle" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          if [ ! -f "target/release/nucl" ]; then
            echo "Error: Binary not found. Run 'nix develop --command cargo build --release' first"
            exit 1
          fi
          
          echo "Creating macOS app bundle..."
          
          # Clean up any existing bundle
          rm -rf Nucleotide.app
          
          # Create app structure
          mkdir -p Nucleotide.app/Contents/{MacOS,Resources}
          
          # Copy binary
          cp target/release/nucl Nucleotide.app/Contents/MacOS/Nucleotide
          
          # Copy runtime files (from Nix store to writable location)
          echo "Copying runtime files..."
          mkdir -p Nucleotide.app/Contents/MacOS/runtime
          
          # Use rsync to properly copy from read-only Nix store
          ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
            ${helixRuntime}/ Nucleotide.app/Contents/MacOS/runtime/
          
          # Ensure proper permissions
          chmod -R u+w Nucleotide.app/Contents/MacOS/runtime
          
          # Copy custom Nucleotide themes if available
          if [ -d "assets/themes" ]; then
            echo "Copying custom Nucleotide themes..."
            cp -r assets/themes/*.toml Nucleotide.app/Contents/MacOS/runtime/themes/ 2>/dev/null || true
          fi
          
          # Create Info.plist with full document type support
          cat > Nucleotide.app/Contents/Info.plist <<EOF
          <?xml version="1.0" encoding="UTF-8"?>
          <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
          <plist version="1.0">
          <dict>
            <key>CFBundleExecutable</key>
            <string>${appName}</string>
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
            <string>nucleotide.icns</string>
            <key>LSApplicationCategoryType</key>
            <string>public.app-category.developer-tools</string>
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
          
          # Copy icon if available
          if [ -f assets/nucleotide.icns ]; then
            cp assets/nucleotide.icns Nucleotide.app/Contents/Resources/
          fi
          
          echo "✓ App bundle created at Nucleotide.app"
        '';

        # Linux package creator
        makeLinuxPackage = pkgs.writeScriptBin "make-linux-package" ''
          #!${pkgs.stdenv.shell}
          set -e
          
          if [ ! -f "target/release/nucl" ]; then
            echo "Error: Binary not found. Run 'nix develop --command cargo build --release' first"
            exit 1
          fi
          
          echo "Creating Linux package..."
          
          # Clean up any existing package
          rm -rf nucleotide-linux nucleotide-linux.tar.gz
          
          # Create directory structure
          mkdir -p nucleotide-linux/{bin,share/{applications,nucleotide}}
          
          # Copy binary
          cp target/release/nucl nucleotide-linux/bin/
          
          # Copy runtime files (from Nix store to writable location)
          echo "Copying runtime files..."
          mkdir -p nucleotide-linux/share/nucleotide/runtime
          
          # Use rsync to properly copy from read-only Nix store
          ${pkgs.rsync}/bin/rsync -a --no-perms --no-owner --no-group \
            ${helixRuntime}/ nucleotide-linux/share/nucleotide/runtime/
          
          # Ensure proper permissions
          chmod -R u+w nucleotide-linux/share/nucleotide/runtime
          
          # Copy custom Nucleotide themes if available
          if [ -d "assets/themes" ]; then
            echo "Copying custom Nucleotide themes..."
            cp -r assets/themes/*.toml nucleotide-linux/share/nucleotide/runtime/themes/ 2>/dev/null || true
          fi
          
          # Create desktop file
          cat > nucleotide-linux/share/applications/nucleotide.desktop <<EOF
          [Desktop Entry]
          Name=Nucleotide
          Comment=A post-modern text editor
          Exec=nucl %F
          Terminal=false
          Type=Application
          Icon=nucleotide
          Categories=Development;TextEditor;
          MimeType=text/plain;
          EOF
          
          # Create tarball
          tar czf nucleotide-linux.tar.gz nucleotide-linux
          
          echo "✓ Linux package created at nucleotide-linux.tar.gz"
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
            cargo-deny

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
            echo "║         Welcome to Nucleotide development environment!         ║"
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