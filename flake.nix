{
  description = "Nucleotide - A Native GUI for the Helix editor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    zig-overlay = {
      url = "github:mitchellh/zig-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    crane.url = "github:ipetkov/crane";

    ghostty = {
      url = "github:ghostty-org/ghostty/fdbf9ff3a31d7531b691cb49c98fc465a1a503a0";
      flake = false;
    };

    # Helix repository for runtime files
    helix = {
      url = "github:helix-editor/helix/25.07.1";
      flake = false;
    };

  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      zig-overlay,
      flake-utils,
      helix,
      crane,
      ghostty,
    }:
    flake-utils.lib.eachSystem [ "aarch64-darwin" "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
          config = {
            allowUnfree = true;
          };
        };

        rustTargets = [
          "x86_64-unknown-linux-musl"
          "aarch64-unknown-linux-musl"
        ];

        # Keep Cargo metadata, local development, Nix, and CI on one compiler.
        rustVersion = "1.95.0";
        rustToolchain = pkgs.rust-bin.stable.${rustVersion}.default.override {
          extensions = [
            "clippy"
            "rust-analyzer"
            "rust-src"
            "rustfmt"
          ];
          targets = rustTargets;
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Dependency management following Helix patterns
        inherit (pkgs) lib stdenv;
        # libghostty-vt-sys 0.2.0 requires Zig 0.15.2 exactly. Keep this
        # binding explicit so `with pkgs` cannot silently select nixpkgs Zig.
        zig_0_15_2 = zig-overlay.packages.${system}."0.15.2";
        ghosttyZigDeps = pkgs.callPackage "${ghostty}/build.zig.zon.nix" {
          zig_0_15 = zig_0_15_2;
          name = "ghostty-cache-libghostty-vt-sys-0.2.0";
        };

        # Keep Nix's cc wrapper so library paths and SDK flags are preserved,
        # while asking clang to use LLD's Darwin linker underneath.
        darwinRustLinker = "${stdenv.cc}/bin/cc";
        darwinRustLinkerFlags = "-C link-arg=-fuse-ld=${pkgs.lld}/bin/ld64.lld";
        darwinRustLinkerEnv = lib.optionalAttrs stdenv.isDarwin {
          CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = darwinRustLinker;
          CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS = darwinRustLinkerFlags;
          CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER = darwinRustLinker;
          CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS = darwinRustLinkerFlags;
        };

        # Common build inputs
        commonBuildInputs = with pkgs; [
          openssl
          pkg-config
          git
          curl
          sqlite
        ];

        # Platform-specific build inputs
        darwinBuildInputs =
          with pkgs;
          lib.optionals stdenv.isDarwin [
            libiconv
            # Modern Apple SDK - the hooks will ensure proper framework linking
            apple-sdk
          ];

        linuxBuildInputs =
          with pkgs;
          lib.optionals stdenv.isLinux [
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

        # Combined build inputs
        allBuildInputs = commonBuildInputs ++ darwinBuildInputs ++ linuxBuildInputs;

        # Version info
        version = "0.2.1";
        appName = "Nucleotide";
        bundleId = "org.spiralpoint.nucleotide";

        # Helix runtime files
        helixRuntime = pkgs.stdenv.mkDerivation {
          name = "helix-runtime";
          src = helix;

          buildPhase = ''
            # Helix stores the canonical language table at the repository root.
            # Older assumptions looked under runtime/ and produced a Rust-only
            # fallback, which breaks grammar metadata for packaged runtimes.
            if [ -f languages.toml ]; then
              true
            elif [ -f runtime/languages.toml ]; then
              cp runtime/languages.toml ./languages.toml
            else
              echo "error: Helix languages.toml not found" >&2
              exit 1
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

        # Crane turns the dependency graph into a reusable derivation shared by
        # every CI check. Include the complete workspace and vendored sources so
        # GPUI shaders and Helix query fixtures remain available to build scripts.
        ciSource = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.lock
            ./Cargo.toml
            ./crates
            ./docs
            ./vendor
          ];
        };

        ciCommonArgs = {
          pname = "nucleotide";
          inherit version;
          inherit cargoVendorDir;
          src = ciSource;
          strictDeps = true;
          CARGO_PROFILE = "dev";
          CARGO_PROFILE_DEV_DEBUG = "false";
          CARGO_PROFILE_DEV_OPT_LEVEL = "0";
          doInstallCargoArtifacts = false;
          cargoExtraArgs = "--locked";
          nativeBuildInputs =
            with pkgs;
            [
              clang
              git
              pkg-config
              zig_0_15_2
            ]
            ++ lib.optionals stdenv.isDarwin [
              darwin.cctools
              xcbuild
            ];
          buildInputs = allBuildInputs;
          HELIX_RUNTIME = "${helixRuntime}";
          GHOSTTY_SOURCE_DIR = "${ghostty}";
          GHOSTTY_ZIG_SYSTEM_DIR = "${ghosttyZigDeps}";
          preBuild = ''
            export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-global-cache"
            export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
          '';
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          OPENSSL_NO_VENDOR = 1;
          RUSTFLAGS = "--cfg tokio_unstable";
        }
        // darwinRustLinkerEnv;

        # Vendored crates are workspace members but function as third-party
        # dependencies. Preserve them in Crane's otherwise-empty dummy workspace
        # so their real APIs remain available while application sources stay cached.
        ciVendorSource = lib.fileset.toSource {
          root = ./vendor;
          fileset = ./vendor;
        };

        # The vendored Helix LSP crate uses this adapter as a dependency.
        ciProcessSource = lib.fileset.toSource {
          root = ./crates/nucleotide-process;
          fileset = ./crates/nucleotide-process;
        };

        ciDummySource = craneLib.mkDummySrc {
          src = ciSource;
          extraDummyScript = ''
            rm -rf "$out/vendor"
            mkdir -p "$out/vendor"
            cp -R ${ciVendorSource}/. "$out/vendor/"
            rm -rf "$out/crates/nucleotide-process"
            mkdir -p "$out/crates/nucleotide-process"
            cp -R ${ciProcessSource}/. "$out/crates/nucleotide-process/"
          '';
        };

        cargoVendorDir = craneLib.vendorCargoDeps {
          src = ciSource;
          overrideVendorGitCheckout =
            packages: drv:
            if lib.any (package: package.name == "helix-loader") packages then
              drv.overrideAttrs (old: {
                postInstall = (old.postInstall or "") + ''
                  # helix-loader embeds this repository-root file via ../../.
                  cp languages.toml "$out/languages.toml"
                '';
              })
            else
              drv;
        };

        cargoArtifacts = craneLib.buildDepsOnly (
          ciCommonArgs
          // {
            inherit cargoVendorDir;
            cargoExtraArgs = "--locked --workspace";
            dummySrc = ciDummySource;
            buildPhaseCargoCommand = "cargoWithProfile check --locked --workspace --all-targets";
          }
        );

        ciApplication = craneLib.buildPackage (
          ciCommonArgs
          // {
            inherit cargoArtifacts cargoVendorDir;
            cargoExtraArgs = "--locked --workspace";
            doCheck = false;
          }
        );

        # Build script that produces the binary
        buildScript = pkgs.writeScriptBin "build-nucleotide" ''
          #!${pkgs.stdenv.shell}
          set -e

          export PATH="${zig_0_15_2}/bin:${rustToolchain}/bin:${pkgs.pkg-config}/bin:${pkgs.git}/bin:$PATH"
          export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig"
          export OPENSSL_NO_VENDOR=1
          export HELIX_RUNTIME="${helixRuntime}"
          ${lib.optionalString stdenv.isDarwin ''
            export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="${darwinRustLinker}"
            export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="${darwinRustLinkerFlags}"
            export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="${darwinRustLinker}"
            export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="${darwinRustLinkerFlags}"
          ''}

          # Platform-specific setup
          ${lib.optionalString stdenv.isDarwin ''
            export DYLD_LIBRARY_PATH="${lib.makeLibraryPath darwinBuildInputs}:$DYLD_LIBRARY_PATH"
          ''}
          ${lib.optionalString stdenv.isLinux ''
            export LD_LIBRARY_PATH="${lib.makeLibraryPath linuxBuildInputs}:$LD_LIBRARY_PATH"
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

          remote_helper_dir="''${NUCL_REMOTE_HELPER_DIR:-target/remote-helpers}"
          remote_helpers_required="''${NUCL_REQUIRE_REMOTE_HELPERS:-0}"
          remote_helpers_copied=0

          if [ -d "$remote_helper_dir" ]; then
            echo "Copying SSH remote helpers from $remote_helper_dir..."
            for helper in nucleotide-remote-linux-x86_64 nucleotide-remote-linux-aarch64; do
              if [ -f "$remote_helper_dir/$helper" ]; then
                cp "$remote_helper_dir/$helper" "Nucleotide.app/Contents/MacOS/$helper"
                chmod +x "Nucleotide.app/Contents/MacOS/$helper"
                remote_helpers_copied=$((remote_helpers_copied + 1))
                echo "  - $helper"
              elif [ "$remote_helpers_required" = "1" ]; then
                echo "Error: required SSH remote helper not found: $remote_helper_dir/$helper" >&2
                exit 1
              fi
            done
          elif [ "$remote_helpers_required" = "1" ]; then
            echo "Error: required SSH remote helper directory not found: $remote_helper_dir" >&2
            exit 1
          else
            echo "Warning: SSH remote helper directory not found at $remote_helper_dir" >&2
          fi

          if [ "$remote_helpers_required" = "1" ] && [ "$remote_helpers_copied" -ne 2 ]; then
            echo "Error: expected 2 SSH remote helpers, copied $remote_helpers_copied" >&2
            exit 1
          fi

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
          if [ ! -f "target/release/nucleotide-remote" ]; then
            echo "Error: Remote helper not found. Run 'nix develop --command cargo build --release -p nucleotide --bins -p nucleotide-remote' first"
            exit 1
          fi

          echo "Creating Linux package..."

          # Clean up any existing package
          rm -rf nucleotide-linux nucleotide-linux.tar.gz

          # Create directory structure
          mkdir -p nucleotide-linux/{bin,share/{applications,nucleotide}}

          # Copy binary
          cp target/release/nucl nucleotide-linux/bin/
          cp target/release/nucleotide-remote nucleotide-linux/bin/

          remote_helper_dir="''${NUCL_REMOTE_HELPER_DIR:-target/remote-helpers}"
          if [ -d "$remote_helper_dir" ]; then
            for helper in nucleotide-remote-linux-x86_64 nucleotide-remote-linux-aarch64; do
              if [ -f "$remote_helper_dir/$helper" ]; then
                cp "$remote_helper_dir/$helper" "nucleotide-linux/bin/$helper"
                chmod +x "nucleotide-linux/bin/$helper"
              fi
            done
          else
            echo "Warning: SSH remote helper directory not found at $remote_helper_dir" >&2
          fi

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

        # SSH remote helper builder
        buildRemoteHelpers = pkgs.writeScriptBin "build-remote-helpers" ''
          #!${pkgs.stdenv.shell}
          set -e

          export PATH="${zig_0_15_2}/bin:$PATH"
          exec ./scripts/build-remote-helpers.sh "$@"
        '';

        installVelopackCli = pkgs.writeScriptBin "install-velopack-cli" ''
          #!${pkgs.stdenv.shell}
          set -euo pipefail

          export DOTNET_ROOT="${pkgs.dotnet-sdk_8}/share/dotnet"
          tool_path="''${NUCLEOTIDE_DOTNET_TOOL_PATH:-$PWD/.dotnet-tools}"
          mkdir -p "$tool_path"

          exec ${pkgs.dotnet-sdk_8}/bin/dotnet tool update vpk --tool-path "$tool_path" "$@"
        '';

        velopackCli = pkgs.writeScriptBin "vpk" ''
          #!${pkgs.stdenv.shell}
          set -euo pipefail

          export DOTNET_ROOT="${pkgs.dotnet-sdk_8}/share/dotnet"
          tool_path="''${NUCLEOTIDE_DOTNET_TOOL_PATH:-$PWD/.dotnet-tools}"
          executable="$tool_path/vpk"

          if [ -x "$executable" ]; then
            exec "$executable" "$@"
          fi

          echo "Velopack CLI is not installed at $executable." >&2
          echo "Run: install-velopack-cli" >&2
          exit 127
        '';

      in
      {
        packages = {
          ci = ciApplication;
          runtime = helixRuntime;
          buildScript = buildScript;
          makeMacOSBundle = makeMacOSBundle;
          makeLinuxPackage = makeLinuxPackage;
          buildRemoteHelpers = buildRemoteHelpers;
          installVelopackCli = installVelopackCli;
          velopackCli = velopackCli;
        };

        checks = {
          build = ciApplication;

          cargo-check = craneLib.mkCargoDerivation (
            ciCommonArgs
            // {
              inherit cargoArtifacts;
              pnameSuffix = "-check";
              buildPhaseCargoCommand = "cargoWithProfile check --workspace --all-targets --locked";
            }
          );

          cargo-clippy = craneLib.cargoClippy (
            ciCommonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
            }
          );

          cargo-doc = craneLib.cargoDoc (
            ciCommonArgs
            // {
              inherit cargoArtifacts;
              cargoDocExtraArgs = "--workspace --no-deps";
              RUSTDOCFLAGS = "--deny warnings";
            }
          );

          cargo-fmt = craneLib.cargoFmt {
            pname = "nucleotide";
            inherit version;
            src = ciSource;
          };

          cargo-test = craneLib.cargoTest (
            ciCommonArgs
            // {
              inherit cargoArtifacts;
              cargoTestExtraArgs = "--workspace -- --skip tests::performance_tests --skip tests::integration_tests::tests::performance_tests --skip tests::command_session_runs_program_args_and_reports_exit_code --skip tests::command_session_try_exit_code_reports_finished_child";
            }
          );
        };

        # CI and packaging only need deterministic compilers, native libraries,
        # and repository build helpers. Keep this separate from the ergonomic
        # development shell so optional developer tools cannot break CI startup.
        devShells.ci = pkgs.mkShell (
          {
            packages = with pkgs; [
              rustToolchain
              zig_0_15_2
              cargo-zigbuild
              clang
              git
              pkg-config
              buildRemoteHelpers
              makeLinuxPackage
            ];
            buildInputs = allBuildInputs;
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            HELIX_RUNTIME = "${helixRuntime}";
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            OPENSSL_NO_VENDOR = 1;
          }
          // darwinRustLinkerEnv
        );

        devShells.default =
          pkgs.mkShell (
            {
              packages =
                with pkgs;
                [
                  # Rust toolchain (includes rust-analyzer and rust-src)
                  rustToolchain

                  # Development tools
                  cargo-nextest
                  cargo-edit
                  cargo-outdated
                  cargo-deny
                  cargo-flamegraph
                  cargo-machete
                  cargo-zigbuild

                  # Build performance tools
                  sccache

                  # Velopack packaging tools
                  dotnet-sdk_8
                  installVelopackCli
                  velopackCli

                  # For running the application
                  ripgrep
                  tree-sitter
                  zig_0_15_2

                  # Build helpers
                  buildScript
                  makeMacOSBundle
                  makeLinuxPackage
                  buildRemoteHelpers

                  # Platform-specific tools
                ]
                ++ lib.optionals (lib.meta.availableOn stdenv.hostPlatform powershell) [
                  powershell
                ]
                ++ lib.optionals stdenv.isDarwin [
                  darwin.DarwinTools
                  xcbuild
                  lld
                  lldb # Debugging on macOS
                ]
                ++ lib.optionals stdenv.isLinux [
                  cargo-tarpaulin # Test coverage
                  gdb # Debugging on Linux
                ];

              buildInputs = allBuildInputs;

              # Development environment variables
              RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
              HELIX_RUNTIME = "${helixRuntime}";
              PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
              OPENSSL_NO_VENDOR = 1;
              DOTNET_CLI_TELEMETRY_OPTOUT = 1;
              DOTNET_NOLOGO = 1;
              DOTNET_ROOT = "${pkgs.dotnet-sdk_8}/share/dotnet";
              NUCLEOTIDE_DOTNET_TOOL_PATH = ".dotnet-tools";

              # Build performance settings following Helix patterns
              CARGO_INCREMENTAL = "1"; # Default to incremental for dev builds

            }
            // darwinRustLinkerEnv
          )
          // {
            shellHook = ''
              # Keep Ghostty's required Zig ahead of any host-level installation.
              export PATH="${zig_0_15_2}/bin:$PATH"

              case "$NUCLEOTIDE_DOTNET_TOOL_PATH" in
                /*) ;;
                *) export NUCLEOTIDE_DOTNET_TOOL_PATH="$PWD/$NUCLEOTIDE_DOTNET_TOOL_PATH" ;;
              esac
              export PATH="$NUCLEOTIDE_DOTNET_TOOL_PATH:$PATH"

              # Define build mode aliases
              alias build-cached='CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache cargo build'
              alias build-incremental='unset RUSTC_WRAPPER && CARGO_INCREMENTAL=1 cargo build'
              alias build-release-cached='CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache cargo build --release'
              alias build-release-incremental='unset RUSTC_WRAPPER && cargo build --release'

              # Always show welcome message to stderr (visible even in non-interactive mode)
              echo "╔════════════════════════════════════════════════════════════════╗" >&2
              echo "║         Welcome to Nucleotide development environment!         ║" >&2
              echo "╚════════════════════════════════════════════════════════════════╝" >&2
              echo "" >&2
              echo "Standard commands:" >&2
              echo "  cargo build --release        - Build with incremental compilation (default)" >&2
              echo "  cargo run                    - Run debug version" >&2
              echo "  cargo test                   - Run tests" >&2
              echo "  cargo clippy                 - Run linter" >&2
              echo "  cargo fmt                    - Format code" >&2
              echo "" >&2
              echo "Optimized build commands:" >&2
              echo "  build-incremental            - Dev build with incremental compilation (best for iterative dev)" >&2
              echo "  build-cached                 - Dev build with sccache (best for branch switches)" >&2
              echo "  build-release-incremental    - Release build with incremental" >&2
              echo "  build-release-cached         - Release build with sccache" >&2
              echo "" >&2
              echo "Bundle creation:" >&2
              echo "  build-remote-helpers        - Build Linux SSH helper binaries" >&2
              echo "  make-macos-bundle            - Create macOS .app bundle" >&2
              echo "  make-linux-package           - Create Linux distribution" >&2
              echo "  install-velopack-cli         - Install/update vpk into .dotnet-tools" >&2
              echo "  ./scripts/package-velopack.sh - Create macOS Velopack package from Nucleotide.app" >&2
              echo "" >&2
              echo "Build optimizations enabled (following Helix patterns):" >&2
              echo "  • Thin LTO for release builds (faster than full LTO)" >&2
              echo "  • Split debuginfo for macOS (faster linking)" >&2
              echo "  • Incremental compilation (default) or sccache (use aliases)" >&2
              ${lib.optionalString stdenv.isDarwin ''echo "  • LLDB debugging available on macOS" >&2''}
              echo "" >&2
              echo "Development tools added from Helix:" >&2
              echo "  • cargo-flamegraph (performance profiling)" >&2
              ${lib.optionalString stdenv.isLinux ''echo "  • cargo-tarpaulin (test coverage)" >&2''}
              ${lib.optionalString stdenv.isDarwin ''echo "  • lldb (debugging)" >&2''}
              ${lib.optionalString stdenv.isLinux ''echo "  • gdb (debugging)" >&2''}
              echo "" >&2
              echo "Nix packages:" >&2
              echo "  nix build .#runtime          - Build runtime files" >&2
              echo "" >&2
              echo "Runtime files available at: $HELIX_RUNTIME" >&2
            '';
          };
      }
    );
}
