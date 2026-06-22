param(
    [Alias("h", "?")]
    [switch]$Help,
    [string]$OutputDir,
    [ValidateSet("debug", "release")]
    [string]$Profile = "debug",
    [switch]$Release,
    [switch]$SkipBuild,
    [switch]$SkipFetchGrammars,
    [switch]$SkipBuildGrammars,
    [switch]$IncludePdb,
    [string[]]$ExcludeGrammars = @("gotmpl"),
    [switch]$Clean,
    [string]$RuntimeSource
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($Help) {
    Write-Host @"
Usage: .\scripts\package-windows-test.cmd [options]

Creates a local Windows test package containing:
  nucl.exe
  nucl.cmd (terminal helper)
  nucleotide.ico
  install-windows-context-menu.cmd
  runtime\
  nucleotide\

Options:
  -OutputDir <path>       Package output directory. Defaults to dist\nucleotide-windows-test.
  -Profile <debug|release>
                          Cargo profile to package. Defaults to debug.
  -Release                Shortcut for -Profile release.
  -SkipBuild              Reuse the existing target\<profile>\nucl.exe.
  -SkipFetchGrammars      Do not fetch tree-sitter grammar sources.
  -SkipBuildGrammars      Do not build grammar DLLs into runtime\grammars.
  -IncludePdb             Copy the matching .pdb file when it exists.
  -ExcludeGrammars <ids>  Grammar IDs to exclude from fetch/build. Comma-separated is OK. Defaults to gotmpl.
  -Clean                  Delete the output directory before packaging.
  -RuntimeSource <path>   Runtime source directory. Defaults to .\runtime, then Cargo's Helix checkout.
  -Help                   Show this help text.
"@
    exit 0
}

if ($Release) {
    $Profile = "release"
}

function Resolve-PackagePath {
    param([string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path)) {
        $Path = Join-Path $RepoRoot "dist\nucleotide-windows-test"
    } elseif (-not [System.IO.Path]::IsPathRooted($Path)) {
        $Path = Join-Path $RepoRoot $Path
    }

    return [System.IO.Path]::GetFullPath($Path)
}

function Assert-SafeCleanPath {
    param([string]$Path)

    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)

    if ($full -eq $root) {
        throw "Refusing to clean filesystem root: $full"
    }

    if ($full -eq $RepoRoot) {
        throw "Refusing to clean repository root: $full"
    }
}

function Invoke-Checked {
    param(
        [string]$Command,
        [string[]]$Arguments
    )

    Write-Host "> $Command $($Arguments -join ' ')"
    & $Command @Arguments

    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Command $($Arguments -join ' ')"
    }
}

function Add-GitToPathIfNeeded {
    if (Get-Command git -ErrorAction SilentlyContinue) {
        return
    }

    $candidates = @(
        "C:\Program Files\Git\cmd\git.exe",
        "C:\Program Files\Git\bin\git.exe",
        "C:\Program Files (x86)\Git\cmd\git.exe",
        "C:\Program Files (x86)\Git\bin\git.exe"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $gitDir = Split-Path $candidate -Parent
            $env:PATH = "$gitDir;$env:PATH"
            Write-Host "Added Git to PATH for grammar commands: $gitDir"
            return
        }
    }
}

function Find-HelixRuntime {
    if (-not [string]::IsNullOrWhiteSpace($RuntimeSource)) {
        if (-not (Test-Path $RuntimeSource)) {
            throw "RuntimeSource does not exist: $RuntimeSource"
        }
        return (Resolve-Path $RuntimeSource).Path
    }

    $localRuntime = Join-Path $RepoRoot "runtime"
    if ((Test-Path $localRuntime) -and (Test-Path (Join-Path $localRuntime "queries"))) {
        return (Resolve-Path $localRuntime).Path
    }

    $cargoCheckouts = Join-Path $env:USERPROFILE ".cargo\git\checkouts"
    if (Test-Path $cargoCheckouts) {
        $runtime = Get-ChildItem $cargoCheckouts -Recurse -Directory -Filter "runtime" |
            Where-Object { $_.FullName -match "\\helix-[^\\]+\\[^\\]+\\runtime$" } |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1

        if ($runtime) {
            return $runtime.FullName
        }
    }

    throw @"
Helix runtime directory not found.

Run `cargo build -p nucleotide` once to populate Cargo's Helix checkout, or pass
-RuntimeSource <path-to-helix-runtime>.
"@
}

function Copy-DirectoryContents {
    param(
        [string]$Source,
        [string]$Destination
    )

    if (-not (Test-Path $Destination)) {
        New-Item -ItemType Directory -Path $Destination | Out-Null
    }

    Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
    }
}

