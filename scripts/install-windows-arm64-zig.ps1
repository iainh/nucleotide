param(
    [Parameter(Mandatory = $true)]
    [string]$Destination
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Native ARM64 Zig 0.16 crashes: https://codeberg.org/ziglang/zig/issues/31865
# Retain x64 emulation for 0.17, with an explicit ARM64 build target.
$Destination = (New-Item -ItemType Directory -Path $Destination -Force).FullName
$archive = Join-Path $Destination "zig-x86_64-windows-0.17.0.zip"
Invoke-WebRequest https://ziglang.org/download/0.17.0/zig-x86_64-windows-0.17.0.zip -OutFile $archive
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428") {
    throw "Zig archive checksum mismatch."
}
Expand-Archive -LiteralPath $archive -DestinationPath $Destination -Force
Remove-Item -LiteralPath $archive
$zigDir = Join-Path $Destination "zig-x86_64-windows-0.17.0"

# Zig 0.17's DLL notification lookup still casts a byte-aligned pointer to an ARM64
# function pointer (alignment 4). Assert the Windows function address alignment
# rather than disabling debug stack traces or optimizing away the failing code.
$selfInfo = Join-Path $zigDir "lib\std\debug\SelfInfo\Windows.zig"
$source = [System.IO.File]::ReadAllText($selfInfo)
$original = "@ptrCast(try si.loadNtdllProc(@tagName(proc)))"
$replacement = "@ptrCast(@alignCast(try si.loadNtdllProc(@tagName(proc))))"
if (-not $source.Contains($original)) {
    throw "Could not find the Zig 0.17 Windows alignment workaround location."
}
[System.IO.File]::WriteAllText($selfInfo, $source.Replace($original, $replacement))

$env:PATH = "$zigDir;$env:PATH"
if ($env:GITHUB_PATH) {
    $zigDir | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
}
Write-Host "Windows ARM64 Zig build tools ready at: $zigDir"
