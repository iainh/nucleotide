param(
    [string]$Destination = "helix-temp"
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$CargoToml = Join-Path $RepoRoot "Cargo.toml"

$cargo = [System.IO.File]::ReadAllText($CargoToml)
$match = [regex]::Match($cargo, 'helix-loader\s*=\s*\{[^\r\n]*rev\s*=\s*"(?<rev>[0-9a-fA-F]+)"')
if (-not $match.Success) {
    throw "Could not find helix-loader rev in $CargoToml"
}

$HelixRev = $match.Groups["rev"].Value
$DestinationPath = if ([System.IO.Path]::IsPathRooted($Destination)) {
    [System.IO.Path]::GetFullPath($Destination)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Destination))
}

if (Test-Path -LiteralPath $DestinationPath) {
    Remove-Item -LiteralPath $DestinationPath -Recurse -Force
}

Write-Host "Fetching Helix runtime at Cargo-pinned revision: $HelixRev"
git init $DestinationPath
if ($LASTEXITCODE -ne 0) {
    throw "git init failed with exit code $LASTEXITCODE"
}

git -C $DestinationPath remote add origin https://github.com/helix-editor/helix.git
if ($LASTEXITCODE -ne 0) {
    throw "git remote add failed with exit code $LASTEXITCODE"
}

git -C $DestinationPath fetch --depth 1 origin $HelixRev
if ($LASTEXITCODE -ne 0) {
    throw "git fetch Helix revision $HelixRev failed with exit code $LASTEXITCODE"
}

git -C $DestinationPath checkout --detach FETCH_HEAD
if ($LASTEXITCODE -ne 0) {
    throw "git checkout Helix revision $HelixRev failed with exit code $LASTEXITCODE"
}

Write-Host "Helix runtime ready at: $DestinationPath\runtime"
