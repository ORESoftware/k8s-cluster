//! Regression test for the per-frame TCP read cap.
//!
//! Pre-fix, the broker used `BufReader::read_line` which has no
//! upper bound on the per-frame buffer growth. A pre-auth client
//! could open a TCP connection and write arbitrarily many bytes
//! without sending `\n`, ballooning the broker's per-connection
//! buffer until OOM. With the cap in place the broker disconnects
//! the offender once the line crosses `LMX_MAX_FRAME_BYTES` (or the
//! built-in default), and stays available to honest peers.

use std::ffi::OsString;
use std::time::Duration;

use dd_rust_network_mutex::{
    broker::BrokerConfig,
    client::{Client, ClientConfig},
    server::{run as run_server, ServerConfig},
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

static SERVER_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn cfg(tcp: std::net::SocketAddr) -> ServerConfig {
    ServerConfig {
        tcp_bind: Some(tcp),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: false,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    }
}

fn cfg_with_broker(tcp: std::net::SocketAddr, broker: BrokerConfig) -> ServerConfig {
    ServerConfig { broker, ..cfg(tcp) }
}

fn cfg_with_auth(tcp: std::net::SocketAddr, token: &str) -> ServerConfig {
    ServerConfig {
        auth_token: Some(token.to_string()),
        ..cfg(tcp)
    }
}

async fn ephemeral_addr() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    a
}

async fn wait_listening(addr: std::net::SocketAddr) {
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("broker never bound {addr}");
}

async fn read_reply_line<R>(reader: &mut BufReader<R>) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line))
        .await
        .expect("broker did not reply within 3s")
        .expect("read_line failed");
    line
}

#[tokio::test]
async fn disabled_ttl_sweeper_does_not_stop_server() {
    let addr = ephemeral_addr().await;
    let broker = BrokerConfig {
        ttl_sweep_interval: Duration::ZERO,
        ..BrokerConfig::default()
    };
    let server = tokio::spawn(run_server(cfg_with_broker(addr, broker)));
    wait_listening(addr).await;
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(
        !server.is_finished(),
        "disabling the TTL sweeper must not make server::run return"
    );
    server.abort();
}

