//! ai-agent-bridge — token-protected LAN inbox so a peer AI agent (e.g. the Codex
//! MacBook) can push messages to THIS Claude session. Claude polls `inbox.jsonl`
//! via a watcher, processes each message, and replies by POSTing to the peer's own
//! bridge. Symmetric to the peer HTTP bridge.
//!
//! Rust port of the retired `claude_inbox_bridge.py`, kept byte-compatible on the
//! wire so existing senders/watchers keep working:
//!
//!   POST /claude  {"prompt": "...", "from": "codex", "topic": "plateau"}  (Bearer token)
//!        -> appends a JSON line to INBOX and returns {"queued": true, "id": <id>}
//!   GET  /health
//!
//! Dependency-light on purpose: a threaded std-net HTTP/1.1 server (mirrors Python's
//! ThreadingTCPServer) + serde_json. No async runtime for two routes.

use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Read the first present env var from `keys`, else `default`.
fn env_or(keys: &[&str], default: &str) -> String {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    default.to_string()
}

struct Config {
    port: u16,
    token: String,
    dir: PathBuf,
}

impl Config {
    fn from_env() -> Self {
        // New AI_AGENT_BRIDGE_* names, falling back to the legacy CLAUDE_INBOX_*
        // names so this is a drop-in replacement for the Python bridge.
        let port = env_or(&["AI_AGENT_BRIDGE_PORT", "CLAUDE_INBOX_PORT"], "8766")
            .parse()
            .unwrap_or(8766);
        let token = env_or(&["AI_AGENT_BRIDGE_TOKEN", "CLAUDE_INBOX_TOKEN"], "");
        let dir = PathBuf::from(env_or(
            &["AI_AGENT_BRIDGE_DIR", "CLAUDE_INBOX_DIR"],
            "/tmp/claude_bridge",
        ));
        Config { port, token, dir }
    }

    fn inbox_path(&self) -> PathBuf {
        self.dir.join("inbox.jsonl")
    }
}

/// A parsed HTTP/1.1 request: just what these two routes need.
struct Request {
    method: String,
    path: String,
    auth: String,
    body: Vec<u8>,
}

/// Format a unix-seconds instant as `YYYY-MM-DDTHH:MM:SSZ` (UTC), matching the
/// Python `time.strftime("%Y-%m-%dT%H:%M:%SZ", gmtime())`. Uses Howard Hinnant's
/// days->civil algorithm so we need no chrono dependency.
fn iso8601_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as i64;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hh, mm, ss
    )
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Count lines in the inbox file (0 if it does not exist yet).
fn inbox_count(path: &PathBuf) -> usize {
    match File::open(path) {
        Ok(f) => BufReader::new(f).lines().count(),
        Err(_) => 0,
    }
}

fn parse_request(stream: &TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream);

    // Request line.
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.trim_end().split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // Headers until the blank line.
    let mut auth = String::new();
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h)?;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "authorization" => auth = val.to_string(),
                "content-length" => content_length = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    // Body (exactly content-length bytes).
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Request {
        method,
        path,
        auth,
        body,
    })
}

fn send_json(stream: &mut TcpStream, code: u16, reason: &str, obj: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(obj).unwrap_or_else(|_| b"{}".to_vec());
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code,
        reason,
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn handle(mut stream: TcpStream, cfg: Arc<Config>) {
    let req = match parse_request(&stream) {
        Ok(r) => r,
        Err(_) => {
            let _ = send_json(&mut stream, 400, "Bad Request", &json!({"error": "bad request"}));
            return;
        }
    };

    // GET /health — no auth, matches the Python payload shape.
    if req.method == "GET" && req.path == "/health" {
        let n = inbox_count(&cfg.inbox_path());
        let _ = send_json(
            &mut stream,
            200,
            "OK",
            &json!({
                "ok": true,
                "service": "ai-agent-bridge",
                "port": cfg.port,
                "inbox_messages": n,
                "auth": "Bearer token required for POST /claude",
            }),
        );
        return;
    }

    // POST /claude — bearer-protected inbox append.
    if req.method == "POST" && req.path == "/claude" {
        if !cfg.token.is_empty() && req.auth != format!("Bearer {}", cfg.token) {
            let _ = send_json(&mut stream, 401, "Unauthorized", &json!({"error": "unauthorized"}));
            return;
        }

        let data: Value = if req.body.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => {
                    let _ = send_json(
                        &mut stream,
                        400,
                        "Bad Request",
                        &json!({"error": format!("bad json: {e}")}),
                    );
                    return;
                }
            }
        };

        let get_str = |key: &str, default: &str, max: usize| -> String {
            let s = data
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string();
            s.chars().take(max).collect()
        };

        let secs = (now_millis() / 1000) as u64;
        let id = now_millis() as u64;
        let msg = json!({
            "id": id,
            "ts": iso8601_utc(secs),
            "from": get_str("from", "codex", 64),
            "topic": get_str("topic", "", 128),
            "prompt": data.get("prompt").and_then(Value::as_str).unwrap_or("").to_string(),
        });

        if let Err(e) = append_inbox(&cfg, &msg) {
            let _ = send_json(
                &mut stream,
                500,
                "Internal Server Error",
                &json!({"error": format!("inbox write failed: {e}")}),
            );
            return;
        }

        let _ = send_json(
            &mut stream,
            200,
            "OK",
            &json!({
                "queued": true,
                "id": id,
                "note": "Claude will read this on its next watcher wake and reply via the peer bridge.",
            }),
        );
        return;
    }

    let _ = send_json(&mut stream, 404, "Not Found", &json!({"error": "not found"}));
}

fn append_inbox(cfg: &Config, msg: &Value) -> std::io::Result<()> {
    create_dir_all(&cfg.dir)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(cfg.inbox_path())?;
    let mut line = serde_json::to_string(msg).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    f.write_all(line.as_bytes())
}

fn main() -> std::io::Result<()> {
    let cfg = Arc::new(Config::from_env());
    create_dir_all(&cfg.dir)?;
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
    println!(
        "ai-agent-bridge listening on 0.0.0.0:{} inbox={} auth={}",
        cfg.port,
        cfg.inbox_path().display(),
        if cfg.token.is_empty() { "OPEN" } else { "bearer" }
    );

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let cfg = Arc::clone(&cfg);
                thread::spawn(move || handle(s, cfg));
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
