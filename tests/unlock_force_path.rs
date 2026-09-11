//! Regression tests for the broker's `handle_unlock` force-path.
//!
//! The Node port shipped a parallel set of fixes in PR #131 (live-mutex):
//! the original `unlock(force=true, lock_uuid=...)` branch had three latent
//! bugs that surface as a "phantom unlock" — a reply of `unlocked: true`
//! while the lock object still listed peer holders, or unrelated peers
//! losing their slots because the request happened to set `force: true`.
//! This file pins the same invariants on the Rust side.
//!
//!   r1  Phantom semaphore unlock with wrong `lock_uuid` + `force: true`:
//!       the broker must NOT wipe peer holders just because someone set
//!       `force: true` and passed a `lock_uuid` that doesn't match
//!       anyone. It should reply `unlocked: false`.
//!
//!   r2  Phantom exclusive unlock with wrong `lock_uuid` + `force: true`:
//!       same shape, on `max=1`. The legitimate holder must survive an
//!       attacker (or operator typo) sending a forceful unlock with a
//!       wrong uuid.
//!
//!   r3  Phantom unlock on an empty key with `force: true`: a forceful
//!       unlock against a key that has zero holders must NOT report
//!       `unlocked: true`. The caller didn't free anything.
//!
//!   r4  Operator wipe-all (`force: true` + `lock_uuid: None`) MUST
//!       clean up every wiped holder's `held_lock_uuids` entry on the
//!       owning client. Otherwise a peer client's bookkeeping says it
//!       still holds locks the broker has already released.
//!
//!   r5  `force: true` + valid `lock_uuid` releases exactly that one
//!       holder on a semaphore (`max>1`) — peer holders survive.

use dd_rust_network_mutex::{
    broker::{Broker, BrokerConfig},
    protocol::{Request, Response},
};
use tokio::sync::mpsc::UnboundedReceiver;

fn drain(rx: &mut UnboundedReceiver<Response>) -> Vec<Response> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

fn lock_uuid_of(msgs: &[Response]) -> Option<String> {
    msgs.iter().find_map(|m| match m {
        Response::Lock {
            acquired: true,
            lock_uuid,
            ..
        } => lock_uuid.clone(),
        _ => None,
    })
}

#[test]
fn r1_force_unlock_with_wrong_uuid_does_not_wipe_semaphore_peers() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();
    let (c, mut c_rx) = broker.register_client(); // attacker

    // A and B both hold a semaphore slot (max=2).
    for (cid, rx, uuid) in [(a, &mut a_rx, "ra"), (b, &mut b_rx, "rb")] {
        broker.handle_request(
            cid,
            Request::Lock {
                uuid: uuid.into(),
                key: Some("r1-sem".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(2),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let msgs = drain(rx);
        assert!(
            msgs.iter()
                .any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "{uuid} should be granted; got {msgs:?}"
        );
    }
    assert_eq!(broker.metrics().holders, 2);

    // C — attacker — sends force=true with a wrong lock_uuid.
    broker.handle_request(
        c,
        Request::Unlock {
            uuid: "uc".into(),
            key: Some("r1-sem".into()),
            keys: None,
            lock_uuid: Some("WRONG-UUID".into()),
            force: true,
        },
    );
    let c_msgs = drain(&mut c_rx);
    let unlocked = c_msgs.iter().find_map(|m| match m {
        Response::Unlock { unlocked, .. } => Some(*unlocked),
        _ => None,
    });
    assert_eq!(
        unlocked,
        Some(false),
        "force-unlock with wrong lock_uuid must report unlocked: false; got {c_msgs:?}"
    );
    let snapshot = broker.metrics();
    assert_eq!(
        snapshot.holders, 2,
        "force-unlock with wrong lock_uuid must NOT wipe peer semaphore holders; got holders={}",
        snapshot.holders
    );
}

#[test]
fn r2_force_unlock_with_wrong_uuid_does_not_evict_exclusive_holder() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (attacker, mut attacker_rx) = broker.register_client();

    broker.handle_request(
        a,
        Request::Lock {
            uuid: "ra".into(),
            key: Some("r2-excl".into()),
            keys: None,
            pid: None,
            ttl: Some(60_000),
            max: None,
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
            wait: None,
        },
    );
    let _ = drain(&mut a_rx);
    assert_eq!(broker.metrics().holders, 1);

    broker.handle_request(
        attacker,
        Request::Unlock {
            uuid: "uatk".into(),
            key: Some("r2-excl".into()),
            keys: None,
            lock_uuid: Some("WRONG-UUID".into()),
            force: true,
        },
    );
    let msgs = drain(&mut attacker_rx);
    let unlocked = msgs.iter().find_map(|m| match m {
        Response::Unlock { unlocked, .. } => Some(*unlocked),
        _ => None,
    });
    assert_eq!(
        unlocked,
        Some(false),
        "force-unlock with wrong lock_uuid must NOT report unlocked:true on exclusive lock; got {msgs:?}"
    );
    assert_eq!(
        broker.metrics().holders,
        1,
        "force-unlock with wrong lock_uuid must NOT evict the legitimate exclusive holder"
    );
}

#[test]
fn r3_force_unlock_on_empty_key_reports_false() {
    let broker = Broker::new(BrokerConfig::default());
    let (c, mut c_rx) = broker.register_client();

    // Materialise the LockState (so `state.locks.get_mut(key)` returns
    // Some) but with zero holders — the force-path used to wipe the
    // empty maps and still report `unlocked: true`.
    let (a, mut a_rx) = broker.register_client();
    broker.handle_request(
        a,
        Request::Lock {
            uuid: "ra".into(),
            key: Some("r3-empty".into()),
            keys: None,
            pid: None,
            ttl: Some(60_000),
            max: None,
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
            wait: None,
        },
    );
    let lock_uuid_a = lock_uuid_of(&drain(&mut a_rx)).unwrap();
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "ua".into(),
            key: Some("r3-empty".into()),
            keys: None,
            lock_uuid: Some(lock_uuid_a),
            force: false,
        },
    );
    let _ = drain(&mut a_rx);
    assert_eq!(broker.metrics().holders, 0);

    // Now C force-unlocks a key with zero holders.
    broker.handle_request(
        c,
        Request::Unlock {
            uuid: "uc".into(),
            key: Some("r3-empty".into()),
            keys: None,
            lock_uuid: None,
            force: true,
        },
    );
    let msgs = drain(&mut c_rx);
    let unlocked = msgs.iter().find_map(|m| match m {
        Response::Unlock { unlocked, .. } => Some(*unlocked),
        _ => None,
    });
    assert_eq!(
        unlocked,
        Some(false),
        "force-unlock on a key with zero holders must report unlocked: false; got {msgs:?}"
    );
}

