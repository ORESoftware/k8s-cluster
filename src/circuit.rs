//! Client-side circuit construction and data relaying.
//!
//! The client telescopes a circuit: it handshakes with the first relay
//! directly, then repeatedly asks the current last hop to `Extend` to the next
//! relay, tunnelling each new handshake through the layers already established.
//! Once built, application bytes are onion-wrapped for the exit and unwrapped
//! layer by layer on the way back.

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use tracing::debug;

use crate::cell::Cell;
use crate::config::RelayInfo;
use crate::crypto::{ClientHandshake, Opener, Sealer};
use crate::wire::{frame_bytes, read_frame, strip_len, write_frame};

const CLIENT_READ_CHUNK: usize = 32 * 1024;

/// Default backward-idle timeout for a spliced stream. Bounds an abandoned or
/// stalled circuit so a detached splice task cannot pin the entry-relay socket
/// and circuit state indefinitely (0 disables). Overridable per deployment.
const DEFAULT_CLIENT_IDLE_SECS: u64 = 600;

fn client_idle_timeout() -> Option<Duration> {
    let secs = std::env::var("TOR_CLIENT_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CLIENT_IDLE_SECS);
    return if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    };
}

pub struct Circuit {
    r0_r: OwnedReadHalf,
    r0_w: OwnedWriteHalf,
    /// Forward (client -> relay) sealer per hop, index 0 = entry.
    sealers_fwd: Vec<Sealer>,
    /// Backward (relay -> client) opener per hop, index 0 = entry.
    openers_bwd: Vec<Opener>,
}

impl Circuit {
    /// Build a circuit along `path` (`path[0]` is the entry, the last is the exit).
    pub async fn build(path: &[RelayInfo]) -> Result<Circuit> {
        if path.is_empty() {
            bail!("cannot build a circuit with no relays");
        }
        let entry = &path[0];
        let stream = TcpStream::connect(&entry.addr).await?;
        stream.set_nodelay(true).ok();
        let (mut r0_r, mut r0_w) = stream.into_split();

        // Hop 0: direct handshake, CREATED comes back in the clear.
        let hs = ClientHandshake::new(entry.pubkey_bytes()?);
        write_frame(&mut r0_w, &hs.create_blob()).await?;
        let created = read_frame(&mut r0_r).await?;
        let keys0 = hs.finish(&created)?;

        let mut sealers_fwd = vec![Sealer::new(keys0.forward)];
        let mut openers_bwd = vec![Opener::new(keys0.backward)];

        // Hops 1..: telescope outward.
        for i in 1..path.len() {
            let relay = &path[i];
            let hs = ClientHandshake::new(relay.pubkey_bytes()?);
            let extend = Cell::Extend {
                addr: relay.addr.clone(),
                create: hs.create_blob(),
            };
            // The Extend is executed by hop i-1 (the current last hop).
            let framed = wrap_forward(&mut sealers_fwd, i - 1, &extend)?;
            write_frame(&mut r0_w, &framed).await?;

            // The CREATED from hop i returns wrapped in `i` Relay layers.
            let created = recv_created(&mut r0_r, &mut openers_bwd, i).await?;
            let keys = hs.finish(&created)?;
            sealers_fwd.push(Sealer::new(keys.forward));
            openers_bwd.push(Opener::new(keys.backward));
        }

        debug!(hops = path.len(), "circuit built");
        return Ok(Circuit {
            r0_r,
            r0_w,
            sealers_fwd,
            openers_bwd,
        });
    }

    fn exit_index(&self) -> usize {
        return self.sealers_fwd.len() - 1;
    }

    /// Ask the exit to open a connection to `host:port`.
    pub async fn begin(&mut self, host: &str, port: u16) -> Result<()> {
        let cell = Cell::Begin {
            host: host.to_string(),
            port,
        };
        let target = self.exit_index();
        let framed = wrap_forward(&mut self.sealers_fwd, target, &cell)?;
        write_frame(&mut self.r0_w, &framed).await?;
        return Ok(());
    }

