param(
  [string]$NuclExe = "target\release\nucl.exe",
  [string]$RuntimeDir = "crates\nucleotide\runtime",
  [string]$RemoteHelperDir = "target\remote-helpers",
  [string]$GhosttyDll,
  [string]$PackDir = "target\release\velopack-app",
  [string]$OutputDir = "target\release\bundle\velopack",
  [string]$Version,
  [string]$PackId = "org.spiralpoint.nucleotide.windows",
  [string]$Channel = "windows",
  [string]$Runtime = "win-x64",
  [switch]$RequireRemoteHelpers
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot

function Resolve-RepoPath([string]$Path) {
  if ([System.IO.Path]::IsPathRooted($Path)) {
    return $Path
  }

  return Join-Path $RepoRoot $Path
}

function Get-WorkspaceVersion {
  $inWorkspacePackage = $false
  foreach ($line in Get-Content -LiteralPath (Resolve-RepoPath "Cargo.toml")) {
    if ($line -match '^\s*\[workspace\.package\]\s*$') {
      $inWorkspacePackage = $true
      continue
    }

    if ($inWorkspacePackage -and $line -match '^\s*\[') {
      break
    }

    if ($inWorkspacePackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
      return $Matches[1]
    }
  }

  throw "Could not find workspace package version in Cargo.toml."
}

function Resolve-GhosttyDll([string]$NuclExePath, [string]$ExplicitPath) {
  if ($ExplicitPath) {
    $resolved = Resolve-RepoPath $ExplicitPath
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
      throw "Explicit Ghostty DLL not found: $resolved"
    }
    return $resolved
  }

  $releaseDir = Split-Path -Parent $NuclExePath
  $dll = Get-ChildItem -Path $releaseDir -Recurse -Filter ghostty-vt.dll -File |
    Select-Object -First 1
  if (-not $dll) {
    throw "ghostty-vt.dll was not found under $releaseDir"
  }
  return $dll.FullName
}

function Copy-RequiredFile([string]$Source, [string]$Destination) {
  if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "Required file not found: $Source"
  }

  Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

if (-not (Get-Command vpk -ErrorAction SilentlyContinue)) {
  throw "vpk was not found. Install it with: dotnet tool update -g vpk"
}

if (-not $Version) {
  $Version = Get-WorkspaceVersion
}
$Version = $Version.TrimStart("v")

$nuclExePath = Resolve-RepoPath $NuclExe
$runtimePath = Resolve-RepoPath $RuntimeDir
$remoteHelperPath = Resolve-RepoPath $RemoteHelperDir
$packPath = Resolve-RepoPath $PackDir
$outputPath = Resolve-RepoPath $OutputDir
$iconPath = Resolve-RepoPath "crates\nucleotide\assets\nucleotide.ico"
$ghosttyDllPath = Resolve-GhosttyDll -NuclExePath $nuclExePath -ExplicitPath $GhosttyDll

if (-not (Test-Path -LiteralPath $runtimePath -PathType Container)) {
  throw "Runtime directory not found: $runtimePath"
}
if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
  throw "Icon not found: $iconPath"
}

Remove-Item -LiteralPath $packPath -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $packPath -Force | Out-Null
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null

Copy-RequiredFile -Source $nuclExePath -Destination (Join-Path $packPath "nucl.exe")
Copy-RequiredFile -Source $ghosttyDllPath -Destination (Join-Path $packPath "ghostty-vt.dll")
Copy-Item -LiteralPath $runtimePath -Destination (Join-Path $packPath "runtime") -Recurse -Force

foreach ($helper in @("nucleotide-remote-linux-x86_64", "nucleotide-remote-linux-aarch64")) {
  $source = Join-Path $remoteHelperPath $helper
  if (Test-Path -LiteralPath $source -PathType Leaf) {
    Copy-Item -LiteralPath $source -Destination (Join-Path $packPath $helper) -Force
  } elseif ($RequireRemoteHelpers) {
    throw "Required remote helper not found: $source"
  }
}

$vpkArgs = @(
  "pack",
  "--packId", $PackId,
  "--packTitle", "Nucleotide",
  "--packVersion", $Version,
  "--packDir", $packPath,
  "--mainExe", "nucl.exe",
  "--outputDir", $outputPath,
  "--channel", $Channel,
  "--icon", $iconPath
)

if ($Runtime) {
  $vpkArgs += @("--runtime", $Runtime)
}

& vpk @vpkArgs
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

Write-Host "Velopack package files written to $outputPath"