function Copy-Runtime {
    param(
        [string]$Source,
        [string]$Destination
    )

    if (Test-Path $Destination) {
        Remove-Item -LiteralPath $Destination -Recurse -Force
    }

    New-Item -ItemType Directory -Path $Destination | Out-Null
    Copy-DirectoryContents -Source $Source -Destination $Destination

    $sourceLanguages = Join-Path $Source "languages.toml"
    $siblingLanguages = Join-Path (Split-Path $Source -Parent) "languages.toml"
    $destLanguages = Join-Path $Destination "languages.toml"

    if (Test-Path $sourceLanguages) {
        Copy-Item -LiteralPath $sourceLanguages -Destination $destLanguages -Force
    } elseif (Test-Path $siblingLanguages) {
        Copy-Item -LiteralPath $siblingLanguages -Destination $destLanguages -Force
    }

    if (-not (Test-Path $destLanguages)) {
        throw "Could not find languages.toml for runtime source: $Source"
    }
}

function Copy-NucleotideThemes {
    param([string]$RuntimeDirectory)

    $themeSource = Join-Path $RepoRoot "crates\nucleotide\assets\themes"
    if (-not (Test-Path $themeSource)) {
        return
    }

    $themeDest = Join-Path $RuntimeDirectory "themes"
    if (-not (Test-Path $themeDest)) {
        New-Item -ItemType Directory -Path $themeDest | Out-Null
    }

    Get-ChildItem -LiteralPath $themeSource -Filter "*.toml" -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $themeDest -Force
    }
}

