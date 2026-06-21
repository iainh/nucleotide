# Windows Install

Nucleotide publishes a Windows zip package named `nucleotide-windows-x86_64.zip`.

## Install

1. Extract the zip to a stable directory, for example `%LOCALAPPDATA%\Programs\Nucleotide`.
2. Run `nucl.exe` from that directory. `nucl.cmd` remains available for terminal launches.

The package keeps its Helix runtime beside the executable, and `nucl.exe` detects that bundled runtime automatically.
The packaged `nucl.exe` is patched with Nucleotide icon resources, and the zip also includes `nucleotide.ico` for Explorer shell menu and Open With registrations.
Nucleotide sets a stable Windows AppUserModelID (`org.spiralpoint.nucleotide`) at startup so taskbar grouping and Jump Lists use the same identity across shortcuts, Explorer launches, and terminal launches.
When Nucleotide is already running, launching `nucl.cmd` or `nucl.exe` again with files, folders, or taskbar Jump List actions forwards that request to the running window instead of creating a second independent instance.
When a project folder is opened, Nucleotide also reports it to Windows Recent Items and updates the taskbar Jump List `Recent Folders` category.
On Windows, Nucleotide also publishes `Open...` and `Open Directory...` taskbar Jump List tasks when the app starts.

## Configuration

Nucleotide uses Helix's Windows config directory:

```powershell
$env:APPDATA\helix\nucleotide.toml
```

GUI-specific settings live in `nucleotide.toml`; editor settings continue to use Helix's `config.toml` in the same directory.

## Explorer Integration

To add per-user Explorer entries, run:

```powershell
.\install-windows-context-menu.cmd
```

This registers:

- `Open with Nucleotide` for files.
- `Open with Nucleotide` for folders.
- `Open with Nucleotide` in folder background context menus.
- `Open with Nucleotide` for drive roots.
- Nucleotide as an `Open With` application.
- `nucl.exe` as a per-user Windows App Paths entry for shell launches.

The script writes only to current-user registry locations under `HKCU`, so it does not require administrator privileges.
Explorer entries and Start Menu shortcuts launch `nucl.exe` directly, avoiding the console window used by batch-file launchers.
The App Paths entry lets Windows shell launchers locate `nucl.exe` without changing terminal `PATH`.

To also add Nucleotide to the `Open With` picker for common source and config file extensions without changing default apps:

```powershell
.\install-windows-context-menu.cmd -RegisterCommonFileTypes
```

To also create a per-user Start Menu shortcut:

```powershell
.\install-windows-context-menu.cmd -StartMenuShortcut
```

The shortcut is stamped with the same AppUserModelID as the application so taskbar grouping and Jump Lists stay consistent.

To also add the install directory to the current user's `PATH` for terminal launches:

```powershell
.\install-windows-context-menu.cmd -AddToPath
```

Open a new terminal after this step to use `nucl.exe` or `nucl.cmd` from `PATH`. The uninstall step removes the `PATH` entry only when this script added it.

To remove the entries:

```powershell
.\install-windows-context-menu.cmd -Uninstall
```

If the script is run from outside the package directory, pass `-InstallDir`:

```powershell
.\install-windows-context-menu.cmd -InstallDir "$env:LOCALAPPDATA\Programs\Nucleotide"
```
