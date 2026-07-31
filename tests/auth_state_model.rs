//! Exhaustive bounded model for refresh, revocation, and signing-key rotation.
//!
//! This is deliberately dependency-free so the model checker runs in the
//! ordinary `cargo test --all-targets --locked` CI gate.

use std::collections::{HashSet, VecDeque};

const MAX_DEPTH: usize = 8;
const SESSION_DEADLINE: u8 = 4;
const ACCESS_TTL: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Key {
    K0,
    K1,
}

impl Key {
    const fn mask(self) -> u8 {
        match self {
            Self::K0 => 0b01,
            Self::K1 => 0b10,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::K0 => Self::K1,
            Self::K1 => Self::K0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AccessToken {
    key: Key,
    issued_at: u8,
    expires_at: u8,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Model {
    now: u8,
    revoked: bool,
    refresh_generation: u8,
    consumed_refreshes: u8,
    active_key: Key,
    trusted_keys: u8,
    tokens: Vec<AccessToken>,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    AdvanceTime,
    Refresh(u8),
    Revoke,
    RotateKey,
    Retire(Key),
}

impl Model {
    fn initial() -> Self {
        Self {
            now: 0,
            revoked: false,
            refresh_generation: 0,
            consumed_refreshes: 0,
            active_key: Key::K0,
            trusted_keys: Key::K0.mask(),
            tokens: vec![AccessToken {
                key: Key::K0,
                issued_at: 0,
                expires_at: ACCESS_TTL,
            }],
        }
    }

    fn refresh_is_live(&self) -> bool {
        !self.revoked
            && self.now < SESSION_DEADLINE
            && self.refresh_generation < 4
            && self.consumed_refreshes & (1 << self.refresh_generation) == 0
    }

    fn accepts(&self, token: AccessToken) -> bool {
        !self.revoked
            && self.now >= token.issued_at
            && self.now < token.expires_at
            && self.now < SESSION_DEADLINE
            && self.trusted_keys & token.key.mask() != 0
    }

    fn apply(&self, action: Action) -> Option<Self> {
        let mut next = self.clone();
        match action {
            Action::AdvanceTime if self.now < SESSION_DEADLINE + 1 => next.now += 1,
            Action::Refresh(generation)
                if generation == self.refresh_generation && self.refresh_is_live() =>
            {
                next.consumed_refreshes |= 1 << self.refresh_generation;
                next.refresh_generation += 1;
                next.tokens.push(AccessToken {
                    key: self.active_key,
                    issued_at: self.now,
                    expires_at: (self.now + ACCESS_TTL).min(SESSION_DEADLINE),
                });
                next.tokens
                    .sort_by_key(|token| (token.issued_at, token.expires_at, token.key.mask()));
                next.tokens.dedup();
            }
            Action::Revoke if !self.revoked => next.revoked = true,
            Action::RotateKey => {
                next.active_key = self.active_key.other();
                next.trusted_keys |= next.active_key.mask();
            }
            Action::Retire(key)
                if key != self.active_key && self.trusted_keys & key.mask() != 0 =>
            {
                next.trusted_keys &= !key.mask();
            }
            _ => return None,
        }
        Some(next)
    }

    fn assert_invariants(&self, trace: &[Action]) {
        assert_ne!(
            self.trusted_keys & self.active_key.mask(),
            0,
            "active signing key is not trusted; trace={trace:?}"
        );
        assert_eq!(
            self.consumed_refreshes & (1 << self.refresh_generation),
            0,
            "current refresh token was already consumed; trace={trace:?}"
        );
        for generation in 0..self.refresh_generation {
            assert_ne!(
                self.consumed_refreshes & (1 << generation),
                0,
                "a prior refresh generation was not consumed; trace={trace:?}"
            );
        }
        for token in &self.tokens {
            assert!(
                token.expires_at <= SESSION_DEADLINE,
                "token outlives its session; token={token:?}; trace={trace:?}"
            );
            if self.revoked || self.now >= SESSION_DEADLINE {
                assert!(
                    !self.accepts(*token),
                    "revoked/expired session accepted a token; token={token:?}; trace={trace:?}"
                );
            }
            if self.trusted_keys & token.key.mask() == 0 {
                assert!(
                    !self.accepts(*token),
                    "retired key accepted a token; token={token:?}; trace={trace:?}"
                );
            }
        }

        if self.refresh_is_live() {
            let refreshed = self
                .apply(Action::Refresh(self.refresh_generation))
                .expect("a legitimate refresh must make progress");
            let issued = AccessToken {
                key: self.active_key,
                issued_at: self.now,
                expires_at: (self.now + ACCESS_TTL).min(SESSION_DEADLINE),
            };
            assert!(refreshed.tokens.contains(&issued));
            assert!(refreshed.accepts(issued));
        }
    }
}

#[test]
fn bounded_protocol_model_preserves_safety_and_liveness() {
    let initial = Model::initial();
    let mut seen = HashSet::from([initial.clone()]);
    let mut queue = VecDeque::from([(initial, Vec::<Action>::new())]);
    let mut observed_revocation = false;
    let mut observed_rotation = false;
    let mut observed_retirement = false;

    while let Some((state, trace)) = queue.pop_front() {
        state.assert_invariants(&trace);
        observed_revocation |= state.revoked;
        observed_rotation |= state.active_key == Key::K1;
        observed_retirement |= state.trusted_keys.count_ones() == 1 && state.active_key == Key::K1;

        if trace.len() == MAX_DEPTH {
            continue;
        }

        for action in [
            Action::AdvanceTime,
            Action::Refresh(0),
            Action::Refresh(1),
            Action::Refresh(2),
            Action::Refresh(3),
            Action::Revoke,
            Action::RotateKey,
            Action::Retire(Key::K0),
            Action::Retire(Key::K1),
        ] {
            let Some(next) = state.apply(action) else {
                continue;
            };
            if seen.insert(next.clone()) {
                let mut next_trace = trace.clone();
                next_trace.push(action);
                queue.push_back((next, next_trace));
            }
        }
    }

    assert!(
        seen.len() >= 100,
        "model unexpectedly explored only {} states",
        seen.len()
    );
    assert!(observed_revocation);
    assert!(observed_rotation);
    assert!(observed_retirement);
}

#[test]
fn replay_revocation_and_key_retirement_are_explicit_regressions() {
    let initial = Model::initial();
    let first_token = initial.tokens[0];

    let refreshed = initial.apply(Action::Refresh(0)).unwrap();
    assert!(
        refreshed.apply(Action::Refresh(0)).is_none(),
        "the consumed refresh generation must reject replay"
    );
    assert_ne!(refreshed.consumed_refreshes & 1, 0);

    let revoked = refreshed.apply(Action::Revoke).unwrap();
    assert!(revoked.tokens.iter().all(|token| !revoked.accepts(*token)));

    let rotated = initial.apply(Action::RotateKey).unwrap();
    assert!(
        rotated.accepts(first_token),
        "old access token must survive the key-overlap window"
    );
    let retired = rotated.apply(Action::Retire(Key::K0)).unwrap();
    assert!(!retired.accepts(first_token));
}
