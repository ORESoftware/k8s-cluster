//! Per-key FIFO queue of pending lock requests.
//!
//! This used to be a hand-rolled arena-backed doubly-linked list living inside
//! this crate. That data structure has been extracted, hardened (property +
//! fuzz tested against a `VecDeque` oracle), and published as the standalone
//! [`linked-queue`](https://github.com/ORESoftware/linked-queue.rs) crate. This
//! module is now a thin adapter over `linked_queue::LinkedQueue` that preserves
//! the exact API the broker relies on:
//!
//! * `push_back` / `push_front` return `bool` (`true` = inserted, `false` =
//!   no-op because the key was already queued) — matching the upstream
//!   `notify.contains` "duplicate request is idempotent" semantics. They map
//!   onto the crate's strict `try_push_*` methods, which reject duplicates
//!   without mutating the queue.
//! * `remove(key)` yanks a request out of the middle in O(1) when a client
//!   times out / disconnects (`cleanupConnection` in upstream `broker.ts`).
//! * `pop_front` / `front` / `get` / `contains` / `len` / `is_empty` / `iter`
//!   behave exactly as before.
//!
//! Keeping the broker-facing surface identical means `broker.rs` did not change
//! at all; only the implementation moved behind a dependency. The
//! `routine_id!` instrumentation is preserved so the OTel/log story is
//! unchanged.

use std::hash::Hash;

/// Borrowing head-to-tail iterator, re-exported from the `linked-queue` crate.
pub use linked_queue::Iter;
use linked_queue::LinkedQueue as Inner;

/// FIFO queue keyed by `K`. Each `K` is unique within the queue; pushing the
/// same `K` again is a no-op (matching upstream `notify.contains` semantics).
#[derive(Debug)]
pub struct LinkedQueue<K: Eq + Hash + Clone, V> {
    inner: Inner<K, V>,
}

impl<K: Eq + Hash + Clone, V> Default for LinkedQueue<K, V> {
    fn default() -> Self {
        crate::routine_id!("ddl-routine-jXkFFGyoB-wXxOy7FC");
        Self::new()
    }
}

impl<K: Eq + Hash + Clone, V> LinkedQueue<K, V> {
    pub fn new() -> Self {
        crate::routine_id!("ddl-routine-u7zVp19R7LDL_wL2Au");
        Self {
            inner: Inner::new(),
        }
    }

    pub fn len(&self) -> usize {
        crate::routine_id!("ddl-routine-yOpS-zAjNsvBXBLSOt");
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        crate::routine_id!("ddl-routine-2R0CreVvd2mDaVd-G-");
        self.inner.is_empty()
    }

    pub fn contains(&self, key: &K) -> bool {
        crate::routine_id!("ddl-routine-AER48-JGALNyb5R_xP");
        self.inner.contains(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        crate::routine_id!("ddl-routine-1TZ61XitZ7fwwXz0Mm");
        self.inner.get(key)
    }

    /// Append to the tail (FIFO). No-op returning `false` if `key` is already
    /// present.
    pub fn push_back(&mut self, key: K, value: V) -> bool {
        crate::routine_id!("ddl-routine-_luiuBgLux_GbtPGIo");
        self.inner.try_push_back(key, value).is_ok()
    }

    /// Push to the head. Used for force/retry insertions that should jump the
    /// queue. No-op returning `false` if `key` is already present.
    pub fn push_front(&mut self, key: K, value: V) -> bool {
        crate::routine_id!("ddl-routine-JWMK_pZuMQVZLjDpAI");
        self.inner.try_push_front(key, value).is_ok()
    }

    /// Pop the head element (FIFO dequeue). Returns `(key, value)`.
    pub fn pop_front(&mut self) -> Option<(K, V)> {
        crate::routine_id!("ddl-routine-z5ECAdo119VbDKreQJ");
        self.inner.pop_front()
    }

    /// Peek at the head without removing it.
    pub fn front(&self) -> Option<(&K, &V)> {
        crate::routine_id!("ddl-routine-HdN0Nd-ERLc5HFSzKW");
        self.inner.front()
    }

    /// Remove an element by `key` in O(1). Returns the value if present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        crate::routine_id!("ddl-routine-2kna_P2-hQO1bUmmZD");
        self.inner.remove(key)
    }