#[test]
fn r4_operator_wipe_cleans_up_held_lock_uuids_on_peer_clients() {
    // When `force: true` + `lock_uuid: None` runs (the operator-break
    // semantic the existing test suite documents), the broker must
    // remove every wiped lock_uuid from its OWNING client's
    // held_lock_uuids — not just the calling client. Otherwise the
    // peer client's `drop_client` later attempts to release something
    // the broker has already evicted, and an `unlock` from the peer
    // gets `unlocked: false` even though it really did hold it.
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client(); // peer holder
    let (operator, mut op_rx) = broker.register_client();

    // A and B both hold a semaphore slot.
    for (cid, rx, uuid) in [(a, &mut a_rx, "ra"), (b, &mut b_rx, "rb")] {
        broker.handle_request(
            cid,
            Request::Lock {
                uuid: uuid.into(),
                key: Some("r4-sem".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(2),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(rx);
    }
    let lock_uuid_b = {
        broker.handle_request(
            b,
            Request::LockInfo {
                uuid: "info".into(),
                key: "r4-sem".into(),
            },
        );
        let msgs = drain(&mut b_rx);
        msgs.iter()
            .find_map(|m| match m {
                Response::LockInfo {
                    lockholder_uuids, ..
                } => lockholder_uuids.first().cloned(),
                _ => None,
            })
            .expect("LockInfo should list at least one holder")
    };

    // Operator wipes the key.
    broker.handle_request(
        operator,
        Request::Unlock {
            uuid: "uop".into(),
            key: Some("r4-sem".into()),
            keys: None,
            lock_uuid: None,
            force: true,
        },
    );
    let _ = drain(&mut op_rx);
    assert_eq!(broker.metrics().holders, 0);

    // B now tries to release with its (still held in B's mind)
    // lock_uuid. Without the held_lock_uuids cleanup, the broker would
    // still see this entry on B's ClientHandle and the unlock would
    // route normally. With the cleanup, B's bookkeeping is consistent
    // with reality: the broker reports unlocked: false because the
    // hold has already been wiped.
    broker.handle_request(
        b,
        Request::Unlock {
            uuid: "ub".into(),
            key: Some("r4-sem".into()),
            keys: None,
            lock_uuid: Some(lock_uuid_b),
            force: false,
        },
    );
    let msgs = drain(&mut b_rx);
    let unlocked = msgs.iter().find_map(|m| match m {
        Response::Unlock { unlocked, .. } => Some(*unlocked),
        _ => None,
    });
    assert_eq!(
        unlocked,
        Some(false),
        "after operator wipe, B's late unlock must report unlocked: false; got {msgs:?}"
    );
}

#[test]
fn r5_force_unlock_with_valid_uuid_releases_only_that_semaphore_slot() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    for (cid, rx, uuid) in [(a, &mut a_rx, "ra"), (b, &mut b_rx, "rb")] {
        broker.handle_request(
            cid,
            Request::Lock {
                uuid: uuid.into(),
                key: Some("r5-sem".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(2),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(rx);
    }
    assert_eq!(broker.metrics().holders, 2);

    let lock_uuid_a = {
        broker.handle_request(
            a,
            Request::LockInfo {
                uuid: "info".into(),
                key: "r5-sem".into(),
            },
        );
        let msgs = drain(&mut a_rx);
        let holders = msgs
            .iter()
            .find_map(|m| match m {
                Response::LockInfo {
                    lockholder_uuids, ..
                } => Some(lockholder_uuids.clone()),
                _ => None,
            })
            .unwrap();
        // Whatever uuid is first; we just need ONE valid one. The
        // broker will reject if it doesn't match a current holder.
        holders[0].clone()
    };

    // A force-releases its own slot via lock_uuid.
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "ua".into(),
            key: Some("r5-sem".into()),
            keys: None,
            lock_uuid: Some(lock_uuid_a.clone()),
            force: true,
        },
    );
    let msgs = drain(&mut a_rx);
    let unlocked = msgs.iter().find_map(|m| match m {
        Response::Unlock { unlocked, .. } => Some(*unlocked),
        _ => None,
    });
    assert_eq!(
        unlocked,
        Some(true),
        "force-unlock with valid uuid should report true; got {msgs:?}"
    );

    // The peer slot must survive.
    let snapshot = broker.metrics();
    assert_eq!(
        snapshot.holders, 1,
        "force-unlock with valid lock_uuid must release exactly one slot, leaving the peer in place"
    );
}
