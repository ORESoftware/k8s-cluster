//! TCP socket-tuning helpers, gated by Cargo `cfg(target_os = "linux")`.
//!
//! This implements the experiment requested in upstream issue
//! [`live-mutex#22`](https://github.com/ORESoftware/live-mutex/issues/22):
//!
//! - **`TCP_NODELAY`**: disable Nagle's algorithm so a small write isn't
//!   buffered waiting for more data. We already set this on every client
//!   (Rust, TS, Go, Dart, Gleam) and now on the broker's accepted sockets
//!   too.
//! - **`TCP_QUICKACK`** (Linux only): disable the kernel's "delayed ACK"
//!   heuristic so an incoming frame is ACKed immediately. This option is
//!   *one-shot* — the kernel re-arms delayed-ACK after the next ACK is
//!   sent — so for sustained benefit we re-apply it after every read
//!   (see `apply_quickack`).
//!
//! Together NODELAY + QUICKACK eliminate the classic ~40 ms RTT cliff
//! that hits request/response RPCs with small frames over real networks.
//! On loopback this is mostly invisible; the latency probe at
//! `clients/ts/src/latency_probe.ts` is set up to A/B-test the delta in
//! a real environment.

use std::io;

#[cfg(any(test, feature = "tls"))]
#[allow(unused_imports)]
use std::os::fd::AsRawFd;

/// Apply `TCP_NODELAY = 1` to a TCP stream. Errors are not fatal — the
/// option is a hint, and we shouldn't fail the connection on a tunable.
pub fn apply_nodelay(stream: &tokio::net::TcpStream) -> io::Result<()> {
    crate::routine_id!("ddl-routine-F00CV-i_xRTMHqbbNC");
    stream.set_nodelay(true)
}

/// Apply `TCP_QUICKACK = 1` on Linux. No-op on non-Linux. Returns `Ok(false)`
/// on platforms where the option doesn't exist so callers can update a
/// "applied" counter only on real applications.
pub fn apply_quickack(_fd: std::os::fd::RawFd) -> io::Result<bool> {
    crate::routine_id!("ddl-routine-FrCnst7QDKZRlB2v54");
    #[cfg(target_os = "linux")]
    {
        let on: libc::c_int = 1;
        let rc = unsafe {
            libc::setsockopt(
                _fd,
                libc::IPPROTO_TCP,
                libc::TCP_QUICKACK,
                &on as *const _ as *const _,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(true);
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

/// Whether `TCP_QUICKACK` is available on this build. Used by `metrics.rs`
/// and the startup banner to tell operators whether the option will be
/// honored at runtime.
pub fn quickack_supported() -> bool {
    crate::routine_id!("ddl-routine-rkljS_AVn7foTX-8hV");
    cfg!(target_os = "linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_nodelay_on_loopback_socket() {
        crate::routine_id!("ddl-routine-T83O_OkMaKYz4C2tBU");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _h = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        // NODELAY must succeed everywhere (BSD-style sockets, not Linux-specific).
        apply_nodelay(&stream).expect("set_nodelay on loopback should succeed");
    }

    #[tokio::test]
    async fn apply_quickack_is_safe_on_loopback() {
        crate::routine_id!("ddl-routine-gUcsz83ImIyMXWxekS");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _h = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let fd = stream.as_raw_fd();
        // On Linux this returns Ok(true). On macOS / BSD it's a no-op
        // (Ok(false)) — the function MUST NOT fail just because the
        // option doesn't exist there, otherwise the broker can't run on
        // dev laptops.
        let applied =
            apply_quickack(fd).expect("apply_quickack must not error on supported builds");
        assert_eq!(applied, quickack_supported());
    }
}
