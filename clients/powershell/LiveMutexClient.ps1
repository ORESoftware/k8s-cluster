# LiveMutexClient.ps1 — PowerShell client for the dd-rust-network-mutex broker.
#
# Mirrors the Bash / Python / Go clients: it speaks the newline-delimited JSON
# wire protocol in ../../PROTOCOL.md (canonical schema: ../../src/protocol.rs).
# Every wire `type` value is a named constant in $LmxReq / $LmxRes below instead
# of an inline magic string. Works on Windows PowerShell 5.1 and PowerShell 7+
# (pwsh) on macOS/Linux.
#
# Dot-source this file and use [LiveMutexClient]::Connect(host, port); see
# smoke.ps1 for an end-to-end example.

# StrictMode 1.0 still catches uninitialized variables but lets an absent JSON
# field read back as $null (broker frames legitimately omit optional fields such
# as fencingToken, or `type` on a bare error frame) instead of throwing.
Set-StrictMode -Version 1.0

# Request `type` values (src/protocol.rs `enum Request`)
$script:LmxReq = @{
    Version       = 'version'
    Auth          = 'auth'
    Lock          = 'lock'
    Unlock        = 'unlock'
    RegisterRead  = 'registerRead'
    RegisterWrite = 'registerWrite'
    EndRead       = 'endRead'
    EndWrite      = 'endWrite'
    LockInfo      = 'lockInfo'
    Ls            = 'ls'
    Heartbeat     = 'heartbeat'
}

# Response `type` values (src/protocol.rs `enum Response`)
$script:LmxRes = @{
    Version             = 'version'
    Auth                = 'auth'
    Lock                = 'lock'
    CompositeLock       = 'compositeLock'
    Unlock              = 'unlock'
    RegisterReadResult  = 'registerReadResult'
    RegisterWriteResult = 'registerWriteResult'
    EndReadResult       = 'endReadResult'
    EndWriteResult      = 'endWriteResult'
    LockInfo            = 'lockInfo'
    LsResult            = 'lsResult'
    Reelection          = 'reelection'
    Error               = 'error'
    Ok                  = 'ok'
}

class LiveMutexClient {
    [System.Net.Sockets.TcpClient] $Tcp
    [System.IO.StreamReader] $Reader
    [System.IO.Stream] $Stream
    [int] $TimeoutMs = 30000

    static [LiveMutexClient] Connect([string] $iHost, [int] $port) {
        return [LiveMutexClient]::Connect($iHost, $port, $null)
    }

    static [LiveMutexClient] Connect([string] $iHost, [int] $port, [string] $token) {
        $c = [LiveMutexClient]::new()
        $c.Tcp = [System.Net.Sockets.TcpClient]::new()
        $c.Tcp.NoDelay = $true
        $c.Tcp.Connect($iHost, $port)
        $c.Stream = $c.Tcp.GetStream()
        $c.Stream.ReadTimeout = $c.TimeoutMs
        $c.Reader = [System.IO.StreamReader]::new($c.Stream, [System.Text.Encoding]::UTF8)
        if ($token) {
            $u = [LiveMutexClient]::NewUuid()
            $r = $c.Roundtrip(@{ type = $script:LmxReq.Auth; uuid = $u; token = $token }, $u)
            if (-not $r.ok) { $c.Disconnect(); throw "auth rejected: $($r | ConvertTo-Json -Compress)" }
        }
        return $c
    }

    static [string] NewUuid() { return [guid]::NewGuid().ToString() }

    hidden [void] Send([hashtable] $frame) {
        $json = ($frame | ConvertTo-Json -Compress -Depth 6) + "`n"
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
        $this.Stream.Write($bytes, 0, $bytes.Length)
        $this.Stream.Flush()
    }

    # Read frames until one carries our uuid; return it parsed.
    hidden [object] ReadReply([string] $want) {
        while ($true) {
            $line = $this.Reader.ReadLine()
            if ($null -eq $line) { throw 'connection closed by broker' }
            if ($line -eq '') { continue }
            $obj = $line | ConvertFrom-Json
            if ($obj.uuid -eq $want) { return $obj }
        }
        throw 'unreachable'
    }

    # Like ReadReply but skips queued (acquired:false, no error) notices.
    hidden [object] ReadGrant([string] $want) {
        while ($true) {
            $obj = $this.ReadReply($want)
            if ($null -ne $obj.error) { return $obj }
            if ($obj.acquired -eq $true) { return $obj }
            if ($obj.acquired -eq $false) { continue }
            return $obj
        }
        throw 'unreachable'
    }

