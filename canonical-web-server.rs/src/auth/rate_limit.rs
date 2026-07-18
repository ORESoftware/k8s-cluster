use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Bounded, per-process protection for the password endpoint. The edge still
/// enforces the authoritative per-IP limit: application processes cannot
/// safely infer a browser IP behind an untrusted proxy header.
#[derive(Clone)]
pub struct LoginRateLimiter {
    state: Arc<Mutex<State>>,
    max_attempts: u32,
    global_max_attempts: u32,
    window: Duration,
    max_keys: usize,
}

struct State {
    attempts: HashMap<String, AttemptWindow>,
    global: AttemptWindow,
}

struct AttemptWindow {
    opened_at: Instant,
    attempts: u32,
}

impl LoginRateLimiter {
    pub fn new(
        max_attempts: u32,
        global_max_attempts: u32,
        window: Duration,
        max_keys: usize,
    ) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(State {
                attempts: HashMap::new(),
                global: AttemptWindow {
                    opened_at: now,
                    attempts: 0,
                },
            })),
            max_attempts,
            global_max_attempts,
            window,
            max_keys,
        }
    }

    /// Records an attempt before contacting Supabase. The key is a SHA-256
    /// digest of a normalized email so raw identifiers never enter metrics or
    /// logs through this component.
    pub async fn check(&self, email: &str) -> Result<(), u64> {
        let now = Instant::now();
        let key = account_key(email);
        let mut state = self.state.lock().await;
        if now.duration_since(state.global.opened_at) >= self.window {
            state.global = AttemptWindow {
                opened_at: now,
                attempts: 0,
            };
        }
        state.global.attempts = state.global.attempts.saturating_add(1);
        if state.global.attempts > self.global_max_attempts {
            return Err(retry_after(now, state.global.opened_at, self.window));
        }

        state
            .attempts
            .retain(|_, entry| now.duration_since(entry.opened_at) < self.window);

        if !state.attempts.contains_key(&key) && state.attempts.len() >= self.max_keys {
            let oldest = state
                .attempts
                .iter()
                .min_by_key(|(_, entry)| entry.opened_at)
                .map(|(_, entry)| entry.opened_at)
                .unwrap_or(now);
            // Never evict a live account window: key churn must not erase a
            // targeted account's block. Capacity exhaustion fails closed until
            // the oldest window expires.
            return Err(retry_after(now, oldest, self.window));
        }

        let entry = state.attempts.entry(key).or_insert(AttemptWindow {
            opened_at: now,
            attempts: 0,
        });
        entry.attempts = entry.attempts.saturating_add(1);
        if entry.attempts > self.max_attempts {
            return Err(retry_after(now, entry.opened_at, self.window));
        }
        Ok(())
    }
}

fn retry_after(now: Instant, opened_at: Instant, window: Duration) -> u64 {
    let remaining = window.saturating_sub(now.duration_since(opened_at));
    remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
        .max(1)
}

fn account_key(email: &str) -> String {
    let normalized = email.trim().to_ascii_lowercase();
    URL_SAFE_NO_PAD.encode(Sha256::digest(normalized.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::LoginRateLimiter;
    use std::time::Duration;

    #[tokio::test]
    async fn normalizes_accounts_and_returns_a_retry_after() {
        let limiter = LoginRateLimiter::new(2, 100, Duration::from_secs(60), 4);
        assert!(limiter.check("User@Example.com").await.is_ok());
        assert!(limiter.check(" user@example.com ").await.is_ok());
        assert_eq!(limiter.check("USER@example.com").await, Err(60));
    }

    #[tokio::test]
    async fn key_churn_cannot_evict_a_blocked_account() {
        let limiter = LoginRateLimiter::new(1, 100, Duration::from_secs(60), 2);
        assert!(limiter.check("a@example.com").await.is_ok());
        assert_eq!(limiter.check("a@example.com").await, Err(60));
        assert!(limiter.check("b@example.com").await.is_ok());
        assert_eq!(limiter.check("c@example.com").await, Err(60));
        assert_eq!(limiter.check("a@example.com").await, Err(60));
    }

    #[tokio::test]
    async fn global_budget_limits_account_spraying() {
        let limiter = LoginRateLimiter::new(10, 2, Duration::from_secs(60), 10);
        assert!(limiter.check("a@example.com").await.is_ok());
        assert!(limiter.check("b@example.com").await.is_ok());
        assert_eq!(limiter.check("c@example.com").await, Err(60));
    }
}
