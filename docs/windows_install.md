# Windows Install

Nucleotide publishes a Windows MSI installer named
`Nucleotide-windows-x86_64.msi`.

## Install

1. Download `Nucleotide-windows-x86_64.msi`.
2. Run the installer.
3. Launch Nucleotide from the Start Menu or desktop shortcut.

The installer is built with `cargo-bundle`'s WiX-backed MSI format. It installs
`nucl.exe` and the bundled Helix runtime together under the application install
directory. Nucleotide detects that bundled runtime automatically when launched
from the installer shortcuts or directly from the install directory.

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

Install the .NET 8 SDK and cargo-bundle, then prepare the runtime resources
that the MSI will embed:

```powershell
cargo install cargo-bundle --version 0.11.0 --locked
cargo build --release -p nucleotide
git clone --depth 1 --branch 25.07.1 https://github.com/helix-editor/helix.git helix-temp
try {
  .\scripts\setup-windows-runtime.cmd -RuntimeSource helix-temp\runtime -NuclExe target\release\nucl.exe
} finally {
  Remove-Item -LiteralPath helix-temp -Recurse -Force
}
```

Then run cargo-bundle from the application crate:

```powershell
Push-Location crates\nucleotide
cargo bundle --release --format wxsmsi
Pop-Location
```

The installer is written to:

```text
target\release\bundle\wxsmsi\bin\Release\nucleotide.msi
```

`setup-windows-runtime.cmd` copies the Helix runtime into
`crates\nucleotide\runtime`, uses the supplied `nucl.exe` to build Windows
tree-sitter grammar DLLs there, copies Nucleotide themes, and removes grammar
source checkouts before bundling.

## Configuration

Nucleotide uses Helix's Windows config directory:

```powershell
$env:APPDATA\helix\nucleotide.toml
```

GUI-specific settings live in `nucleotide.toml`; editor settings continue to use
Helix's `config.toml` in the same directory.
