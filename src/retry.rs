use std::time::{Duration, SystemTime};

const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    pub retryable: bool,
    pub retry_after: Option<Duration>,
}

impl RetryDecision {
    pub const fn permanent() -> Self {
        Self {
            retryable: false,
            retry_after: None,
        }
    }

    pub fn retryable(retry_after: Option<Duration>) -> Self {
        Self {
            retryable: true,
            retry_after: retry_after.map(|duration| duration.min(MAX_RETRY_AFTER)),
        }
    }
}

/// Parse either a delta-seconds or HTTP-date `Retry-After` value.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    let duration = retry_at.duration_since(now).unwrap_or_default();
    Some(duration.min(MAX_RETRY_AFTER))
}

/// Provider-neutral baseline classification. Adapters may refine individual
/// provider error codes, but must not turn a permanent target failure into an
/// unbounded retry loop.
pub fn classify_http_status(status: u16, retry_after: Option<Duration>) -> RetryDecision {
    match status {
        408 | 425 | 429 | 500..=599 => RetryDecision::retryable(retry_after),
        _ => RetryDecision::permanent(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_delta_seconds_and_caps_extreme_values() {
        assert_eq!(
            parse_retry_after("120", SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after("999999", SystemTime::UNIX_EPOCH),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn parses_http_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let value = httpdate::fmt_http_date(now + Duration::from_secs(45));
        assert_eq!(
            parse_retry_after(&value, now),
            Some(Duration::from_secs(45))
        );
    }

    #[test]
    fn classifies_transient_and_permanent_statuses() {
        assert!(classify_http_status(429, None).retryable);
        assert!(classify_http_status(503, Some(Duration::from_secs(3))).retryable);
        assert!(!classify_http_status(400, None).retryable);
        assert!(!classify_http_status(401, None).retryable);
        assert!(!classify_http_status(404, None).retryable);
    }
}