    /// Iterate from head to tail without consuming.
    pub fn iter(&self) -> Iter<'_, K, V> {
        crate::routine_id!("ddl-routine-YL8_XRwkTGmdu0Eo2b");
        self.inner.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order() {
        crate::routine_id!("ddl-routine--er5KgxaOJcyZbEpMe");
        let mut q: LinkedQueue<&'static str, u32> = LinkedQueue::new();
        assert!(q.push_back("a", 1));
        assert!(q.push_back("b", 2));
        assert!(q.push_back("c", 3));
        assert_eq!(q.len(), 3);
        assert_eq!(q.pop_front(), Some(("a", 1)));
        assert_eq!(q.pop_front(), Some(("b", 2)));
        assert_eq!(q.pop_front(), Some(("c", 3)));
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn duplicate_key_is_noop() {
        crate::routine_id!("ddl-routine-WR0Bc-O2-i_NWp3JYr");
        let mut q: LinkedQueue<&'static str, u32> = LinkedQueue::new();
        assert!(q.push_back("a", 1));
        assert!(!q.push_back("a", 99));
        assert_eq!(q.len(), 1);
        assert_eq!(q.get(&"a"), Some(&1));
    }

    #[test]
    fn remove_from_middle_o1() {
        crate::routine_id!("ddl-routine-tIny8nlxpTWQXqG772");
        let mut q: LinkedQueue<&'static str, u32> = LinkedQueue::new();
        q.push_back("a", 1);
        q.push_back("b", 2);
        q.push_back("c", 3);
        q.push_back("d", 4);
        assert_eq!(q.remove(&"b"), Some(2));
        assert_eq!(q.remove(&"d"), Some(4));
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop_front(), Some(("a", 1)));
        assert_eq!(q.pop_front(), Some(("c", 3)));
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn remove_head_and_tail() {
        crate::routine_id!("ddl-routine-32PjlKmz6LNtCvFQ6T");
        let mut q: LinkedQueue<&'static str, u32> = LinkedQueue::new();
        q.push_back("a", 1);
        q.push_back("b", 2);
        q.push_back("c", 3);
        assert_eq!(q.remove(&"a"), Some(1));
        assert_eq!(q.remove(&"c"), Some(3));
        assert_eq!(q.len(), 1);
        assert_eq!(q.pop_front(), Some(("b", 2)));
    }

    #[test]
    fn push_front_jumps_queue() {
        crate::routine_id!("ddl-routine-HsNPxbgCEqaBQ7LUFq");
        let mut q: LinkedQueue<&'static str, u32> = LinkedQueue::new();
        q.push_back("a", 1);
        q.push_back("b", 2);
        q.push_front("c", 3);
        assert_eq!(q.pop_front(), Some(("c", 3)));
        assert_eq!(q.pop_front(), Some(("a", 1)));
        assert_eq!(q.pop_front(), Some(("b", 2)));
    }

    /// Smoke test that the adapter still agrees with a `VecDeque` model for the
    /// broker-facing operations. (Exhaustive property/fuzz testing lives in the
    /// `linked-queue` crate itself.)
    #[test]
    fn oracle_smoke() {
        crate::routine_id!("ddl-routine-fuzz-vecdeque-_4M");
        use std::collections::VecDeque;

        let mut q: LinkedQueue<u32, u32> = LinkedQueue::new();
        let mut model: VecDeque<(u32, u32)> = VecDeque::new();
        let mut state: u64 = 0xdeadbeefcafef00d;
        let mut rng = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };

        for step in 0..10_000u32 {
            let op = rng() % 5;
            let k = (rng() % 64) as u32;
            let v = (rng() % 1024) as u32;
            match op {
                0 => {
                    let inserted = q.push_back(k, v);
                    if !model.iter().any(|(mk, _)| *mk == k) {
                        assert!(inserted, "step {step}: push_back should insert");
                        model.push_back((k, v));
                    } else {
                        assert!(!inserted, "step {step}: push_back should be idempotent");
                    }
                }
                1 => {
                    let inserted = q.push_front(k, v);
                    if !model.iter().any(|(mk, _)| *mk == k) {
                        assert!(inserted, "step {step}: push_front should insert");
                        model.push_front((k, v));
                    } else {
                        assert!(!inserted, "step {step}: push_front should be idempotent");
                    }
                }
                2 => assert_eq!(q.pop_front(), model.pop_front(), "step {step}: pop_front"),
                3 => {
                    let removed = q.remove(&k);
                    let model_removed = model
                        .iter()
                        .position(|(mk, _)| *mk == k)
                        .map(|p| model.remove(p).unwrap().1);
                    assert_eq!(removed, model_removed, "step {step}: remove");
                }
                _ => {
                    assert_eq!(q.len(), model.len(), "step {step}: len");
                    let q_order: Vec<(u32, u32)> = q.iter().map(|(k, v)| (*k, *v)).collect();
                    let m_order: Vec<(u32, u32)> = model.iter().copied().collect();
                    assert_eq!(q_order, m_order, "step {step}: iter order");
                    assert_eq!(
                        q.front().map(|(k, v)| (*k, *v)),
                        model.front().copied(),
                        "step {step}: front"
                    );
                }
            }
        }
    }
}