function Format-TomlStringList {
    param([string[]]$Values)

    $quoted = foreach ($value in $Values) {
        '"' + $value.Replace('\', '\\').Replace('"', '\"') + '"'
    }

    Write-Output -NoEnumerate ($quoted -join ', ')
}

function Update-GrammarExclusions {
    param(
        [string]$RuntimeDirectory,
        [string[]]$GrammarIds
    )

    $ids = @($GrammarIds | ForEach-Object { $_ -split ',' } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim() } | Sort-Object -Unique)
    if ($ids.Count -eq 0) {
        return
    }

    $languagesFile = Join-Path $RuntimeDirectory "languages.toml"
    if (-not (Test-Path $languagesFile)) {
        throw "Cannot exclude grammars because languages.toml is missing: $languagesFile"
    }

    $content = [System.IO.File]::ReadAllText($languagesFile)
    $pattern = '(?m)^use-grammars\s*=\s*\{\s*except\s*=\s*\[(?<items>[^\]]*)\]\s*\}'
    $match = [regex]::Match($content, $pattern)

    if ($match.Success) {
        $existing = @([regex]::Matches($match.Groups["items"].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
        $merged = @($existing + $ids | Sort-Object -Unique)
        $grammarList = [string](Format-TomlStringList -Values $merged)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = [regex]::Replace($content, $pattern, $replacement, 1)
    } else {
        $grammarList = [string](Format-TomlStringList -Values $ids)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = $replacement + [Environment]::NewLine + [Environment]::NewLine + $content
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($languagesFile, $content, $utf8NoBom)
    Write-Host "Excluded grammar IDs from package runtime: $($ids -join ', ')"
}

function Write-WorkspaceGrammarExclusions {
    param(
        [string]$PackageDirectory,
        [string[]]$GrammarIds
    )

    $ids = @($GrammarIds | ForEach-Object { $_ -split ',' } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim() } | Sort-Object -Unique)
    if ($ids.Count -eq 0) {
        return
    }

    $helixDir = Join-Path $PackageDirectory ".helix"
    if (-not (Test-Path $helixDir)) {
        New-Item -ItemType Directory -Path $helixDir | Out-Null
    }

    $languagesFile = Join-Path $helixDir "languages.toml"
    $grammarList = [string](Format-TomlStringList -Values $ids)
    $grammarLine = "use-grammars = { except = [ $grammarList ] }"
    $content = @(
        "# Package-local grammar overrides for test builds.",
        $grammarLine,
        ""
    ) -join [Environment]::NewLine

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($languagesFile, $content, $utf8NoBom)
    Write-Host "Wrote package-local grammar exclusions: $languagesFile"
}

function Write-Launcher {
    param(
        [string]$PackageDirectory,
        [string]$ManifestDirectory
    )

    $launcher = Join-Path $PackageDirectory "nucl.cmd"
    $content = @"
@echo off
setlocal
set "NUCL_DIR=%~dp0"
set "CARGO_MANIFEST_DIR=%NUCL_DIR%nucleotide"
set "HELIX_RUNTIME=%NUCL_DIR%runtime"
"%NUCL_DIR%nucl.exe" %*
exit /b %ERRORLEVEL%
"@

    Set-Content -LiteralPath $launcher -Value $content -Encoding ASCII

    if (-not (Test-Path $ManifestDirectory)) {
        New-Item -ItemType Directory -Path $ManifestDirectory | Out-Null
    }
}

function Copy-WindowsIntegrationScripts {
    param([string]$PackageDirectory)

    foreach ($name in @("install-windows-context-menu.ps1", "install-windows-context-menu.cmd")) {
        $source = Join-Path $PSScriptRoot $name
        if (-not (Test-Path -LiteralPath $source)) {
            throw "Missing Windows integration script: $source"
        }

        Copy-Item -LiteralPath $source -Destination (Join-Path $PackageDirectory $name) -Force
    }

    $iconSource = Join-Path $RepoRoot "crates\nucleotide\assets\nucleotide.ico"
    if (-not (Test-Path -LiteralPath $iconSource)) {
        throw "Missing Windows package icon: $iconSource"
    }

    Copy-Item -LiteralPath $iconSource -Destination (Join-Path $PackageDirectory "nucleotide.ico") -Force
}

function Set-PackagedExecutableIcon {
    param(
        [string]$PackageExe,
        [string]$PackageDirectory
    )

    $iconPatchScript = Join-Path $PSScriptRoot "set-windows-exe-icon.ps1"
    if (-not (Test-Path -LiteralPath $iconPatchScript)) {
        throw "Missing Windows executable icon patch script: $iconPatchScript"
    }

    $iconPath = Join-Path $PackageDirectory "nucleotide.ico"
    if (-not (Test-Path -LiteralPath $iconPath)) {
        throw "Missing package icon: $iconPath"
    }

    & $iconPatchScript -ExePath $PackageExe -IconPath $iconPath
}

function Get-WindowsExecutableSubsystem {
    param([string]$ExePath)

    $bytes = [System.IO.File]::ReadAllBytes($ExePath)
    if ($bytes.Length -lt 0x100) {
        throw "Executable is too small to contain a PE header: $ExePath"
    }

    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($peOffset -lt 0 -or ($peOffset + 96) -ge $bytes.Length) {
        throw "Invalid PE header offset in executable: $ExePath"
    }

    $signature = [System.Text.Encoding]::ASCII.GetString($bytes, $peOffset, 4)
    if ($signature -ne "PE`0`0") {
        throw "Executable does not contain a PE signature: $ExePath"
    }

    $optionalHeaderOffset = $peOffset + 24
    $subsystemOffset = $optionalHeaderOffset + 68
    [BitConverter]::ToUInt16($bytes, $subsystemOffset)
}

function Assert-ReleaseExecutableUsesGuiSubsystem {
    param([string]$ExePath)

    if ($Profile -ne "release") {
        return
    }

    $subsystem = Get-WindowsExecutableSubsystem -ExePath $ExePath
    if ($subsystem -ne 2) {
        throw "Release nucl.exe must use the Windows GUI subsystem (2), but found subsystem $subsystem. A console subsystem build opens an extra terminal window."
    }

    Write-Host "Verified release executable subsystem: Windows GUI"
}

function Invoke-GrammarCommand {
    param(
        [string]$PackageExe,
        [string]$Command,
        [string]$RuntimeDirectory,
        [string]$ManifestDirectory
    )

    $oldHelixRuntime = $env:HELIX_RUNTIME
    $oldManifestDir = $env:CARGO_MANIFEST_DIR

    try {
        $env:HELIX_RUNTIME = $RuntimeDirectory
        $env:CARGO_MANIFEST_DIR = $ManifestDirectory

        if ($Profile -eq "release") {
            $DebugExe = Join-Path $RepoRoot "target\debug\nucl.exe"
            Push-Location $RepoRoot
            try {
                Invoke-Checked -Command "cargo" -Arguments @("build", "-p", "nucleotide")
            } finally {
                Pop-Location
            }

            if (-not (Test-Path $DebugExe)) {
                throw "Debug helper binary not found: $DebugExe"
            }

            Invoke-Checked -Command $DebugExe -Arguments @("--grammar", $Command)
        } else {
            Push-Location (Split-Path $PackageExe -Parent)
            try {
                Invoke-Checked -Command $PackageExe -Arguments @("--grammar", $Command)
            } finally {
                Pop-Location
            }
        }
    } finally {
        $env:HELIX_RUNTIME = $oldHelixRuntime
        $env:CARGO_MANIFEST_DIR = $oldManifestDir
    }
}

$PackageDir = Resolve-PackagePath $OutputDir
$RuntimeDest = Join-Path $PackageDir "runtime"
$ManifestDir = Join-Path $PackageDir "nucleotide"
$PackageExe = Join-Path $PackageDir "nucl.exe"
$TargetDir = Join-Path $RepoRoot "target\$Profile"
$BuiltExe = Join-Path $TargetDir "nucl.exe"

Assert-SafeCleanPath -Path $PackageDir

if ($Clean -and (Test-Path $PackageDir)) {
    Remove-Item -LiteralPath $PackageDir -Recurse -Force
}

if (-not $SkipBuild) {
    $cargoArgs = @("build", "-p", "nucleotide")
    if ($Profile -eq "release") {
        $cargoArgs += "--release"
    }
    Invoke-Checked -Command "cargo" -Arguments $cargoArgs
}

if (-not (Test-Path $BuiltExe)) {
    throw "Compiled binary not found: $BuiltExe"
}

$runtimeSourcePath = Find-HelixRuntime

Write-Host "Package directory: $PackageDir"
Write-Host "Runtime source:    $runtimeSourcePath"
Write-Host "Binary source:     $BuiltExe"

if (-not (Test-Path $PackageDir)) {
    New-Item -ItemType Directory -Path $PackageDir | Out-Null
}

Copy-Item -LiteralPath $BuiltExe -Destination $PackageExe -Force

if ($IncludePdb) {
    $builtPdb = [System.IO.Path]::ChangeExtension($BuiltExe, ".pdb")
    if (Test-Path $builtPdb) {
        Copy-Item -LiteralPath $builtPdb -Destination ([System.IO.Path]::ChangeExtension($PackageExe, ".pdb")) -Force
    }
}

Copy-Runtime -Source $runtimeSourcePath -Destination $RuntimeDest
Copy-NucleotideThemes -RuntimeDirectory $RuntimeDest
Update-GrammarExclusions -RuntimeDirectory $RuntimeDest -GrammarIds $ExcludeGrammars
Write-WorkspaceGrammarExclusions -PackageDirectory $PackageDir -GrammarIds $ExcludeGrammars
Write-Launcher -PackageDirectory $PackageDir -ManifestDirectory $ManifestDir
Copy-WindowsIntegrationScripts -PackageDirectory $PackageDir
Set-PackagedExecutableIcon -PackageExe $PackageExe -PackageDirectory $PackageDir
Assert-ReleaseExecutableUsesGuiSubsystem -ExePath $PackageExe

if (-not $SkipFetchGrammars -or -not $SkipBuildGrammars) {
    Add-GitToPathIfNeeded
}

if (-not $SkipFetchGrammars) {
    try {
        Invoke-GrammarCommand -PackageExe $PackageExe -Command "fetch" -RuntimeDirectory $RuntimeDest -ManifestDirectory $ManifestDir
    } catch {
        $sourceCount = @(Get-ChildItem (Join-Path $RuntimeDest "grammars\sources") -Directory -ErrorAction SilentlyContinue).Count
        if ($sourceCount -gt 0) {
            Write-Warning "Some grammar sources failed to fetch; continuing with $sourceCount fetched source(s)."
            $global:LASTEXITCODE = 0
        } else {
            throw
        }
    }
}

if (-not $SkipBuildGrammars) {
    try {
        Invoke-GrammarCommand -PackageExe $PackageExe -Command "build" -RuntimeDirectory $RuntimeDest -ManifestDirectory $ManifestDir
    } catch {
        $builtDllCount = @(Get-ChildItem (Join-Path $RuntimeDest "grammars") -Filter "*.dll" -ErrorAction SilentlyContinue).Count
        if ($builtDllCount -gt 0) {
            Write-Warning "Some grammars failed to build; continuing with $builtDllCount compiled grammar DLL(s)."
            $global:LASTEXITCODE = 0
        } else {
            throw
        }
    }
}

$dllCount = @(Get-ChildItem (Join-Path $RuntimeDest "grammars") -Filter "*.dll" -ErrorAction SilentlyContinue).Count
$queryCount = @(Get-ChildItem (Join-Path $RuntimeDest "queries") -Directory -ErrorAction SilentlyContinue).Count
$themeCount = @(Get-ChildItem (Join-Path $RuntimeDest "themes") -Filter "*.toml" -ErrorAction SilentlyContinue).Count

Write-Host ""
Write-Host "Windows test package ready:"
Write-Host "  $PackageDir"
Write-Host ""
Write-Host "Contents:"
Write-Host "  Binary:       $PackageExe"
Write-Host "  Executable:   $(Join-Path $PackageDir "nucl.exe")"
Write-Host "  Cmd helper:   $(Join-Path $PackageDir "nucl.cmd")"
Write-Host "  Icon:         $(Join-Path $PackageDir "nucleotide.ico")"
Write-Host "  Explorer:     $(Join-Path $PackageDir "install-windows-context-menu.cmd")"
Write-Host "  Runtime:      $RuntimeDest"
Write-Host "  Grammar DLLs: $dllCount"
Write-Host "  Query dirs:   $queryCount"
Write-Host "  Themes:       $themeCount"

if ($dllCount -eq 0) {
    if ($SkipBuildGrammars) {
        Write-Warning "No grammar DLLs found because -SkipBuildGrammars was used."
    } else {
        throw "No grammar DLLs were built. Check compiler output above."
    }
}
