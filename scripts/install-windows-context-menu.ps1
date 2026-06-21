param(
    [Alias("h", "?")]
    [switch]$Help,
    [switch]$Uninstall,
    [switch]$StartMenuShortcut,
    [switch]$RegisterCommonFileTypes,
    [switch]$AddToPath,
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host @"
Usage: .\install-windows-context-menu.cmd [options]

Adds or removes per-user Windows shell integration for Nucleotide:
  Open with Nucleotide on files
  Open with Nucleotide on folders
  Open with Nucleotide in folder background context menus
  Nucleotide as an Open With application
  Nucleotide App Paths registration for Windows shell launches
  Optional Open With registration for common source and config file types
  Optional Start Menu shortcut
  Optional user PATH entry for terminal launches

Options:
  -InstallDir <path>         Directory containing nucl.exe. Defaults to this script's directory.
  -RegisterCommonFileTypes   Add Nucleotide to Open With for common source/config file extensions.
  -StartMenuShortcut         Create a per-user Start Menu shortcut that launches nucl.exe.
  -AddToPath                 Add the install directory to the current user's PATH.
  -Uninstall                 Remove Nucleotide shell integration for the current user, including PATH entries added by this script.
  -Help                      Show this help text.
"@
    exit 0
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = $PSScriptRoot
}

$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
$ExePath = Join-Path $InstallDir "nucl.exe"
$LauncherPath = $ExePath
$IconPath = Join-Path $InstallDir "nucleotide.ico"
$AppUserModelId = "org.spiralpoint.nucleotide"

if (-not $Uninstall) {
    if (-not (Test-Path -LiteralPath $ExePath)) {
        throw "Could not find nucl.exe in: $InstallDir"
    }

    if (-not (Test-Path -LiteralPath $IconPath)) {
        $IconPath = $ExePath
    }
}

function Quote-CommandPart {
    param([string]$Value)

    '"' + $Value.Replace('"', '\"') + '"'
}

function New-NucleotideCommand {
    param([string]$ArgumentToken)

    "$(Quote-CommandPart $LauncherPath) $ArgumentToken"
}

