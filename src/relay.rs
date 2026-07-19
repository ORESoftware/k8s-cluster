//! Relay node.
//!
//! A relay accepts an inbound TCP link from the previous hop (the client or an
//! upstream relay), performs the handshake, then forwards cells. It holds
//! exactly one symmetric key pair (forward/backward) and knows only its
//! immediate neighbours — never the full path, never the payload destined for
//! deeper hops.
//!
//! A relay's role is decided by the first control cell it receives:
//!   * `Extend`  -> it becomes a *middle* relay: it opens a link to the next
//!                  relay and thereafter forwards `Relay` cells verbatim.
//!   * `Begin`   -> it becomes the *exit*: it opens a TCP connection to the
//!                  real destination and shuttles application `Data`.

use anyhow::{anyhow, bail, Result};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use x25519_dalek::StaticSecret;

use crate::cell::Cell;
use crate::crypto::{relay_handshake, Opener, Sealer};
use crate::policy::Policy;
use crate::wire::{frame_bytes, read_frame, write_frame};

/// Bytes read from the exit's destination per chunk before being wrapped in a
/// `Data` cell.
const EXIT_READ_CHUNK: usize = 32 * 1024;

/// A stalled peer must complete the handshake within this window, so half-open
/// connections cannot pile up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Bound on dialing the next hop / destination, so a black-holed target cannot
/// pin a circuit slot during connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

fn max_circuits() -> Result<usize> {
    let value = match std::env::var("TOR_MAX_CIRCUITS") {
        Ok(raw) => raw
            .parse::<usize>()
            .map_err(|_| anyhow!("TOR_MAX_CIRCUITS must be a positive integer"))?,
        Err(_) => 1024,
    };
    if value == 0 || value > 1_000_000 {
        bail!("TOR_MAX_CIRCUITS must be between 1 and 1000000");
    }
    return Ok(value);
}

/// Optional idle timeout on the forward read loop (0 = disabled). Bounds
/// post-handshake slowloris, where a peer completes the handshake then holds
/// the circuit open sending nothing.
///
/// Applies to BOTH the forward read loop and the detached backward pumps, so a
/// peer that goes silent cannot park a pump forever and leak its circuit-slot
/// permit. Defaults to a finite value (a disabled/0 timeout re-enables the
/// slot-exhaustion DoS); operators with legitimately long-idle streams can raise
/// it. 0 disables (not recommended).
const DEFAULT_IDLE_SECS: u64 = 600;

fn idle_timeout() -> Option<Duration> {
    let secs: u64 = std::env::var("TOR_CIRCUIT_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_IDLE_SECS);
    return if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    };
}

/// Connect to `addr` with a bounded timeout.
async fn connect_timeout(addr: impl tokio::net::ToSocketAddrs) -> Result<TcpStream> {
    return timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))?
        .map_err(|e| anyhow!("connect failed: {e}"));
}

/// Try every permitted DNS answer within one bounded deadline. This avoids an
/// unreachable IPv6 answer preventing a usable IPv4 connection (or vice versa).
async fn connect_resolved(addrs: &[std::net::SocketAddr]) -> Result<TcpStream> {
    let attempt = async {
        let mut last_error = None;
        for addr in addrs {
            match TcpStream::connect(addr).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }
        return Err(anyhow!(
            "all {} resolved destination addresses failed{}",
            addrs.len(),
            last_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ));
    };

    return timeout(CONNECT_TIMEOUT, attempt)
        .await
        .map_err(|_| anyhow!("connect timed out"))?;
}

pub async fn run(listen: &str, secret: StaticSecret) -> Result<()> {
    let secret = Arc::new(secret);
    let policy = Arc::new(Policy::from_env()?);
    let limit = Arc::new(Semaphore::new(max_circuits()?));
    let listener = TcpListener::bind(listen).await?;
    if !policy.extend_allowlisted() && !is_loopback_listen(listen) {
        warn!(
            "relay is non-loopback and TOR_RELAY_PEERS is unset; Extend targets are unrestricted — pin the allowlist to prevent relay-to-arbitrary-host connections"
        );
    }
    info!(
        %listen,
        allow_private_exit = policy.allow_private_exit(),
        exit_enabled = policy.exit_enabled(),
        "relay listening"
    );
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("accept failed: {e}");
                continue;
            }
        };
        // Reject rather than queue when saturated, bounding memory/FD use. The
        // permit is shared (Arc) with the backward pump the circuit later spawns
        // so a circuit only frees its slot once *both* directions have finished;
        // a detached pump must not outlive the accounting.
        let permit = match limit.clone().try_acquire_owned() {
            Ok(p) => Arc::new(p),
            Err(_) => {
                debug!("at circuit capacity, dropping {peer}");
                continue;
            }
        };
        let secret = secret.clone();
        let policy = policy.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_circuit(stream, secret, policy, permit).await {
                debug!("circuit from {peer} ended: {e:#}");
            }
        });
    }
}

/// Loopback check mirroring `main::is_loopback_listener` for the relay warning.
fn is_loopback_listen(listen: &str) -> bool {
    return listen
        .parse::<std::net::SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false);
}