#[tokio::test]
async fn auth_enabled_tcp_allows_version_probe_before_auth() {
    let _env_lock = SERVER_ENV_LOCK.lock().await;

    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg_with_auth(addr, "secret-token")));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    let version = json!({
        "type": "version",
        "uuid": "preauth-version",
        "value": "0.1.0"
    })
    .to_string();
    write.write_all(version.as_bytes()).await.unwrap();
    write.write_all(b"\n").await.unwrap();
    write.flush().await.unwrap();

    let line = read_reply_line(&mut reader).await;
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["type"], "version");
    assert_eq!(reply["uuid"], "preauth-version");
    assert_eq!(reply["ok"], true);

    let auth = json!({
        "type": "auth",
        "uuid": "preauth-token",
        "token": "secret-token"
    })
    .to_string();
    write.write_all(auth.as_bytes()).await.unwrap();
    write.write_all(b"\n").await.unwrap();
    write.flush().await.unwrap();

    let line = read_reply_line(&mut reader).await;
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["type"], "auth");
    assert_eq!(reply["uuid"], "preauth-token");
    assert_eq!(reply["ok"], true);

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn rust_client_connects_to_auth_enabled_tcp_server() {
    let _env_lock = SERVER_ENV_LOCK.lock().await;

    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg_with_auth(addr, "secret-token")));
    wait_listening(addr).await;

    let client = Client::connect_tcp(
        addr,
        ClientConfig {
            auth_token: Some("secret-token".to_string()),
            ..ClientConfig::default()
        },
    )
    .await
    .expect("auth-enabled Rust client should connect");
    let guard = client
        .acquire("auth-enabled-client-regression", Duration::from_secs(1))
        .await
        .expect("auth-enabled client acquire");
    client
        .release(&guard)
        .await
        .expect("auth-enabled client release");

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn auth_handshake_timeout_drops_idle_pre_auth_connection() {
    let _env_lock = SERVER_ENV_LOCK.lock().await;
    let _auth_timeout = EnvVarGuard::set("LMX_AUTH_HANDSHAKE_MS", "50");

    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg_with_auth(addr, "secret-token")));
    wait_listening(addr).await;

    let mut sock = TcpStream::connect(addr).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut byte))
        .await
        .expect("idle pre-auth connection should be closed by handshake deadline")
        .expect("read after auth timeout");
    assert_eq!(
        read, 0,
        "idle pre-auth connection should close without sending a frame"
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn oversized_frame_disconnects_offender_and_keeps_broker_available() {
    // Force a small cap so the test stays fast. 4 KiB is well below
    // a real composite-lock JSON payload so any actual call would
    // still fit; here we explicitly send way more.
    let _env_lock = SERVER_ENV_LOCK.lock().await;
    let _frame_cap = EnvVarGuard::set("LMX_MAX_FRAME_BYTES", "4096");

    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    // Attacker: send 64 KiB of garbage with no newline. Broker should
    // close the connection once the cap is crossed; the writer side
    // surfaces a structured `Error` frame that we may or may not see
    // depending on scheduling — what we MUST see is the read side
    // returning EOF after a finite amount of bytes.
    let mut atk = TcpStream::connect(addr).await.unwrap();
    let mut blob = vec![b'x'; 64 * 1024];
    blob[0] = b'{';
    let _ = atk.write_all(&blob).await;
    let _ = atk.flush().await;
    let (mut atk_r, mut atk_w) = atk.split();
    let mut sink = Vec::new();
    let read_fut = async {
        let mut tmp = [0u8; 4096];
        loop {
            match atk_r.read(&mut tmp).await {
                Ok(0) => break,
                Ok(n) => sink.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
    };
    let timed_out = tokio::time::timeout(Duration::from_secs(3), read_fut)
        .await
        .is_err();
    let _ = atk_w.shutdown().await;
    assert!(
        !timed_out,
        "broker did not close oversized-frame connection within 3s — DoS vector"
    );

    // Honest client should still be served — the broker survived.
    let honest = TcpStream::connect(addr).await.unwrap();
    let (h_r, mut h_w) = honest.into_split();
    let mut h_r = BufReader::new(h_r);
    let payload = json!({
        "type": "version",
        "uuid": "v1",
        "value": "test"
    })
    .to_string();
    h_w.write_all(payload.as_bytes()).await.unwrap();
    h_w.write_all(b"\n").await.unwrap();
    h_w.flush().await.unwrap();

    let mut reply = String::new();
    let read_one = async {
        use tokio::io::AsyncBufReadExt;
        h_r.read_line(&mut reply).await.unwrap();
    };
    tokio::time::timeout(Duration::from_secs(3), read_one)
        .await
        .expect("honest client got no reply within 3s");
    assert!(
        reply.contains("\"type\":\"version\""),
        "honest client did not get a version reply after attacker DoS attempt; got `{reply}`"
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_accepts_final_json_frame_without_trailing_newline_on_eof() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    let payload = json!({
        "type": "version",
        "uuid": "v-final-no-newline",
        "value": "test"
    })
    .to_string();
    write.write_all(payload.as_bytes()).await.unwrap();
    write.shutdown().await.unwrap();

    let line = read_reply_line(&mut reader).await;
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["type"], "version");
    assert_eq!(reply["uuid"], "v-final-no-newline");

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_processes_buffered_jsonl_then_final_eof_frame() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    let mut stream = Vec::new();
    for uuid in ["buffered-0", "buffered-1"] {
        let payload = json!({
            "type": "version",
            "uuid": uuid,
            "value": "test"
        })
        .to_string();
        stream.extend_from_slice(payload.as_bytes());
        stream.push(b'\n');
    }

    let final_payload = json!({
        "type": "version",
        "uuid": "buffered-final-no-newline",
        "value": "test"
    })
    .to_string();
    stream.extend_from_slice(final_payload.as_bytes());

    write.write_all(&stream).await.unwrap();
    write.shutdown().await.unwrap();

    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..3 {
        let line = read_reply_line(&mut reader).await;
        let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(reply["type"], "version");
        seen.insert(reply["uuid"].as_str().unwrap().to_string());
    }

    assert_eq!(
        seen,
        std::collections::BTreeSet::from([
            "buffered-0".to_string(),
            "buffered-1".to_string(),
            "buffered-final-no-newline".to_string(),
        ])
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_reports_malformed_final_frame_without_trailing_newline_on_eof() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    write
        .write_all(b"{\"type\":\"version\",\"uuid\":\"bad-final\"")
        .await
        .unwrap();
    write.shutdown().await.unwrap();

    let line = read_reply_line(&mut reader).await;
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["uuid"], "malformed");
    assert!(
        reply["error"]
            .as_str()
            .unwrap_or("")
            .contains("malformed request"),
        "malformed final frame should produce a structured parser error, got {reply:?}"
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_recovers_after_malformed_object_frame() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    write
        .write_all(b"{\"type\":\"version\",\"uuid\":\"bad\",}\n")
        .await
        .unwrap();

    let payload = json!({
        "type": "version",
        "uuid": "after-bad-object",
        "value": "test"
    })
    .to_string();
    write.write_all(payload.as_bytes()).await.unwrap();
    write.write_all(b"\n").await.unwrap();
    write.flush().await.unwrap();

    let mut replies = Vec::new();
    for _ in 0..2 {
        let line = read_reply_line(&mut reader).await;
        replies.push(serde_json::from_str::<serde_json::Value>(line.trim()).unwrap());
    }

    assert!(
        replies.iter().any(|reply| {
            reply["type"] == "error"
                && reply["uuid"] == "malformed"
                && reply["error"]
                    .as_str()
                    .is_some_and(|err| err.contains("malformed request"))
        }),
        "broker did not report the malformed frame: {replies:?}"
    );
    assert!(
        replies
            .iter()
            .any(|reply| reply["type"] == "version" && reply["uuid"] == "after-bad-object"),
        "broker did not recover for the next valid frame: {replies:?}"
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_preserves_split_utf8_jsonl_frame() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    let payload = "{\"type\":\"version\",\"uuid\":\"split-😊\",\"value\":\"test\"}\n";
    let emoji = "😊".as_bytes();
    let split = payload
        .as_bytes()
        .windows(emoji.len())
        .position(|w| w == emoji)
        .expect("payload should contain emoji bytes")
        + 1;

    write.write_all(&payload.as_bytes()[..split]).await.unwrap();
    tokio::task::yield_now().await;
    write.write_all(&payload.as_bytes()[split..]).await.unwrap();
    write.flush().await.unwrap();

    let line = read_reply_line(&mut reader).await;
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["type"], "version");
    assert_eq!(reply["uuid"], "split-😊");

    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn broker_drains_large_jsonl_burst_with_frame_yield_option() {
    let _env_lock = SERVER_ENV_LOCK.lock().await;
    let _frame_yield = EnvVarGuard::set("LMX_FRAME_YIELD_EVERY", "1");

    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    let total = 256usize;
    let mut burst = Vec::new();
    for i in 0..total {
        let payload = json!({
            "type": "version",
            "uuid": format!("burst-{i}"),
            "value": "test"
        })
        .to_string();
        burst.extend_from_slice(payload.as_bytes());
        burst.push(b'\n');
    }
    write.write_all(&burst).await.unwrap();
    write.flush().await.unwrap();

    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..total {
        let line = read_reply_line(&mut reader).await;
        let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(reply["type"], "version");
        seen.insert(reply["uuid"].as_str().unwrap().to_string());
    }

    assert_eq!(seen.len(), total);
    assert!(seen.contains("burst-0"));
    assert!(seen.contains(&format!("burst-{}", total - 1)));

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn broker_handles_empty_malformed_and_crlf_jsonl_frames() {
    let addr = ephemeral_addr().await;
    let server = tokio::spawn(run_server(cfg(addr)));
    wait_listening(addr).await;

    let sock = TcpStream::connect(addr).await.unwrap();
    let (read, mut write) = sock.into_split();
    let mut reader = BufReader::new(read);

    write.write_all(b"\n\r\nnot-json\n").await.unwrap();
    let payload = json!({
        "type": "version",
        "uuid": "v-crlf",
        "value": "test"
    })
    .to_string();
    write.write_all(payload.as_bytes()).await.unwrap();
    write.write_all(b"\r\n").await.unwrap();
    write.flush().await.unwrap();

    let mut replies = Vec::new();
    for _ in 0..2 {
        replies.push(read_reply_line(&mut reader).await);
    }

    assert!(
        replies
            .iter()
            .any(|line| line.contains("\"type\":\"error\"") && line.contains("malformed")),
        "broker did not report malformed JSON; replies: {replies:?}"
    );
    assert!(
        replies
            .iter()
            .any(|line| line.contains("\"type\":\"version\"") && line.contains("v-crlf")),
        "broker did not accept CRLF version frame after malformed input; replies: {replies:?}"
    );

    server.abort();
    let _ = server.await;
}
