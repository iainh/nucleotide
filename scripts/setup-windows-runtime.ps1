param(
    [Alias("h", "?")]
    [switch]$Help,
    [switch]$SkipFetch,
    [switch]$SkipBuild,
    [switch]$AllowGrammarFailures,
    [string[]]$ExcludeGrammars = @("gotmpl"),
    [string]$RuntimeSource
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host @"
Usage: .\scripts\setup-windows-runtime.cmd [options]

Prepares the Helix runtime under crates\nucleotide\runtime for cargo-bundle's
Windows MSI build.

Options:
  -SkipFetch              Do not fetch tree-sitter grammar sources.
  -SkipBuild              Do not build tree-sitter grammar DLLs.
  -AllowGrammarFailures   Continue when some grammar fetch/build jobs fail.
  -ExcludeGrammars <ids>  Grammar IDs to exclude from fetch/build. Comma-separated is OK. Defaults to gotmpl.
  -RuntimeSource <path>   Runtime source directory. Defaults to Cargo's Helix checkout, then .\runtime.
  -Help                   Show this help text.
"@
    exit 0
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$BundleCrateRoot = Join-Path $RepoRoot "crates\nucleotide"
$RuntimeDest = Join-Path $BundleCrateRoot "runtime"
$GrammarDest = Join-Path $RuntimeDest "grammars"
$ManifestDir = Join-Path $BundleCrateRoot "nucleotide"
$ExcludedGrammarIds = @($ExcludeGrammars | ForEach-Object { $_ -split ',' } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim() } | Sort-Object -Unique)

function Find-HelixRuntime {
    if (-not [string]::IsNullOrWhiteSpace($RuntimeSource)) {
        if (-not (Test-Path -LiteralPath $RuntimeSource)) {
            throw "RuntimeSource does not exist: $RuntimeSource"
        }

        return (Resolve-Path $RuntimeSource).Path
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

    $localRuntime = Join-Path $RepoRoot "runtime"
    if ((Test-Path $localRuntime) -and (Test-Path (Join-Path $localRuntime "queries"))) {
        return (Resolve-Path $localRuntime).Path
    }

    throw @"
Helix runtime directory not found.

Run `cargo build -p nucleotide` once to populate Cargo's Helix checkout, clone
Helix and copy its runtime directory to .\runtime, or pass
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
    param([string]$Source)

    if (Test-Path $RuntimeDest) {
        Remove-Item -LiteralPath $RuntimeDest -Recurse -Force
    }

    New-Item -ItemType Directory -Path $RuntimeDest | Out-Null
    Copy-DirectoryContents -Source $Source -Destination $RuntimeDest

    $sourceLanguages = Join-Path $Source "languages.toml"
    $siblingLanguages = Join-Path (Split-Path $Source -Parent) "languages.toml"
    $destLanguages = Join-Path $RuntimeDest "languages.toml"

    if (Test-Path $sourceLanguages) {
        Copy-Item -LiteralPath $sourceLanguages -Destination $destLanguages -Force
    } elseif (Test-Path $siblingLanguages) {
        Copy-Item -LiteralPath $siblingLanguages -Destination $destLanguages -Force
    }

    if (-not (Test-Path $destLanguages)) {
        throw "Could not find languages.toml next to Helix runtime source: $Source"
    }
}

function Copy-NucleotideThemes {
    $themeSource = Join-Path $BundleCrateRoot "assets\themes"
    if (-not (Test-Path $themeSource)) {
        return
    }

    $themeDest = Join-Path $RuntimeDest "themes"
    if (-not (Test-Path $themeDest)) {
        New-Item -ItemType Directory -Path $themeDest | Out-Null
    }

    Get-ChildItem -LiteralPath $themeSource -Filter "*.toml" -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $themeDest -Force
    }
}