    hidden [object] ReadUntilGranted([string] $want) {
        while ($true) {
            $obj = $this.ReadReply($want)
            if ($obj.granted -eq $true) { return $obj }
            if ($null -ne $obj.error) { throw "rw acquire failed: $($obj.error)" }
        }
        throw 'unreachable'
    }

    hidden [object] Roundtrip([hashtable] $frame, [string] $uuid) {
        $this.Send($frame); return $this.ReadReply($uuid)
    }

    # -- exclusive / semaphore -------------------------------------------

    [pscustomobject] Acquire([string] $key, [int] $ttlMs) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Send(@{ type = $script:LmxReq.Lock; uuid = $u; key = $key; ttl = $ttlMs; wait = $true })
        $r = $this.ReadGrant($u)
        if ($r.acquired -ne $true) { throw "acquire($key) failed: $($r | ConvertTo-Json -Compress)" }
        return [pscustomobject]@{ Key = $key; LockUuid = $r.lockUuid; FencingToken = $r.fencingToken }
    }

    # Returns $null on contention.
    [pscustomobject] TryAcquire([string] $key, [int] $ttlMs) {
        $u = [LiveMutexClient]::NewUuid()
        $r = $this.Roundtrip(@{ type = $script:LmxReq.Lock; uuid = $u; key = $key; ttl = $ttlMs; wait = $false }, $u)
        if ($r.type -eq $script:LmxRes.Error) { throw "try_acquire($key) error: $($r.error)" }
        if ($r.acquired -ne $true) { return $null }
        return [pscustomobject]@{ Key = $key; LockUuid = $r.lockUuid; FencingToken = $r.fencingToken }
    }

    [void] Release([string] $key, [string] $lockUuid) {
        $u = [LiveMutexClient]::NewUuid()
        $r = $this.Roundtrip(@{ type = $script:LmxReq.Unlock; uuid = $u; key = $key; lockUuid = $lockUuid }, $u)
        if ($r.unlocked -ne $true) { throw "release($key) failed: $($r | ConvertTo-Json -Compress)" }
    }

    # -- composite (multi-key) -------------------------------------------

    [pscustomobject] AcquireMany([string[]] $keys, [int] $ttlMs) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Send(@{ type = $script:LmxReq.Lock; uuid = $u; keys = $keys; ttl = $ttlMs; wait = $true })
        $r = $this.ReadGrant($u)
        if ($r.acquired -ne $true) { throw "acquire_many failed: $($r | ConvertTo-Json -Compress)" }
        return [pscustomobject]@{ Keys = $keys; LockUuid = $r.lockUuid; FencingTokens = $r.fencingTokens }
    }

    [void] ReleaseMany([string[]] $keys, [string] $lockUuid) {
        $u = [LiveMutexClient]::NewUuid()
        $r = $this.Roundtrip(@{ type = $script:LmxReq.Unlock; uuid = $u; keys = $keys; lockUuid = $lockUuid }, $u)
        if ($r.unlocked -ne $true) { throw "release_many failed: $($r | ConvertTo-Json -Compress)" }
    }

    # -- reader / writer -------------------------------------------------

    [pscustomobject] AcquireWrite([string] $key) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Send(@{ type = $script:LmxReq.RegisterWrite; uuid = $u; key = $key })
        $r = $this.ReadUntilGranted($u)
        return [pscustomobject]@{ Key = $key; FencingToken = $r.fencingToken }
    }

    [pscustomobject] AcquireRead([string] $key) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Send(@{ type = $script:LmxReq.RegisterRead; uuid = $u; key = $key })
        $r = $this.ReadUntilGranted($u)
        return [pscustomobject]@{ Key = $key; FencingToken = $r.fencingToken }
    }

    [void] ReleaseWrite([string] $key) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Roundtrip(@{ type = $script:LmxReq.EndWrite; uuid = $u; key = $key }, $u) | Out-Null
    }

    [void] ReleaseRead([string] $key) {
        $u = [LiveMutexClient]::NewUuid()
        $this.Roundtrip(@{ type = $script:LmxReq.EndRead; uuid = $u; key = $key }, $u) | Out-Null
    }

    # -- introspection ---------------------------------------------------

    [string[]] Ls() {
        $u = [LiveMutexClient]::NewUuid()
        $r = $this.Roundtrip(@{ type = $script:LmxReq.Ls; uuid = $u }, $u)
        if ($null -eq $r.keys) { return @() }
        return $r.keys
    }

    [void] Disconnect() {
        if ($this.Reader) { $this.Reader.Dispose() }
        if ($this.Tcp) { $this.Tcp.Close() }
    }
}
