param(
    [Alias("h", "?")]
    [switch]$Help,
    [Parameter(Mandatory = $false)]
    [string]$ExePath,
    [Parameter(Mandatory = $false)]
    [string]$IconPath,
    [int]$GroupResourceId = 1,
    [int]$FirstIconResourceId = 32513,
    [int]$LanguageId = 1033
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host @"
Usage: .\set-windows-exe-icon.ps1 -ExePath <path> -IconPath <path> [options]

Replaces the primary Windows icon resource in an executable with an .ico file.

Options:
  -ExePath <path>             Executable to update.
  -IconPath <path>            .ico file to embed.
  -GroupResourceId <id>       RT_GROUP_ICON id to update. Defaults to 1.
  -FirstIconResourceId <id>   First RT_ICON image id to write. Defaults to 32513.
  -LanguageId <id>            Resource language id. Defaults to 1033 (en-US).
  -Help                       Show this help text.
"@
    exit 0
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    throw "-ExePath is required."
}

if ([string]::IsNullOrWhiteSpace($IconPath)) {
    throw "-IconPath is required."
}

$ExePath = [System.IO.Path]::GetFullPath($ExePath)
$IconPath = [System.IO.Path]::GetFullPath($IconPath)

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Executable not found: $ExePath"
}

if (-not (Test-Path -LiteralPath $IconPath)) {
    throw "Icon file not found: $IconPath"
}

if ($GroupResourceId -lt 1 -or $GroupResourceId -gt 65535) {
    throw "GroupResourceId must be between 1 and 65535."
}

if ($FirstIconResourceId -lt 1 -or $FirstIconResourceId -gt 65535) {
    throw "FirstIconResourceId must be between 1 and 65535."
}

$NativeType = "Nucleotide.WindowsResources.NativeMethods" -as [type]
if (-not $NativeType) {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

namespace Nucleotide.WindowsResources
{
    public static class NativeMethods
    {
        [DllImport("kernel32.dll", EntryPoint = "BeginUpdateResourceW", SetLastError = true, CharSet = CharSet.Unicode)]
        public static extern IntPtr BeginUpdateResource(string pFileName, bool bDeleteExistingResources);

        [DllImport("kernel32.dll", EntryPoint = "UpdateResourceW", SetLastError = true, CharSet = CharSet.Unicode)]
        public static extern bool UpdateResource(IntPtr hUpdate, IntPtr lpType, IntPtr lpName, ushort wLanguage, byte[] lpData, uint cbData);

        [DllImport("kernel32.dll", EntryPoint = "EndUpdateResourceW", SetLastError = true)]
        public static extern bool EndUpdateResource(IntPtr hUpdate, bool fDiscard);
    }
}
"@
    $NativeType = "Nucleotide.WindowsResources.NativeMethods" -as [type]
}

function Get-Win32Error {
    $code = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
    $message = (New-Object System.ComponentModel.Win32Exception($code)).Message
    "$message ($code)"
}

function Get-UInt16LE {
    param(
        [byte[]]$Bytes,
        [int]$Offset
    )

    [System.BitConverter]::ToUInt16($Bytes, $Offset)
}

function Get-UInt32LE {
    param(
        [byte[]]$Bytes,
        [int]$Offset
    )

    [System.BitConverter]::ToUInt32($Bytes, $Offset)
}