function Set-RegistryString {
    param(
        [string]$SubKey,
        [string]$Name,
        [string]$Value
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($SubKey)
    if (-not $key) {
        throw "Could not create registry key: HKCU\$SubKey"
    }

    try {
        $key.SetValue($Name, $Value, [Microsoft.Win32.RegistryValueKind]::String)
    } finally {
        $key.Dispose()
    }
}

function Set-RegistryExpandString {
    param(
        [string]$SubKey,
        [string]$Name,
        [string]$Value
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($SubKey)
    if (-not $key) {
        throw "Could not create registry key: HKCU\$SubKey"
    }

    try {
        $key.SetValue($Name, $Value, [Microsoft.Win32.RegistryValueKind]::ExpandString)
    } finally {
        $key.Dispose()
    }
}

function Set-RegistryNone {
    param(
        [string]$SubKey,
        [string]$Name
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($SubKey)
    if (-not $key) {
        throw "Could not create registry key: HKCU\$SubKey"
    }

    try {
        $key.SetValue($Name, [byte[]]@(), [Microsoft.Win32.RegistryValueKind]::None)
    } finally {
        $key.Dispose()
    }
}

function Get-RegistryString {
    param(
        [string]$SubKey,
        [string]$Name
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($SubKey)
    if (-not $key) {
        return $null
    }

    try {
        $value = $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($null -eq $value) {
            return $null
        }

        [string]$value
    } finally {
        $key.Dispose()
    }
}

function Remove-RegistryValue {
    param(
        [string]$SubKey,
        [string]$Name
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($SubKey, $true)
    if (-not $key) {
        return
    }

    try {
        $key.DeleteValue($Name, $false)
    } finally {
        $key.Dispose()
    }
}

function Remove-RegistryTree {
    param([string]$SubKey)

    try {
        [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($SubKey, $false)
    } catch [System.ArgumentException] {
        # Key already absent.
    }
}

function Split-PathList {
    param([string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return @()
    }

    @(
        $Value -split ';' |
            ForEach-Object { $_.Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
}

function Normalize-PathEntry {
    param([string]$Value)

    $expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim())

    try {
        [System.IO.Path]::GetFullPath($expanded).TrimEnd('\')
    } catch {
        $expanded.TrimEnd('\')
    }
}

function Test-PathEntryEquals {
    param(
        [string]$Left,
        [string]$Right
    )

    [string]::Equals(
        (Normalize-PathEntry $Left),
        (Normalize-PathEntry $Right),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Add-InstallDirToUserPath {
    $currentPath = Get-RegistryString -SubKey "Environment" -Name "Path"
    $entries = @(Split-PathList $currentPath)
    $alreadyPresent = @($entries | Where-Object { Test-PathEntryEquals $_ $InstallDir }).Count -gt 0

    if ($alreadyPresent) {
        return "already-present"
    }

    Set-RegistryExpandString -SubKey "Environment" -Name "Path" -Value (@($entries + $InstallDir) -join ';')
    Set-RegistryString -SubKey $SettingsKey -Name "PathEntryAddedByInstaller" -Value $InstallDir

    "added"
}

function Remove-InstallDirFromUserPath {
    $pathAddedByScript = Get-RegistryString -SubKey $SettingsKey -Name "PathEntryAddedByInstaller"
    if ([string]::IsNullOrWhiteSpace($pathAddedByScript)) {
        return "not-owned"
    }

    $currentPath = Get-RegistryString -SubKey "Environment" -Name "Path"
    $entries = @(Split-PathList $currentPath)
    $remaining = @($entries | Where-Object { -not (Test-PathEntryEquals $_ $pathAddedByScript) })

    if ($remaining.Count -eq $entries.Count) {
        Remove-RegistryValue -SubKey $SettingsKey -Name "PathEntryAddedByInstaller"
        return "not-present"
    }

    if ($remaining.Count -eq 0) {
        Remove-RegistryValue -SubKey "Environment" -Name "Path"
    } else {
        Set-RegistryExpandString -SubKey "Environment" -Name "Path" -Value ($remaining -join ';')
    }

    Remove-RegistryValue -SubKey $SettingsKey -Name "PathEntryAddedByInstaller"
    "removed"
}

function Get-StartMenuShortcutPath {
    $programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
    if ([string]::IsNullOrWhiteSpace($programs)) {
        throw "Could not locate the current user's Start Menu programs directory."
    }

    Join-Path $programs "Nucleotide.lnk"
}

function Set-ShortcutAppUserModelId {
    param(
        [string]$ShortcutPath,
        [string]$AppUserModelId
    )

    if (-not ("Nucleotide.WindowsShortcutProperties" -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Nucleotide
{
    [ComImport]
    [Guid("00021401-0000-0000-C000-000000000046")]
    internal class ShellLink
    {
    }

    [ComImport]
    [Guid("0000010B-0000-0000-C000-000000000046")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IPersistFile
    {
        [PreserveSig]
        int GetClassID(out Guid pClassID);

        [PreserveSig]
        int IsDirty();

        [PreserveSig]
        int Load([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, uint dwMode);

        [PreserveSig]
        int Save([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, bool fRemember);

        [PreserveSig]
        int SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string pszFileName);

        [PreserveSig]
        int GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string ppszFileName);
    }

    [ComImport]
    [Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IPropertyStore
    {
        [PreserveSig]
        int GetCount(out uint cProps);

        [PreserveSig]
        int GetAt(uint iProp, out PROPERTYKEY pkey);

        [PreserveSig]
        int GetValue(ref PROPERTYKEY key, out PROPVARIANT pv);

        [PreserveSig]
        int SetValue(ref PROPERTYKEY key, ref PROPVARIANT pv);

        [PreserveSig]
        int Commit();
    }

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    internal struct PROPERTYKEY
    {
        public Guid fmtid;
        public uint pid;
    }

    [StructLayout(LayoutKind.Explicit)]
    internal struct PROPVARIANT
    {
        [FieldOffset(0)]
        public ushort vt;

        [FieldOffset(8)]
        public IntPtr p;
    }

    public static class WindowsShortcutProperties
    {
        private const ushort VT_LPWSTR = 31;

        [DllImport("ole32.dll")]
        private static extern int PropVariantClear(ref PROPVARIANT pvar);

        private static PROPVARIANT PropVariantFromString(string value)
        {
            return new PROPVARIANT
            {
                vt = VT_LPWSTR,
                p = Marshal.StringToCoTaskMemUni(value)
            };
        }

        public static void SetAppUserModelId(string shortcutPath, string appUserModelId)
        {
            object shellLink = new ShellLink();
            try
            {
                IPersistFile file = (IPersistFile)shellLink;
                Marshal.ThrowExceptionForHR(file.Load(shortcutPath, 0));

                IPropertyStore store = (IPropertyStore)shellLink;
                PROPERTYKEY appIdKey = new PROPERTYKEY
                {
                    fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"),
                    pid = 5
                };

                PROPVARIANT value = PropVariantFromString(appUserModelId);
                try
                {
                    Marshal.ThrowExceptionForHR(store.SetValue(ref appIdKey, ref value));
                    Marshal.ThrowExceptionForHR(store.Commit());
                    Marshal.ThrowExceptionForHR(file.Save(shortcutPath, true));
                }
                finally
                {
                    PropVariantClear(ref value);
                }
            }
            finally
            {
                if (shellLink != null)
                {
                    Marshal.ReleaseComObject(shellLink);
                }
            }
        }
    }
}
'@
    }

    [Nucleotide.WindowsShortcutProperties]::SetAppUserModelId($ShortcutPath, $AppUserModelId)
}

function New-NucleotideStartMenuShortcut {
    $shortcutPath = Get-StartMenuShortcutPath
    $shortcutDir = Split-Path -Parent $shortcutPath
    if (-not (Test-Path -LiteralPath $shortcutDir)) {
        New-Item -ItemType Directory -Path $shortcutDir | Out-Null
    }

    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $LauncherPath
        $shortcut.WorkingDirectory = $InstallDir
        $shortcut.IconLocation = $IconPath
        $shortcut.Description = "Nucleotide"
        $shortcut.Save()
    } finally {
        if ($shortcut) {
            [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shortcut)
        }
        [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }

    Set-ShortcutAppUserModelId -ShortcutPath $shortcutPath -AppUserModelId $AppUserModelId

    $shortcutPath
}

function Remove-StartMenuShortcut {
    $shortcutPath = Get-StartMenuShortcutPath
    if (Test-Path -LiteralPath $shortcutPath) {
        Remove-Item -LiteralPath $shortcutPath -Force
    }
}

$ExplorerEntries = @(
    @{
        Key = "Software\Classes\*\shell\Nucleotide"
        CommandKey = "Software\Classes\*\shell\Nucleotide\command"
        Argument = '"%1"'
    },
    @{
        Key = "Software\Classes\Directory\shell\Nucleotide"
        CommandKey = "Software\Classes\Directory\shell\Nucleotide\command"
        Argument = '"%1"'
    },
    @{
        Key = "Software\Classes\Directory\Background\shell\Nucleotide"
        CommandKey = "Software\Classes\Directory\Background\shell\Nucleotide\command"
        Argument = '"%V"'
    },
    @{
        Key = "Software\Classes\Drive\shell\Nucleotide"
        CommandKey = "Software\Classes\Drive\shell\Nucleotide\command"
        Argument = '"%V"'
    }
)

$ApplicationKey = "Software\Classes\Applications\nucl.exe"
$ApplicationIconKey = "$ApplicationKey\DefaultIcon"
$ApplicationCommandKey = "$ApplicationKey\shell\open\command"
$AppPathsKey = "Software\Microsoft\Windows\CurrentVersion\App Paths\nucl.exe"
$SettingsKey = "Software\SpiralPoint\Nucleotide"
$SourceFileProgId = "Nucleotide.SourceFile"
$SourceFileProgIdKey = "Software\Classes\$SourceFileProgId"
$CommonFileTypeExtensions = @(
    ".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx",
    ".cs", ".css", ".dart", ".diff", ".dockerfile", ".editorconfig",
    ".env", ".fs", ".fsi", ".fsx", ".go", ".gradle", ".groovy",
    ".hbs", ".html", ".ini", ".java", ".js", ".jsx", ".json", ".jsonc",
    ".kt", ".kts", ".less", ".lock", ".log", ".lua", ".m", ".md", ".mdx",
    ".ml", ".mli", ".php", ".pl", ".ps1", ".psd1", ".psm1", ".py", ".pyi",
    ".r", ".rb", ".rs", ".sass", ".scss", ".sh", ".sql", ".svelte",
    ".swift", ".toml", ".ts", ".tsx", ".txt", ".vue", ".xml", ".yaml",
    ".yml", ".zig", ".zsh"
)

if ($Uninstall) {
    foreach ($entry in $ExplorerEntries) {
        Remove-RegistryTree -SubKey $entry.Key
    }
    Remove-RegistryTree -SubKey $ApplicationKey
    Remove-RegistryTree -SubKey $AppPathsKey
    Remove-RegistryTree -SubKey $SourceFileProgIdKey
    $pathRemovalStatus = Remove-InstallDirFromUserPath
    Remove-RegistryTree -SubKey $SettingsKey
    foreach ($extension in $CommonFileTypeExtensions) {
        Remove-RegistryValue -SubKey "Software\Classes\$extension\OpenWithProgids" -Name $SourceFileProgId
    }
    Remove-StartMenuShortcut
    Write-Host "Removed Nucleotide Explorer integration for the current user."
    if ($pathRemovalStatus -eq "removed") {
        Write-Host "Removed Nucleotide from the current user's PATH. Open a new terminal to see the updated PATH."
    }
    exit 0
}

foreach ($entry in $ExplorerEntries) {
    Set-RegistryString -SubKey $entry.Key -Name "" -Value "Open with Nucleotide"
    Set-RegistryString -SubKey $entry.Key -Name "Icon" -Value $IconPath
    Set-RegistryString `
        -SubKey $entry.CommandKey `
        -Name "" `
        -Value (New-NucleotideCommand -ArgumentToken $entry.Argument)
}

Set-RegistryString -SubKey $ApplicationKey -Name "FriendlyAppName" -Value "Nucleotide"
Set-RegistryString -SubKey $ApplicationKey -Name "ApplicationName" -Value "Nucleotide"
Set-RegistryString -SubKey $ApplicationKey -Name "ApplicationDescription" -Value "A native GUI for the Helix editor"
Set-RegistryString -SubKey $ApplicationKey -Name "AppUserModelID" -Value $AppUserModelId
Remove-RegistryValue -SubKey $ApplicationKey -Name "DefaultIcon"
Set-RegistryString -SubKey $ApplicationIconKey -Name "" -Value $IconPath
Set-RegistryString -SubKey $ApplicationCommandKey -Name "" -Value (New-NucleotideCommand -ArgumentToken '"%1"')
Set-RegistryString -SubKey $AppPathsKey -Name "" -Value $ExePath
Set-RegistryString -SubKey $AppPathsKey -Name "Path" -Value $InstallDir

if ($RegisterCommonFileTypes) {
    Set-RegistryString -SubKey $SourceFileProgIdKey -Name "" -Value "Nucleotide Source File"
    Set-RegistryString -SubKey $SourceFileProgIdKey -Name "AppUserModelID" -Value $AppUserModelId
    Set-RegistryString -SubKey "$SourceFileProgIdKey\DefaultIcon" -Name "" -Value $IconPath
    Set-RegistryString -SubKey "$SourceFileProgIdKey\shell\open" -Name "Icon" -Value $IconPath
    Set-RegistryString `
        -SubKey "$SourceFileProgIdKey\shell\open\command" `
        -Name "" `
        -Value (New-NucleotideCommand -ArgumentToken '"%1"')

    foreach ($extension in $CommonFileTypeExtensions) {
        Set-RegistryNone -SubKey "Software\Classes\$extension\OpenWithProgids" -Name $SourceFileProgId
    }
}

Write-Host "Installed Nucleotide Explorer integration for the current user."
Write-Host "  Executable: $LauncherPath"
Write-Host "  Icon:     $IconPath"
Write-Host "  AppID:    $AppUserModelId"
Write-Host "  App Paths: nucl.exe"

if ($AddToPath) {
    $pathStatus = Add-InstallDirToUserPath
    if ($pathStatus -eq "added") {
        Write-Host "  PATH:     added $InstallDir"
        Write-Host "            Open a new terminal to use nucl.exe or nucl.cmd from PATH."
    } else {
        Write-Host "  PATH:     already contains $InstallDir"
    }
}

if ($RegisterCommonFileTypes) {
    Write-Host "  File types: $($CommonFileTypeExtensions.Count) common source/config extensions"
}

if ($StartMenuShortcut) {
    $shortcutPath = New-NucleotideStartMenuShortcut
    Write-Host "  Shortcut: $shortcutPath"
}