async fn handle_circuit(
    prev: TcpStream,
    secret: Arc<StaticSecret>,
    policy: Arc<Policy>,
    permit: Arc<OwnedSemaphorePermit>,
) -> Result<()> {
    prev.set_nodelay(true).ok();
    let (mut prev_r, mut prev_w) = prev.into_split();

    // First frame on the link is always the CREATE handshake blob.
    let create = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut prev_r))
        .await
        .map_err(|_| anyhow!("handshake timed out"))??;
    let (created, keys) = relay_handshake(&secret, &create)?;
    write_frame(&mut prev_w, &created).await?;

    let mut opener_fwd = Opener::new(keys.forward);

    // `prev_w` and the backward sealer are handed to whichever pump we spawn
    // once this relay learns its role. Until then they stay here.
    let mut prev_w = Some(prev_w);
    let mut sealer_bwd = Some(Sealer::new(keys.backward));

    // At most one of these is ever set (a relay has exactly one next hop).
    let mut next_w: Option<OwnedWriteHalf> = None; // middle: link to next relay
    let mut dest_w: Option<OwnedWriteHalf> = None; // exit: link to destination

    let idle = idle_timeout();
    loop {
        let read = read_frame(&mut prev_r);
        let frame = match idle {
            Some(d) => match timeout(d, read).await {
                Ok(Ok(f)) => f,
                Ok(Err(_)) => break, // previous hop closed
                Err(_) => break,     // idle timeout
            },
            None => match read.await {
                Ok(f) => f,
                Err(_) => break,
            },
        };
        let plaintext = opener_fwd.open(&frame)?;
        let cell = Cell::decode(&plaintext)?;
        match cell {
            Cell::Extend { addr, create } => {
                if next_w.is_some() || dest_w.is_some() {
                    bail!("Extend after this relay already has a next hop");
                }
                policy.check_extend(&addr)?;
                let next = connect_timeout(&addr).await?;
                next.set_nodelay(true).ok();
                let (next_r, mut nw) = next.into_split();
                write_frame(&mut nw, &create).await?;
                next_w = Some(nw);
                let pw = prev_w
                    .take()
                    .ok_or_else(|| anyhow!("prev_w already taken"))?;
                let sealer = sealer_bwd
                    .take()
                    .ok_or_else(|| anyhow!("sealer already taken"))?;
                let hold = permit.clone();
                tokio::spawn(async move {
                    if let Err(e) = middle_pump(next_r, pw, sealer, idle).await {
                        debug!("middle pump ended: {e:#}");
                    }
                    drop(hold); // release the circuit slot only when this direction ends
                });
            }
            Cell::Relay { payload } => {
                let nw = next_w
                    .as_mut()
                    .ok_or_else(|| anyhow!("Relay cell before Extend"))?;
                // `payload` is already a complete framed blob for the next hop.
                nw.write_all(&payload).await?;
                nw.flush().await?;
            }
            Cell::Begin { host, port } => {
                if next_w.is_some() || dest_w.is_some() {
                    bail!("Begin after this relay already has a next hop");
                }
                // Exit policy: enforce exit-enabled, resolve + reject
                // private/loopback/metadata ranges.
                let addrs = policy.resolve_exit(&host, port).await?;
                let dest = connect_resolved(&addrs).await?;
                dest.set_nodelay(true).ok();
                let (dest_r, dw) = dest.into_split();
                dest_w = Some(dw);
                let pw = prev_w
                    .take()
                    .ok_or_else(|| anyhow!("prev_w already taken"))?;
                let sealer = sealer_bwd
                    .take()
                    .ok_or_else(|| anyhow!("sealer already taken"))?;
                let hold = permit.clone();
                tokio::spawn(async move {
                    if let Err(e) = exit_pump(dest_r, pw, sealer).await {
                        debug!("exit pump ended: {e:#}");
                    }
                    drop(hold); // release the circuit slot only when this direction ends
                });
            }
            Cell::Data { bytes } => {
                let dw = dest_w
                    .as_mut()
                    .ok_or_else(|| anyhow!("Data cell before Begin"))?;
                dw.write_all(&bytes).await?;
                dw.flush().await?;
            }
            Cell::End => break,
        }
    }
    return Ok(());
}

/// Middle-relay backward path: read framed blobs coming back from the next hop,
/// wrap each in a `Relay` cell, seal it under our backward key, and send toward
/// the previous hop.
async fn middle_pump(
    mut next_r: OwnedReadHalf,
    mut prev_w: OwnedWriteHalf,
    mut sealer: Sealer,
) -> Result<()> {
    loop {
        let frame = match read_frame(&mut next_r).await {
            Ok(f) => f,
            Err(_) => break,
        };
        let cell = Cell::Relay {
            payload: frame_bytes(&frame),
        };
        let ct = sealer.seal(&cell.encode()?)?;
        write_frame(&mut prev_w, &ct).await?;
    }
    return Ok(());
}

/// Exit-relay backward path: read raw application bytes from the destination,
/// wrap each chunk in a `Data` cell sealed under our backward key, and send
/// toward the previous hop. On EOF, send an `End` cell.
async fn exit_pump(
    mut dest_r: OwnedReadHalf,
    mut prev_w: OwnedWriteHalf,
    mut sealer: Sealer,
) -> Result<()> {
    let mut buf = vec![0u8; EXIT_READ_CHUNK];
    loop {
        let n = match dest_r.read(&mut buf).await {
            Ok(0) => {
                let ct = sealer.seal(&Cell::End.encode()?)?;
                let _ = write_frame(&mut prev_w, &ct).await;
                break;
            }
            Ok(n) => n,
            Err(_) => break,
        };
        let cell = Cell::Data {
            bytes: buf[..n].to_vec(),
        };
        let ct = sealer.seal(&cell.encode()?)?;
        write_frame(&mut prev_w, &ct).await?;
    }
    return Ok(());
}
