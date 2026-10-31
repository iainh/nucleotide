# Runs a native Windows client against an existing WSL helper. Never installs one.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Distro,
    [Parameter(Mandatory = $true)][string]$HelperPath,
    [ValidateRange(1, 10080)][int]$Minutes = 30,
    [ValidateRange(1, 2147483647)][int]$MaxIterations = 100000,
    [ValidateRange(1, 2147483647)][int]$IterationsPerConnection = 200,
    [UInt64]$Seed = 104729,
    [string]$OutputDirectory = '.amp\in\artifacts\wsl-soak',
    [switch]$InjectFailure
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$sampler = $null
$runDirectory = $null
Push-Location $root
try {
    if (-not $HelperPath.StartsWith('/')) { throw 'HelperPath must be an existing absolute Linux path.' }
    if ($OutputDirectory.StartsWith('.amp\in')) {
        & git check-ignore -q -- '.amp/in/artifacts/wsl-soak/probe'
        if ($LASTEXITCODE -ne 0) {
            throw 'Exclude /.amp/in/ in the repository-local Git info/exclude before running this script.'
        }
    }
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $prefix = Join-Path (Resolve-Path $OutputDirectory) ((Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssfffZ'))
    & cargo build --locked -p nucleotide-remote --example wsl_soak
    if ($LASTEXITCODE -ne 0) { throw "Soak build failed: $LASTEXITCODE" }
    $target = & cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    # Run a copy: Windows locks loaded executables, and cargo test also links
    # examples. The soak must not block concurrent workspace verification.
    $runDirectory = Join-Path ([IO.Path]::GetTempPath()) ('nucleotide-wsl-soak-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $runDirectory | Out-Null
    $exe = Join-Path $runDirectory 'wsl_soak.exe'
    Copy-Item (Join-Path $target.target_directory 'debug\examples\wsl_soak.exe') $exe
    $helperHash = & wsl.exe --distribution $Distro --exec sha256sum -- $HelperPath
    if ($LASTEXITCODE -ne 0) { throw 'Cannot hash the existing helper; nothing was installed.' }
    [ordered]@{
        timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')
        git_head = (& git rev-parse HEAD)
        git_status = @(& git status --short)
        client_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash
        helper_sha256 = "$helperHash"
        distro = $Distro
        helper = $HelperPath
        windows = [Environment]::OSVersion.VersionString
        seconds = $Minutes * 60
        max_iterations = $MaxIterations
        iterations_per_connection = $IterationsPerConnection
        seed = $Seed
        inject_failure = [bool]$InjectFailure
        coverage = 'Public backend and remote sessions, not the GPUI/Helix desktop workflow'
    } | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 "$prefix.metadata.json"
    $arguments = @('--run', $Distro, $HelperPath, ($Minutes * 60), $MaxIterations, $Seed, $IterationsPerConnection, "$prefix.jsonl")
    if ($InjectFailure) { $arguments += '--inject-failure' }
    $sampler = Start-Job -ArgumentList $prefix, (($Minutes * 60) + 240) -ScriptBlock {
        param($prefix, $seconds)
        $deadline = (Get-Date).AddSeconds($seconds)
        while ((Get-Date) -lt $deadline) {
            if (Test-Path "$prefix.jsonl") {
                $start = Get-Content "$prefix.jsonl" -TotalCount 1 | ConvertFrom-Json
                if ($start.detail.client_pid) {
                    $process = Get-Process -Id $start.detail.client_pid -ErrorAction SilentlyContinue
                    if (-not $process) { break }
                    [ordered]@{
                        timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')
                        pid = $process.Id
                        working_set = $process.WorkingSet64
                        private_bytes = $process.PrivateMemorySize64
                        handles = $process.HandleCount
                        threads = $process.Threads.Count
                        cpu_seconds = $process.TotalProcessorTime.TotalSeconds
                    } | ConvertTo-Json -Compress | Add-Content -Encoding UTF8 "$prefix.windows-resources.jsonl"
                    Start-Sleep -Seconds 30
                    continue
                }
            }
            Start-Sleep -Milliseconds 200
        }
    }
    # The example writes and flushes UTF-8 JSONL itself. Keep stderr separately
    # readable in the transcript, including connection and protocol diagnostics.
    # Windows PowerShell represents native stderr as ErrorRecord objects even
    # when exit is zero; use the process exit code, not that representation.
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & $exe @arguments 2>&1 | Tee-Object -FilePath "$prefix.console.log"
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedPreference
    }
    Write-Host "Results: $prefix.jsonl (exit $code)"
    if ($code -ne 0) { exit $code }
} finally {
    if ($sampler) {
        Stop-Job -Job $sampler
        Receive-Job -Job $sampler
        Remove-Job -Job $sampler
    }
    if ($runDirectory) { Remove-Item -Recurse -Force $runDirectory }
    Pop-Location
}
