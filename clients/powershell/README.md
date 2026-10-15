# PowerShell client

The Windows-shell companion to [`../shell`](../shell). Same wire protocol
([`../../PROTOCOL.md`](../../PROTOCOL.md)), implemented with
`System.Net.Sockets.TcpClient` and `ConvertTo-Json` / `ConvertFrom-Json`. Runs
on Windows PowerShell 5.1 and on PowerShell 7+ (`pwsh`) on macOS/Linux.

The wire `type` discriminators live in the `$LmxReq` / `$LmxRes` constant tables
in `LiveMutexClient.ps1` (no inline magic strings), and
`../check-protocol-parity.sh` greps that file to confirm it mirrors every
variant in `src/protocol.rs`.

## Files

- `LiveMutexClient.ps1` — the `[LiveMutexClient]` class: Connect, Acquire /
  TryAcquire / Release, AcquireMany / ReleaseMany, AcquireRead / AcquireWrite /
  ReleaseRead / ReleaseWrite, and Ls.
- `smoke.ps1` — end-to-end smoke test mirroring `../shell/smoke.sh`.

## Smoke test

```powershell
# 1. start the broker (in another terminal)
cargo run --release --no-default-features --bin dd-rust-network-mutex

# 2. run the PowerShell smoke
pwsh ./clients/powershell/smoke.ps1
```

Override the endpoint with `LIVE_MUTEX_HOST` / `LIVE_MUTEX_PORT` (defaults
`127.0.0.1:6970`); set `LIVE_MUTEX_TOKEN` when the broker requires auth.

## Using it from your own script

```powershell
. ./clients/powershell/LiveMutexClient.ps1

$c = [LiveMutexClient]::Connect('127.0.0.1', 6970)
$h = $c.Acquire('my-key', 30000)   # blocks until granted
Write-Host "held with fencing token $($h.FencingToken) (handle $($h.LockUuid))"
# ... critical section ...
$c.Release('my-key', $h.LockUuid)
$c.Disconnect()
```

`TryAcquire` returns `$null` on contention instead of a handle.
