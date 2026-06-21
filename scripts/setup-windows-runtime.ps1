param(
    [Alias("h", "?")]
    [switch]$Help,
    [switch]$SkipFetch,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host @"
Usage: .\scripts\setup-windows-runtime.cmd [options]

Options:
  -SkipFetch   Do not fetch tree-sitter grammar sources.
  -SkipBuild   Do not build tree-sitter grammar DLLs.
  -Help        Show this help text.
"@
    exit 0
}

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$RuntimeDest = Join-Path $RepoRoot "runtime"
$GrammarDest = Join-Path $RuntimeDest "grammars"

function Find-HelixRuntime {
    $localRuntime = Join-Path $RepoRoot "runtime"
    if ((Test-Path $localRuntime) -and (Test-Path (Join-Path $localRuntime "queries"))) {
        return (Resolve-Path $localRuntime).Path
    }

    $cargoCheckouts = Join-Path $env:USERPROFILE ".cargo\git\checkouts"
    if (Test-Path $cargoCheckouts) {
        $runtime = Get-ChildItem $cargoCheckouts -Recurse -Directory -Filter "runtime" |
            Where-Object { $_.FullName -match "\\helix-[^\\]+\\[^\\]+\\runtime$" } |
            Select-Object -First 1

        if ($runtime) {
            return $runtime.FullName
        }
    }

    return $null
}

function Copy-Runtime {
    param([string]$Source)

    if (-not (Test-Path $RuntimeDest)) {
        New-Item -ItemType Directory -Path $RuntimeDest | Out-Null
    }

    Copy-Item (Join-Path $Source "*") $RuntimeDest -Recurse -Force

    $sourceLanguages = Join-Path $Source "languages.toml"
    $siblingLanguages = Join-Path (Split-Path $Source -Parent) "languages.toml"
    $destLanguages = Join-Path $RuntimeDest "languages.toml"

    if (Test-Path $sourceLanguages) {
        Copy-Item $sourceLanguages $destLanguages -Force
    } elseif (Test-Path $siblingLanguages) {
        Copy-Item $siblingLanguages $destLanguages -Force
    } elseif (-not (Test-Path $destLanguages)) {
        throw "Could not find languages.toml next to Helix runtime source: $Source"
    }
}

function Invoke-NuclGrammarCommand {
    param([string]$Command)

    $env:HELIX_RUNTIME = $RuntimeDest
    $env:CARGO_MANIFEST_DIR = Join-Path $RepoRoot "nucleotide"

    cargo run -p nucleotide -- --grammar $Command
}

$runtimeSource = Find-HelixRuntime
if (-not $runtimeSource) {
    throw @"
Helix runtime directory not found.

Run `cargo build -p nucleotide` once to populate Cargo's Helix checkout, or clone Helix
and copy its runtime directory to .\runtime.
"@
}

Write-Host "Using Helix runtime source: $runtimeSource"
Copy-Runtime -Source $runtimeSource

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

$dllCount = @(Get-ChildItem $GrammarDest -Filter "*.dll" -ErrorAction SilentlyContinue).Count
$queryCount = @(Get-ChildItem (Join-Path $RuntimeDest "queries") -Directory -ErrorAction SilentlyContinue).Count
$themeCount = @(Get-ChildItem (Join-Path $RuntimeDest "themes") -Filter "*.toml" -ErrorAction SilentlyContinue).Count

Write-Host "Runtime ready at: $RuntimeDest"
Write-Host "  Grammar DLLs: $dllCount"
Write-Host "  Query dirs:   $queryCount"
Write-Host "  Themes:       $themeCount"

if ($dllCount -eq 0 -and -not $SkipBuild) {
    throw "No grammar DLLs were built. Check compiler output above."
}
