# End-to-end smoke test for the PowerShell client, mirroring clients/shell/smoke.sh.
#
#   pwsh ./smoke.ps1
#
# Override the broker endpoint via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT. Start a
# broker first, e.g.:
#   cargo run --release --no-default-features --bin dd-rust-network-mutex

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'LiveMutexClient.ps1')

$lmxHost = if ($env:LIVE_MUTEX_HOST) { $env:LIVE_MUTEX_HOST } else { '127.0.0.1' }
$port = if ($env:LIVE_MUTEX_PORT) { [int]$env:LIVE_MUTEX_PORT } else { 6970 }

$c = [LiveMutexClient]::Connect($lmxHost, $port, $env:LIVE_MUTEX_TOKEN)
Write-Host "[smoke-powershell] connected ${lmxHost}:${port}"
try {
    $ex = $c.Acquire('smoke-powershell-exclusive', 5000)
    Write-Host "[smoke-powershell] exclusive grant: lockUuid=$($ex.LockUuid) fencing=$($ex.FencingToken)"
    $c.Release('smoke-powershell-exclusive', $ex.LockUuid)
    Write-Host '[smoke-powershell] released exclusive'

    $held = $c.Acquire('smoke-powershell-try', 5000)
    if ($null -ne $c.TryAcquire('smoke-powershell-try', 5000)) {
        throw 'try_acquire granted a held key'
    }
    Write-Host '[smoke-powershell] try-lock correctly refused a held key'
    $c.Release('smoke-powershell-try', $held.LockUuid)

    $comp = $c.AcquireMany(@('smoke-powershell-a', 'smoke-powershell-b', 'smoke-powershell-c'), 5000)
    Write-Host "[smoke-powershell] composite grant: lockUuid=$($comp.LockUuid) tokens=$($comp.FencingTokens | ConvertTo-Json -Compress)"
    $c.ReleaseMany(@('smoke-powershell-a', 'smoke-powershell-b', 'smoke-powershell-c'), $comp.LockUuid)
    Write-Host '[smoke-powershell] released composite'

    $w = $c.AcquireWrite('smoke-powershell-rw')
    Write-Host "[smoke-powershell] writer grant: fencing=$($w.FencingToken)"
    $c.ReleaseWrite('smoke-powershell-rw')
    $r = $c.AcquireRead('smoke-powershell-rw')
    Write-Host "[smoke-powershell] reader grant: fencing=$($r.FencingToken)"
    $c.ReleaseRead('smoke-powershell-rw')

    Write-Host "[smoke-powershell] ls keys=$($c.Ls() -join ',')"
    Write-Host '[smoke-powershell] OK'
}
finally {
    $c.Disconnect()
}
