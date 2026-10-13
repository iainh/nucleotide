param(
    [Parameter(Mandatory = $true)]
    [string]$Destination
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Native ARM64 Zig 0.16 crashes: https://codeberg.org/ziglang/zig/issues/31865
# Run the x64 compiler under emulation, with an explicit ARM64 build target.
$Destination = (New-Item -ItemType Directory -Path $Destination -Force).FullName
$archive = Join-Path $Destination "zig-x86_64-windows-0.16.0.zip"
Invoke-WebRequest https://ziglang.org/download/0.16.0/zig-x86_64-windows-0.16.0.zip -OutFile $archive
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e") {
    throw "Zig archive checksum mismatch."
}
Expand-Archive -LiteralPath $archive -DestinationPath $Destination -Force
Remove-Item -LiteralPath $archive
$zigDir = Join-Path $Destination "zig-x86_64-windows-0.16.0"

# Zig 0.16's DLL notification lookup casts a byte-aligned pointer to an ARM64
# function pointer (alignment 4). Assert the Windows function address alignment
# rather than disabling debug stack traces or optimizing away the failing code.
$selfInfo = Join-Path $zigDir "lib\std\debug\SelfInfo\Windows.zig"
$source = [System.IO.File]::ReadAllText($selfInfo)
$original = "@ptrCast(try si.loadNtdllProc(@tagName(proc)))"
$replacement = "@ptrCast(@alignCast(try si.loadNtdllProc(@tagName(proc))))"
if (-not $source.Contains($original)) {
    throw "Could not find the Zig 0.16 Windows alignment workaround location."
}
[System.IO.File]::WriteAllText($selfInfo, $source.Replace($original, $replacement))

$env:PATH = "$zigDir;$env:PATH"
if ($env:GITHUB_PATH) {
    $zigDir | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
}
Write-Host "Windows ARM64 Zig build tools ready at: $zigDir"