function Read-IcoFile {
    param([string]$Path)

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 6) {
        throw "Invalid .ico file: header is too short."
    }

    $reserved = Get-UInt16LE -Bytes $bytes -Offset 0
    $type = Get-UInt16LE -Bytes $bytes -Offset 2
    $count = Get-UInt16LE -Bytes $bytes -Offset 4

    if ($reserved -ne 0 -or $type -ne 1 -or $count -lt 1) {
        throw "Invalid .ico file: expected an icon directory."
    }

    $directoryLength = 6 + (16 * $count)
    if ($bytes.Length -lt $directoryLength) {
        throw "Invalid .ico file: directory is truncated."
    }

    $entries = @()
    for ($index = 0; $index -lt $count; $index++) {
        $entryOffset = 6 + (16 * $index)
        $imageSize = [int](Get-UInt32LE -Bytes $bytes -Offset ($entryOffset + 8))
        $imageOffset = [int](Get-UInt32LE -Bytes $bytes -Offset ($entryOffset + 12))

        if ($imageSize -lt 1 -or $imageOffset -lt $directoryLength -or ($imageOffset + $imageSize) -gt $bytes.Length) {
            throw "Invalid .ico file: image entry $index points outside the file."
        }

        $imageData = New-Object byte[] $imageSize
        [System.Array]::Copy($bytes, $imageOffset, $imageData, 0, $imageSize)

        $entries += [pscustomobject]@{
            WidthByte = $bytes[$entryOffset]
            HeightByte = $bytes[$entryOffset + 1]
            ColorCount = $bytes[$entryOffset + 2]
            Reserved = $bytes[$entryOffset + 3]
            Planes = Get-UInt16LE -Bytes $bytes -Offset ($entryOffset + 4)
            BitCount = Get-UInt16LE -Bytes $bytes -Offset ($entryOffset + 6)
            BytesInRes = $imageSize
            ImageData = $imageData
            ResourceId = $FirstIconResourceId + $index
        }
    }

    return $entries
}

function New-GroupIconData {
    param([object[]]$Entries)

    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter($stream)
    try {
        $writer.Write([UInt16]0)
        $writer.Write([UInt16]1)
        $writer.Write([UInt16]$Entries.Count)

        foreach ($entry in $Entries) {
            $writer.Write([byte]$entry.WidthByte)
            $writer.Write([byte]$entry.HeightByte)
            $writer.Write([byte]$entry.ColorCount)
            $writer.Write([byte]$entry.Reserved)
            $writer.Write([UInt16]$entry.Planes)
            $writer.Write([UInt16]$entry.BitCount)
            $writer.Write([UInt32]$entry.BytesInRes)
            $writer.Write([UInt16]$entry.ResourceId)
        }

        $writer.Flush()
        return $stream.ToArray()
    } finally {
        $writer.Dispose()
        $stream.Dispose()
    }
}

$entries = @(Read-IcoFile -Path $IconPath)
if (($FirstIconResourceId + $entries.Count - 1) -gt 65535) {
    throw "Icon resource ids would exceed 65535."
}

$groupIconData = New-GroupIconData -Entries $entries

$rtIcon = [IntPtr]3
$rtGroupIcon = [IntPtr]14
$hUpdate = $NativeType::BeginUpdateResource($ExePath, $false)
if ($hUpdate -eq [IntPtr]::Zero) {
    throw "BeginUpdateResource failed for ${ExePath}: $(Get-Win32Error)"
}

$commit = $false
try {
    foreach ($entry in $entries) {
        $ok = $NativeType::UpdateResource(
            $hUpdate,
            $rtIcon,
            [IntPtr]$entry.ResourceId,
            [UInt16]$LanguageId,
            $entry.ImageData,
            [UInt32]$entry.ImageData.Length
        )

        if (-not $ok) {
            throw "UpdateResource failed for RT_ICON $($entry.ResourceId): $(Get-Win32Error)"
        }
    }

    $ok = $NativeType::UpdateResource(
        $hUpdate,
        $rtGroupIcon,
        [IntPtr]$GroupResourceId,
        [UInt16]$LanguageId,
        $groupIconData,
        [UInt32]$groupIconData.Length
    )

    if (-not $ok) {
        throw "UpdateResource failed for RT_GROUP_ICON ${GroupResourceId}: $(Get-Win32Error)"
    }

    $commit = $true
} finally {
    $discard = -not $commit
    $ended = $NativeType::EndUpdateResource($hUpdate, $discard)
    if (-not $ended) {
        throw "EndUpdateResource failed for ${ExePath}: $(Get-Win32Error)"
    }
}

Write-Host "Updated Windows executable icon:"
Write-Host "  Executable: $ExePath"
Write-Host "  Icon:       $IconPath"
Write-Host "  Images:     $($entries.Count)"
