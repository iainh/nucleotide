{
  description = "Helix GPUI - A GUI implementation of the Helix text editor built with GPUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, fenix, crane, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        # Use latest stable Rust from Fenix
        rustToolchain = fenix.packages.${system}.stable.toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

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
          cmake
          git
          curl
          sqlite
        ];

        # Source filtering to improve rebuild performance
        src = pkgs.lib.cleanSourceWith {
          src = craneLib.path ./.;
          filter = path: type:
            (pkgs.lib.hasSuffix "\.md" path) ||
            (pkgs.lib.hasSuffix "\.rs" path) ||
            (pkgs.lib.hasSuffix "\.toml" path) ||
            (pkgs.lib.hasSuffix "\.lock" path) ||
            (pkgs.lib.hasInfix "/assets/" path) ||
            (pkgs.lib.hasInfix "/src/" path) ||
            (craneLib.filterCargoSources path type);
        };

        commonArgs = {
          inherit src;
          strictDeps = true;

          buildInputs = commonDeps ++ darwinDeps ++ linuxDeps;

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            rustToolchain
          ] ++ lib.optionals stdenv.isDarwin [
            xcbuild
          ];

          # Environment variables
          OPENSSL_NO_VENDOR = 1;
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
        };

        # Build dependencies only (for better caching)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The main package
        helix-gpui = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          # macOS-specific: Create app bundle
          postInstall = pkgs.lib.optionalString pkgs.stdenv.isDarwin ''
            mkdir -p $out/Applications
            
            # Create app bundle structure
            APP_NAME="Helix GPUI.app"
            APP_DIR="$out/Applications/$APP_NAME"
            mkdir -p "$APP_DIR/Contents/MacOS"
            mkdir -p "$APP_DIR/Contents/Resources"
            
            # Copy binary
            cp target/release/hxg "$APP_DIR/Contents/MacOS/hxg"
            
            # Copy icon if it exists
            if [ -f assets/helix-gpui.icns ]; then
              cp assets/helix-gpui.icns "$APP_DIR/Contents/Resources/helix-gpui.icns"
            fi
            
            # Create Info.plist
            cat > "$APP_DIR/Contents/Info.plist" << EOF
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
              <key>CFBundleExecutable</key>
              <string>hxg</string>
              <key>CFBundleIdentifier</key>
              <string>dev.plhk.helix-gpui</string>
              <key>CFBundleName</key>
              <string>Helix GPUI</string>
              <key>CFBundleDisplayName</key>
              <string>Helix GPUI</string>
              <key>CFBundleVersion</key>
              <string>0.0.1</string>
              <key>CFBundleShortVersionString</key>
              <string>0.0.1</string>
              <key>CFBundleIconFile</key>
              <string>helix-gpui</string>
              <key>LSMinimumSystemVersion</key>
              <string>10.15</string>
              <key>NSHighResolutionCapable</key>
              <true/>
            </dict>
            </plist>
            EOF
            
            # Create symlink in bin
            ln -s "$APP_DIR/Contents/MacOS/hxg" $out/bin/hxg
          '';

          meta = with pkgs.lib; {
            description = "A GUI implementation of the Helix text editor built with GPUI";
            homepage = "https://github.com/polachok/helix-gpui";
            license = licenses.mpl20;
            maintainers = [ ];
            mainProgram = "hxg";
          };
        });

      in
      {
        packages = {
          default = helix-gpui;
          helix-gpui = helix-gpui;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = helix-gpui;
          name = "hxg";
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = with pkgs; [
            # Rust toolchain with components
            (fenix.packages.${system}.stable.withComponents [
              "cargo"
              "clippy"
              "rust-src"
              "rustc"
              "rustfmt"
            ])

            # Development tools
            rust-analyzer
            cargo-watch
            cargo-edit
            cargo-outdated

            # For running the application
            ripgrep

            # Platform-specific tools
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.DarwinTools
            xcbuild
          ];

          inputsFrom = [ helix-gpui ];

          # Development environment variables
          RUST_SRC_PATH = "${fenix.packages.${system}.stable.rust-src}/lib/rustlib/src/rust/library";

          shellHook = ''
            echo "Welcome to helix-gpui development environment!"
            echo ""
            echo "Available commands:"
            echo "  cargo build          - Build the project"
            echo "  cargo run            - Run helix-gpui"
            echo "  cargo test           - Run tests"
            echo "  cargo clippy         - Run linter"
            echo "  cargo fmt            - Format code"
            echo ""
            ${pkgs.lib.optionalString pkgs.stdenv.isDarwin ''
              echo "macOS specific:"
              echo "  open target/release/bundle/osx/Helix\\ GPUI.app - Open the app bundle"
              echo ""
            ''}
          '';
        };

        # Optional: checks for CI
        checks = {
          inherit helix-gpui;

          helix-gpui-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          helix-gpui-fmt = craneLib.cargoFmt {
            inherit src;
          };
        };
      });
}