    /// Splice a local application stream onto this circuit, relaying in both
    /// directions until either side closes. Works over a real TCP socket
    /// (SOCKS) or an in-memory duplex pipe (the web fetch feature).
    pub async fn splice<S>(self, app: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut app_r, mut app_w) = tokio::io::split(app);

        let Circuit {
            mut r0_r,
            mut r0_w,
            mut sealers_fwd,
            mut openers_bwd,
        } = self;
        let exit = sealers_fwd.len() - 1;

        // App -> circuit: wrap each chunk as Data for the exit.
        let up = async move {
            let mut buf = vec![0u8; CLIENT_READ_CHUNK];
            loop {
                let n = match app_r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let cell = Cell::Data {
                    bytes: buf[..n].to_vec(),
                };
                let framed = match wrap_forward(&mut sealers_fwd, exit, &cell) {
                    Ok(f) => f,
                    Err(_) => break,
                };
                if write_frame(&mut r0_w, &framed).await.is_err() {
                    break;
                }
            }
            // Best-effort teardown of the exit's stream.
            if let Ok(framed) = wrap_forward(&mut sealers_fwd, exit, &Cell::End) {
                let _ = write_frame(&mut r0_w, &framed).await;
            }
        };

        // Circuit -> app: peel layers until a Data (or End) cell.
        let down = async move {
            loop {
                let frame = match read_frame(&mut r0_r).await {
                    Ok(f) => f,
                    Err(_) => break,
                };
                match peel_backward(&mut openers_bwd, frame) {
                    Ok(Peeled::Data(bytes)) => {
                        if app_w.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Ok(Peeled::End) => break,
                    Err(e) => {
                        debug!("peel failed: {e:#}");
                        break;
                    }
                }
            }
            let _ = app_w.shutdown().await;
        };

        tokio::join!(up, down);
        return Ok(());
    }
}

/// Onion-wrap `cell` so it is delivered to hop `target`: seal it under the
/// target's forward key, then wrap-and-seal it as a `Relay` cell at each
/// shallower hop down to the entry. Returns the outermost ciphertext to write.
fn wrap_forward(sealers: &mut [Sealer], target: usize, cell: &Cell) -> Result<Vec<u8>> {
    if target >= sealers.len() {
        bail!(
            "wrap_forward target {target} out of range {}",
            sealers.len()
        );
    }
    let mut ct = sealers[target].seal(&cell.encode()?)?;
    let mut m = target;
    while m > 0 {
        m -= 1;
        let relay_cell = Cell::Relay {
            payload: frame_bytes(&ct),
        };
        ct = sealers[m].seal(&relay_cell.encode()?)?;
    }
    return Ok(ct);
}

/// Read one backward frame and peel exactly `hops_deep` `Relay` layers, yielding
/// the raw (still un-parsed) inner blob — used for CREATED replies during build.
async fn recv_created(
    r0_r: &mut OwnedReadHalf,
    openers: &mut [Opener],
    hops_deep: usize,
) -> Result<Vec<u8>> {
    let mut ct = read_frame(r0_r).await?;
    for idx in 0..hops_deep {
        let plaintext = openers[idx].open(&ct)?;
        match Cell::decode(&plaintext)? {
            Cell::Relay { payload } => {
                ct = strip_len(&payload)?;
            }
            other => bail!("expected Relay while peeling CREATED, got {other:?}"),
        }
    }
    return Ok(ct);
}

enum Peeled {
    Data(Vec<u8>),
    End,
}

/// Peel a backward frame layer by layer until a terminal cell.
fn peel_backward(openers: &mut [Opener], frame: Vec<u8>) -> Result<Peeled> {
    let mut ct = frame;
    let mut idx = 0usize;
    loop {
        if idx >= openers.len() {
            bail!("ran out of onion layers while peeling backward frame");
        }
        let plaintext = openers[idx].open(&ct)?;
        match Cell::decode(&plaintext)? {
            Cell::Relay { payload } => {
                ct = strip_len(&payload)?;
                idx += 1;
            }
            Cell::Data { bytes } => return Ok(Peeled::Data(bytes)),
            Cell::End => return Ok(Peeled::End),
            other => bail!("unexpected cell while peeling backward: {other:?}"),
        }
    }
}
