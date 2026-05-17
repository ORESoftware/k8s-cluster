//! Webhook delivery-ID dedupe.
//!
//! GitHub retries deliveries (up to 8 attempts with exponential backoff)
//! when the receiver doesn't return 2xx in time. Each retry carries the
//! same `X-GitHub-Delivery` value. Without dedupe a slow analysis would
//! get retried while still running, and end up running twice for the same
//! PR head commit.
//!
//! We keep a bounded, time-windowed set of recently-seen delivery IDs.
//! When `record()` returns `false` the caller should respond `202
//! Accepted` with `status: duplicate` instead of dispatching new work.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct DeliveryDedupe {
    seen: VecDeque<(String, Instant)>,
    capacity: usize,
    ttl: Duration,
}

impl DeliveryDedupe {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            seen: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            ttl,
        }
    }

    /// Returns `true` if `delivery_id` was not seen within the TTL window
    /// (and records it now). Returns `false` if it was already seen.
    pub fn record(&mut self, delivery_id: &str) -> bool {
        self.evict_expired();

        if self.seen.iter().any(|(id, _)| id == delivery_id) {
            return false;
        }

        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }
        self.seen
            .push_back((delivery_id.to_string(), Instant::now()));
        true
    }

    fn evict_expired(&mut self) {
        let now = Instant::now();
        while let Some((_, at)) = self.seen.front() {
            if now.duration_since(*at) >= self.ttl {
                self.seen.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sight_returns_true() {
        let mut d = DeliveryDedupe::new(4, Duration::from_secs(60));
        assert!(d.record("a"));
        assert!(d.record("b"));
    }

    #[test]
    fn duplicate_within_ttl_returns_false() {
        let mut d = DeliveryDedupe::new(4, Duration::from_secs(60));
        assert!(d.record("a"));
        assert!(!d.record("a"));
    }

    #[test]
    fn evicts_oldest_when_at_capacity() {
        let mut d = DeliveryDedupe::new(2, Duration::from_secs(60));
        assert!(d.record("a"));
        assert!(d.record("b"));
        assert!(d.record("c"));
        // "a" was evicted, "c" pushed
        assert_eq!(d.len(), 2);
        // re-recording "a" is now a fresh sight
        assert!(d.record("a"));
    }

    #[test]
    fn expired_entries_drop_after_ttl() {
        let mut d = DeliveryDedupe::new(4, Duration::from_millis(10));
        assert!(d.record("a"));
        std::thread::sleep(Duration::from_millis(20));
        // Expired ⇒ same id seen again is treated as fresh.
        assert!(d.record("a"));
    }

    #[test]
    fn empty_id_strings_dedupe_normally() {
        let mut d = DeliveryDedupe::new(4, Duration::from_secs(60));
        assert!(d.record(""));
        assert!(!d.record(""));
    }
}
