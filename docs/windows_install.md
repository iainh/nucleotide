# Windows Install

Nucleotide publishes Windows Velopack setup and update artifacts from CI.

## Install

1. Download the Windows Velopack setup executable from the release.
2. Run the installer.
3. Launch Nucleotide from the Start Menu or desktop shortcut.

The installer is built from the checked-in Velopack packaging script. It stages
`nucl.exe`, `ghostty-vt.dll`, Linux SSH remote helpers, and the bundled Helix
runtime, then runs `vpk pack`.

Nucleotide detects that bundled runtime automatically when launched from the
installed app directory.

Nucleotide sets a stable Windows AppUserModelID (`org.spiralpoint.nucleotide`)
at startup so taskbar grouping and Jump Lists use the same identity across
shortcuts and direct launches. When Nucleotide is already running, launching
`nucl.exe` again with files, folders, or taskbar Jump List actions forwards that
request to the running window instead of creating a second independent
instance.

When a project folder is opened, Nucleotide reports it to Windows Recent Items
and updates the taskbar Jump List `Recent Folders` category. On Windows,
Nucleotide also publishes `Open...` and `Open Directory...` taskbar Jump List
tasks when the app starts.

## Build Locally

Install Rust stable, Zig 0.15.2, the .NET 8 SDK, and the Velopack CLI, then
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

Then build the Velopack package:

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

`setup-windows-runtime.cmd` copies the Helix runtime into
`crates\nucleotide\runtime`, uses the supplied grammar-capable executable to
build Windows tree-sitter grammar DLLs there, copies Nucleotide themes, and
removes grammar source checkouts before bundling.

## Configuration

Nucleotide uses Helix's Windows config directory:

```powershell
$env:APPDATA\helix\nucleotide.toml
```

GUI-specific settings live in `nucleotide.toml`; editor settings continue to use
Helix's `config.toml` in the same directory.
