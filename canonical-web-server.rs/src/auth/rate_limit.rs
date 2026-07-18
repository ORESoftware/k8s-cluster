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
    window: Duration,
    max_keys: usize,
}

struct State {
    attempts: HashMap<String, AttemptWindow>,
}

struct AttemptWindow {
    opened_at: Instant,
    attempts: u32,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, window: Duration, max_keys: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                attempts: HashMap::new(),
            })),
            max_attempts,
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
        state
            .attempts
            .retain(|_, entry| now.duration_since(entry.opened_at) < self.window);

        if !state.attempts.contains_key(&key) && state.attempts.len() >= self.max_keys {
            if let Some(oldest) = state
                .attempts
                .iter()
                .min_by_key(|(_, entry)| entry.opened_at)
                .map(|(key, _)| key.clone())
            {
                state.attempts.remove(&oldest);
            }
        }

        let entry = state.attempts.entry(key).or_insert(AttemptWindow {
            opened_at: now,
            attempts: 0,
        });
        entry.attempts = entry.attempts.saturating_add(1);
        if entry.attempts > self.max_attempts {
            let remaining = self
                .window
                .saturating_sub(now.duration_since(entry.opened_at));
            let retry_after = remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0))
                .max(1);
            return Err(retry_after);
        }
        Ok(())
    }
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
        let limiter = LoginRateLimiter::new(2, Duration::from_secs(60), 4);
        assert!(limiter.check("User@Example.com").await.is_ok());
        assert!(limiter.check(" user@example.com ").await.is_ok());
        assert_eq!(limiter.check("USER@example.com").await, Err(60));
    }
}