function Rename-WixUnsafeQueryDirs {
    $queryRoot = Join-Path $RuntimeDest "queries"
    if (-not (Test-Path $queryRoot)) {
        return
    }

    $renames = [ordered]@{
        "_gjs" = "underscore-gjs"
        "_javascript" = "underscore-javascript"
        "_jsx" = "underscore-jsx"
        "_typescript" = "underscore-typescript"
    }

    foreach ($entry in $renames.GetEnumerator()) {
        $source = Join-Path $queryRoot $entry.Key
        $dest = Join-Path $queryRoot $entry.Value

        if (Test-Path $source) {
            if (Test-Path $dest) {
                Remove-Item -LiteralPath $dest -Recurse -Force
            }

            Rename-Item -LiteralPath $source -NewName $entry.Value
            Write-Host "Renamed WiX-unsafe query directory: $($entry.Key) -> $($entry.Value)"
        }
    }

    $textFiles = Get-ChildItem -LiteralPath $queryRoot -Recurse -File |
        Where-Object { $_.Extension -in @(".scm", ".md") }

    foreach ($file in $textFiles) {
        $content = [System.IO.File]::ReadAllText($file.FullName)
        $updated = $content
        foreach ($entry in $renames.GetEnumerator()) {
            $updated = $updated.Replace($entry.Key, $entry.Value)
        }

        if ($updated -ne $content) {
            $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
            [System.IO.File]::WriteAllText($file.FullName, $updated, $utf8NoBom)
        }
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
    param([string[]]$GrammarIds)

    if ($GrammarIds.Count -eq 0) {
        return
    }

    $languagesFile = Join-Path $RuntimeDest "languages.toml"
    if (-not (Test-Path $languagesFile)) {
        throw "Cannot exclude grammars because languages.toml is missing: $languagesFile"
    }

    $content = [System.IO.File]::ReadAllText($languagesFile)
    $pattern = '(?m)^use-grammars\s*=\s*\{\s*except\s*=\s*\[(?<items>[^\]]*)\]\s*\}'
    $match = [regex]::Match($content, $pattern)

    if ($match.Success) {
        $existing = @([regex]::Matches($match.Groups["items"].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
        $merged = @($existing + $GrammarIds | Sort-Object -Unique)
        $grammarList = [string](Format-TomlStringList -Values $merged)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = [regex]::Replace($content, $pattern, $replacement, 1)
    } else {
        $grammarList = [string](Format-TomlStringList -Values $GrammarIds)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = $replacement + [Environment]::NewLine + [Environment]::NewLine + $content
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($languagesFile, $content, $utf8NoBom)
    Write-Host "Excluded grammar IDs from bundled runtime: $($GrammarIds -join ', ')"
}

function Write-WorkspaceGrammarExclusions {
    param([string[]]$GrammarIds)

    if ($GrammarIds.Count -eq 0) {
        return $null
    }

    $helixDir = Join-Path $RepoRoot ".helix"
    $languagesFile = Join-Path $helixDir "languages.toml"
    $hadHelixDir = Test-Path -LiteralPath $helixDir
    $hadLanguagesFile = Test-Path -LiteralPath $languagesFile
    $previousContent = $null

    if ($hadLanguagesFile) {
        $previousContent = [System.IO.File]::ReadAllText($languagesFile)
        $content = $previousContent
    } else {
        $content = ""
    }

    $pattern = '(?m)^use-grammars\s*=\s*\{\s*except\s*=\s*\[(?<items>[^\]]*)\]\s*\}'
    $match = [regex]::Match($content, $pattern)

    if ($match.Success) {
        $existing = @([regex]::Matches($match.Groups["items"].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
        $merged = @($existing + $GrammarIds | Sort-Object -Unique)
        $grammarList = [string](Format-TomlStringList -Values $merged)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = [regex]::Replace($content, $pattern, $replacement, 1)
    } else {
        $grammarList = [string](Format-TomlStringList -Values $GrammarIds)
        $replacement = "use-grammars = { except = [ $grammarList ] }"
        $content = $replacement + [Environment]::NewLine + [Environment]::NewLine + $content
    }

    if (-not $hadHelixDir) {
        New-Item -ItemType Directory -Path $helixDir | Out-Null
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($languagesFile, $content, $utf8NoBom)
    Write-Host "Excluded grammar IDs from grammar commands: $($GrammarIds -join ', ')"

    [pscustomobject]@{
        HelixDir = $helixDir
        LanguagesFile = $languagesFile
        HadHelixDir = $hadHelixDir
        HadLanguagesFile = $hadLanguagesFile
        PreviousContent = $previousContent
    }
}

function Restore-WorkspaceGrammarExclusions {
    param($State)

    if ($null -eq $State) {
        return
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)

    if ($State.HadLanguagesFile) {
        [System.IO.File]::WriteAllText($State.LanguagesFile, $State.PreviousContent, $utf8NoBom)
    } elseif (Test-Path -LiteralPath $State.LanguagesFile) {
        Remove-Item -LiteralPath $State.LanguagesFile -Force
    }

    if (-not $State.HadHelixDir -and (Test-Path -LiteralPath $State.HelixDir)) {
        $remaining = @(Get-ChildItem -LiteralPath $State.HelixDir -Force)
        if ($remaining.Count -eq 0) {
            Remove-Item -LiteralPath $State.HelixDir -Force
        }
    }
}

function Invoke-NuclGrammarCommand {
    param([string]$Command)

    $oldHelixRuntime = $env:HELIX_RUNTIME
    $oldManifestDir = $env:CARGO_MANIFEST_DIR
    $grammarConfigState = $null

    try {
        $env:HELIX_RUNTIME = $RuntimeDest
        $env:CARGO_MANIFEST_DIR = $ManifestDir
        $grammarConfigState = Write-WorkspaceGrammarExclusions -GrammarIds $ExcludedGrammarIds

        Push-Location $RepoRoot
        try {
            cargo build -p nucleotide
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build -p nucleotide failed with exit code $LASTEXITCODE"
            }

            $nuclExe = Join-Path $RepoRoot "target\debug\nucl.exe"
            if (-not (Test-Path $nuclExe)) {
                $nuclExe = Join-Path $RepoRoot "target\debug\nucl"
            }
            if (-not (Test-Path $nuclExe)) {
                throw "Could not find built nucl executable under target\debug"
            }

            & $nuclExe --grammar $Command
            if ($LASTEXITCODE -ne 0) {
                $message = "nucl --grammar $Command failed with exit code $LASTEXITCODE"
                if ($AllowGrammarFailures) {
                    Write-Warning "$message; continuing because -AllowGrammarFailures was specified."
                    $global:LASTEXITCODE = 0
                    return
                }

                throw $message
            }
        } finally {
            Pop-Location
        }
    } finally {
        Restore-WorkspaceGrammarExclusions -State $grammarConfigState
        $env:HELIX_RUNTIME = $oldHelixRuntime
        $env:CARGO_MANIFEST_DIR = $oldManifestDir
    }
}

function Remove-PackagedGrammarSources {
    $grammarSources = Join-Path $GrammarDest "sources"
    if (Test-Path $grammarSources) {
        Remove-Item -LiteralPath $grammarSources -Recurse -Force
        Write-Host "Removed grammar sources from bundled runtime: $grammarSources"
    }

    $grammarPlaceholder = Join-Path $GrammarDest ".gitkeep"
    if (Test-Path $grammarPlaceholder) {
        Remove-Item -LiteralPath $grammarPlaceholder -Force
        Write-Host "Removed grammar placeholder from bundled runtime: $grammarPlaceholder"
    }
}

$runtimeSource = Find-HelixRuntime

Write-Host "Using Helix runtime source: $runtimeSource"
Write-Host "Preparing cargo-bundle runtime: $RuntimeDest"

Copy-Runtime -Source $runtimeSource
Copy-NucleotideThemes
Rename-WixUnsafeQueryDirs
Update-GrammarExclusions -GrammarIds $ExcludedGrammarIds

if (-not (Test-Path $GrammarDest)) {
    New-Item -ItemType Directory -Path $GrammarDest | Out-Null
}

if (-not $SkipFetch) {
    Write-Host "Fetching tree-sitter grammar sources..."
    Invoke-NuclGrammarCommand -Command "fetch"
}

if (-not $SkipBuild) {
    Write-Host "Building Windows tree-sitter grammar DLLs..."
    Invoke-NuclGrammarCommand -Command "build"
}

Remove-PackagedGrammarSources

$dllCount = @(Get-ChildItem $GrammarDest -Filter "*.dll" -ErrorAction SilentlyContinue).Count
$queryCount = @(Get-ChildItem (Join-Path $RuntimeDest "queries") -Directory -ErrorAction SilentlyContinue).Count
$themeCount = @(Get-ChildItem (Join-Path $RuntimeDest "themes") -Filter "*.toml" -ErrorAction SilentlyContinue).Count

Write-Host "Cargo-bundle runtime ready at: $RuntimeDest"
Write-Host "  Grammar DLLs: $dllCount"
Write-Host "  Query dirs:   $queryCount"
Write-Host "  Themes:       $themeCount"

if ($dllCount -eq 0 -and -not $SkipBuild) {
    throw "No grammar DLLs were built. Check compiler output above."
}
