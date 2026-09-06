//! Live concurrency stress over the real TCP wire (not the in-process synchronous
//! broker API). Spins up a broker, then drives many independent tokio tasks /
//! client connections that contend on a small key space with single-key and
//! composite (multi-key) blocking acquires.
//!
//! Invariants enforced against a shared shadow model under every grant/release:
//! * mutual exclusion  — no two live holders ever overlap on a key;
//! * fencing monotonic — each grant's per-key token strictly exceeds the last
//!   token observed for that key, across all tasks;
//! * composite atomicity — a composite guard carries a token for every key.
//!
//! Ordering rule that makes the model race-free: we mark a key FREE in the
//! model *before* releasing it on the broker, so the broker can't hand the key
//! to the next waiter until the model already reflects the release.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dd_rust_network_mutex::{server, BrokerConfig, Client, ClientConfig, ServerConfig};
use tokio::net::TcpListener;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }
}

#[derive(Default)]
struct KeyModel {
    occupied: bool,
    last_token: u64,
}

struct Model {
    keys: Mutex<HashMap<String, KeyModel>>,
    violations: AtomicUsize,
}

impl Model {
    fn new() -> Self {
        Model {
            keys: Mutex::new(HashMap::new()),
            violations: AtomicUsize::new(0),
        }
    }

    /// Called immediately after the broker grants `keys` with `tokens`.
    fn on_grant(&self, keys: &[String], tokens: &HashMap<String, u64>) {
        let mut map = self.keys.lock().unwrap();
        for k in keys {
            let entry = map.entry(k.clone()).or_default();
            if entry.occupied {
                self.violations.fetch_add(1, Ordering::Relaxed);
                eprintln!("MUTUAL-EXCLUSION VIOLATION: {k} granted while already held");
            }
            let tok = *tokens.get(k).unwrap_or(&0);
            if tok <= entry.last_token {
                self.violations.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "FENCING VIOLATION: {k} token {tok} <= last {} (not strictly increasing)",
                    entry.last_token
                );
            }
            if tok == 0 {
                self.violations.fetch_add(1, Ordering::Relaxed);
                eprintln!("FENCING VIOLATION: {k} granted with zero/absent token");
            }
            entry.occupied = true;
            entry.last_token = tok.max(entry.last_token);
        }
    }

    /// Called just BEFORE releasing on the broker.
    fn on_release(&self, keys: &[String]) {
        let mut map = self.keys.lock().unwrap();
        for k in keys {
            if let Some(entry) = map.get_mut(k) {
                entry.occupied = false;
            }
        }
    }

    fn all_free(&self) -> bool {
        self.keys.lock().unwrap().values().all(|e| !e.occupied)
    }
}

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_broker() -> u16 {
    let port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{port}").parse().unwrap()),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_fencing_and_multikey_over_wire() {
    let port = start_broker().await;
    let model = Arc::new(Model::new());

    let n_tasks = 32usize;
    let iters = 120usize;
    let n_keys = 4usize;
    let keys: Vec<String> = (0..n_keys).map(|i| format!("stress-k{i}")).collect();

    let mut handles = Vec::new();
    for task_id in 0..n_tasks {
        let model = Arc::clone(&model);
        let keys = keys.clone();
        handles.push(tokio::spawn(async move {
            // Generous per-request timeout: composite acquires use sorted-order
            // hold-and-wait, so under heavy contention an individual grant can
            // lag well past the 5s default. We still bound each op with a 30s
            // tokio guard below, which would catch a genuine deadlock/livelock.
            let cfg = ClientConfig {
                default_request_timeout: Duration::from_secs(45),
                ..Default::default()
            };
            let client = Client::connect_tcp(("127.0.0.1", port), cfg)
                .await
                .expect("connect");
            let mut rng = Rng::new(0xABCD_0000 + task_id as u64);

            for _ in 0..iters {
                let composite = rng.below(100) < 45;
                if composite {
                    // 2..=3 distinct keys.
                    let want = 2 + rng.below(2);
                    let mut pool = keys.clone();
                    let mut chosen: Vec<String> = Vec::new();
                    for _ in 0..want {
                        if pool.is_empty() {
                            break;
                        }
                        chosen.push(pool.remove(rng.below(pool.len())));
                    }
                    let refs: Vec<&str> = chosen.iter().map(|s| s.as_str()).collect();
                    let guard = match tokio::time::timeout(
                        Duration::from_secs(30),
                        client.acquire_composite(&refs, Duration::from_millis(60_000)),
                    )
                    .await
                    {
                        Ok(Ok(g)) => g,
                        Ok(Err(e)) => panic!("task {task_id} composite acquire failed: {e:?}"),
                        Err(_) => {
                            panic!("task {task_id} composite acquire timed out (possible deadlock)")
                        }
                    };
                    let mut toks: HashMap<String, u64> = HashMap::new();
                    for (k, t) in &guard.fencing_tokens {
                        toks.insert(k.clone(), *t);
                    }
                    model.on_grant(&guard.keys, &toks);
                    // brief hold
                    if rng.below(4) == 0 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    model.on_release(&guard.keys);
                    client.release(&guard).await.expect("composite release");
                } else {
                    let key = keys[rng.below(keys.len())].clone();
                    let guard = match tokio::time::timeout(
                        Duration::from_secs(30),
                        client.acquire(&key, Duration::from_millis(60_000)),
                    )
                    .await
                    {
                        Ok(Ok(g)) => g,
                        Ok(Err(e)) => panic!("task {task_id} acquire({key}) failed: {e:?}"),
                        Err(_) => {
                            panic!("task {task_id} acquire({key}) timed out (possible deadlock)")
                        }
                    };
                    let mut toks: HashMap<String, u64> = HashMap::new();
                    toks.insert(key.clone(), guard.fencing_token.unwrap_or(0));
                    model.on_grant(&guard.keys, &toks);
                    if rng.below(4) == 0 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    model.on_release(&guard.keys);
                    client.release(&guard).await.expect("release");
                }
            }
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        h.await
            .unwrap_or_else(|e| panic!("task {i} panicked: {e:?}"));
    }

    assert_eq!(
        model.violations.load(Ordering::Relaxed),
        0,
        "stress run recorded mutual-exclusion / fencing violations"
    );
    assert!(
        model.all_free(),
        "some keys still marked held after all tasks finished"
    );
}
