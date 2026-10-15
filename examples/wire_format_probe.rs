//! Print the canonical JSON wire format to stdout. Used by the cross-runtime
//! client work as a sanity check that everyone (TS / Go / Dart / Gleam /
//! Rust) is targeting the same on-the-wire shape.

use dd_rust_network_mutex::protocol::{Request, Response};

fn main() {
    let req = Request::Lock {
        uuid: "u1".into(),
        key: Some("k".into()),
        keys: None,
        pid: Some(1),
        ttl: Some(1000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(true),
    };
    println!("REQ_LOCK: {}", serde_json::to_string(&req).unwrap());

    let resp = Response::Lock {
        uuid: "u1".into(),
        key: "k".into(),
        acquired: true,
        lock_request_count: 0,
        lock_uuid: Some("L1".into()),
        fencing_token: Some(42),
        readers_count: Some(0),
        error: None,
    };
    println!("RESP_LOCK: {}", serde_json::to_string(&resp).unwrap());

    let req2 = Request::RegisterRead {
        uuid: "u2".into(),
        key: "k".into(),
    };
    println!("REQ_REGREAD: {}", serde_json::to_string(&req2).unwrap());

    let comp = Response::CompositeLock {
        uuid: "u3".into(),
        keys: vec!["a".into(), "b".into()],
        acquired: true,
        lock_uuid: Some("L2".into()),
        fencing_tokens: Some(std::collections::BTreeMap::from([
            ("a".into(), 1u64),
            ("b".into(), 2u64),
        ])),
        error: None,
    };
    println!("RESP_COMPOSITE: {}", serde_json::to_string(&comp).unwrap());
}
