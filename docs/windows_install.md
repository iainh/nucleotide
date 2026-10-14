# Windows Portable Install

Nucleotide publishes a Windows Velopack portable bundle and update artifacts
from CI. It does not publish a Windows setup executable or MSI.

## Install

1. Download the Windows `*-Portable.zip` bundle from the release.
2. Extract the entire archive to a directory where your account has write
   access.
3. Launch `Nucleotide.exe` from the extracted directory.

Keep the extracted directory structure intact so Velopack can apply updates in
place. The portable bundle is built from the checked-in Velopack packaging
script. It stages `nucl.exe`, `ghostty-vt.dll`, Linux remote helpers for SSH and
WSL, and the bundled Helix runtime, then runs `vpk pack --noInst`.

Nucleotide detects that bundled runtime automatically when launched from the
packaged app directory.

Nucleotide sets a stable Windows AppUserModelID (`org.spiralpoint.nucleotide`)
at startup so taskbar grouping and Jump Lists use the same identity across
user-created shortcuts and direct launches. When Nucleotide is already running,
launching `nucl.exe` again with files, folders, or taskbar Jump List actions
forwards that request to the running window instead of creating a second
independent instance.

When a project folder is opened, Nucleotide reports it to Windows Recent Items
and updates the taskbar Jump List `Recent Folders` category. On Windows,
Nucleotide also publishes `Open...` and `Open Directory...` taskbar Jump List
tasks when the app starts.

## Build Locally

Install Rust stable, Zig 0.16.0, the .NET 8 SDK, and the Velopack CLI, then
prepare the runtime resources that the package will embed:

```powershell
cargo build --release -p nucleotide --bins
dotnet tool update -g vpk
.\scripts\clone-helix-runtime.ps1 -Destination helix-temp
try {
  .\scripts\setup-windows-runtime.cmd -RuntimeSource helix-temp\runtime -NuclExe target\release\nucl-grammar.exe
} finally {
  Remove-Item -LiteralPath helix-temp -Recurse -Force
}
```

Then build the portable Velopack package:

```powershell
.\scripts\package-velopack.ps1 -RequireRemoteHelpers
```

The package build discovers `ghostty-vt.dll` from the release build output. If
a custom build places the DLL elsewhere, pass it explicitly:

```powershell
.\scripts\package-velopack.ps1 -GhosttyDll path\to\ghostty-vt.dll
```

Velopack output is written to:

```text
target\release\bundle\velopack
```

The output includes a `*-Portable.zip` bundle plus the packages and feed needed
for updates. The packaging script rejects any generated `Setup.exe` or MSI.

`setup-windows-runtime.cmd` copies the Helix runtime into
`crates\nucleotide\runtime`, uses the supplied grammar-capable executable to
build Windows tree-sitter grammar DLLs there, copies Nucleotide themes, and
removes grammar source checkouts before bundling.

### Windows ARM64 builds

The native ARM64 Windows Zig 0.16.0 compiler crashes during compilation
([upstream issue](https://codeberg.org/ziglang/zig/issues/31865)). Use the
installer below to download checksum-verified x64 Zig, correct a missing
ARM64 alignment cast in its Windows debug support and add it to the current
session's `PATH`. Use the x64 Rust host toolchain under Windows emulation:

```powershell
.\scripts\install-windows-arm64-zig.ps1 -Destination target\build-tools\zig-x64
rustup toolchain install 1.96.0-x86_64-pc-windows-msvc --force-non-host --profile minimal --target aarch64-pc-windows-msvc
cargo +1.96.0-x86_64-pc-windows-msvc check --workspace --all-targets --locked --target aarch64-pc-windows-msvc
cargo +1.96.0-x86_64-pc-windows-msvc build --release -p nucleotide --bins --locked --target aarch64-pc-windows-msvc
```

Install the MSVC ARM64 build tools and Windows SDK for linking. Both host
toolchains must be x64: with native ARM64 Rust, the Ghostty build script omits
the Zig target argument, so x64 Zig would build an x64 library instead. The
explicit Cargo target above produces ARM64 binaries in
`target\aarch64-pc-windows-msvc\release`. CI uses the same workaround for its
Windows ARM64 workspace check. Published Windows bundles still target x64.

## Configuration

Nucleotide uses Helix's Windows config directory:

```powershell
$env:APPDATA\helix\nucleotide.toml
```

GUI-specific settings live in `nucleotide.toml`; editor settings continue to use
Helix's `config.toml` in the same directory.

## Keyboard shortcuts

Non-editor text fields support `Ctrl+Z` to undo and either `Ctrl+Y` or
`Ctrl+Shift+Z` to redo. In pickers, `Ctrl+Shift+P` toggles the preview;
`Ctrl+P` and `Ctrl+N` continue to select the previous and next results.
These shortcuts do not change Helix's modal editor bindings.

## Clipboard

Path-copy commands in the file tree and tab menus, and the LSP log-directory
button, use the native clipboard. Non-ASCII paths are preserved without relying
on command-line clipboard tools or their code-page settings.

## Background tools

Local workspace commands and Git queries run without attaching a console
window. Commands launched intentionally in the terminal panel still use that
terminal.

## Logs

Nucleotide writes daily application logs to the local application data
directory:

```powershell
$env:LOCALAPPDATA\Spiralpoint\Nucleotide\logs
```

Files use a UTC date suffix, such as `nucleotide.log.2026-07-15`. Nucleotide
retains the five most recent daily log files by default.

Set `NUCLEOTIDE_LOG_DIR` to override the log directory for troubleshooting or
automated testing.

`nucleotide-remote` also writes daily logs on the machine where the helper
runs. On WSL and Linux SSH hosts, the directory is:

```text
$XDG_STATE_HOME/nucleotide/logs
```

If `XDG_STATE_HOME` is not set, the helper uses
`~/.local/state/nucleotide/logs`. Remote files use names such as
`nucleotide-remote.log.2026-07-15`. If the helper cannot create its log file,
it falls back to stderr without writing diagnostic output to the protocol's
stdout stream.
