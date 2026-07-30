//! Shared helpers for provider sync code.
//!
//! Every provider sync was independently re-implementing
//! `parse_decimal_to_minor`, `constant_time_eq_str`, and a
//! `stablecoin_to_fiat` mapping. That's three places to fix the same
//! bug — extract to one place so the rest of the codebase can converge
//! on a single canonical implementation.

use crate::error::{AppError, AppResult};

/// Parse a fiat decimal string like `"10.00"` / `"-2.5"` / `"1,234.5"`
/// / `"1.5"` into i128 minor units (cents). The caller provides
/// `provider_tag` so error messages identify which provider sent the
/// malformed amount.
///
/// Rules:
///   - leading `+` / `-` accepted
///   - commas and underscores are stripped (locale-tolerant)
///   - fractional digits beyond 2 are truncated, not rounded
///   - missing fractional digits are zero-padded
///   - more than one `.` is rejected
pub fn parse_decimal_to_minor(s: &str, provider_tag: &'static str) -> AppResult<i128> {
    let cleaned = s.trim().replace([',', '_'], "");
    let neg = cleaned.starts_with('-');
    let cleaned = cleaned.trim_start_matches('-').trim_start_matches('+');
    let parts: Vec<&str> = cleaned.split('.').collect();
    let (whole, frac) = match parts.as_slice() {
        [w] => (*w, "00".to_string()),
        [w, f] => {
            // Truncate on CHARACTER boundaries, not bytes: `f[..2]` panics on
            // multi-byte fractional input like "5½". Any non-ASCII-digit chars
            // that survive truncation are caught by the `frac.parse()` below,
            // which turns them into a provider error rather than a panic.
            let frac_len = f.chars().count();
            let f = if frac_len >= 2 {
                f.chars().take(2).collect::<String>()
            } else {
                format!("{f}{}", "0".repeat(2 - frac_len))
            };
            (*w, f)
        }
        _ => {
            return Err(AppError::Provider {
                provider: provider_tag.into(),
                message: format!("malformed decimal {s}"),
            });
        }
    };
    let whole_i: i128 = whole.parse().map_err(|e| AppError::Provider {
        provider: provider_tag.into(),
        message: format!("amount whole {whole}: {e}"),
    })?;
    let frac_i: i128 = frac.parse().map_err(|e| AppError::Provider {
        provider: provider_tag.into(),
        message: format!("amount frac {frac}: {e}"),
    })?;
    // Checked arithmetic: a huge whole part must surface as a provider error,
    // never a panic (debug) or silent wrap (release) in a money code path.
    let v = whole_i
        .checked_mul(100)
        .and_then(|scaled| scaled.checked_add(frac_i))
        .ok_or_else(|| AppError::Provider {
            provider: provider_tag.into(),
            message: format!("amount out of range: {s}"),
        })?;
    Ok(if neg { -v } else { v })
}

/// Constant-time equality on the underlying bytes (string-typed wrapper
/// over a fixed-time comparison). Used for HMAC hex/base64 comparisons
/// where a timing leak would let an attacker probe a signature digit
/// at a time.
pub fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Reject a webhook whose signed timestamp header is outside
/// `tolerance_seconds` of now, in either direction.
///
/// Signing `{timestamp}.{body}` proves an attacker cannot *forge* a fresh
/// timestamp — the HMAC covers it. It does nothing to stop them replaying
/// the original `(timestamp, body, signature)` triple verbatim, forever.
/// Binding the delivery to a time window is what closes that, and it is
/// why Stripe (`t=` + tolerance) and Bridge (`validate_timestamp_freshness`)
/// both do it. Providers that sign a timestamp but never check it are the
/// gap this helper exists to close.
///
/// Accepts the header in seconds or milliseconds. Providers document
/// seconds, but a unit mismatch here would reject every legitimate
/// delivery — an ingestion outage — so the magnitude is sniffed rather
/// than assumed. `1e11` splits them unambiguously: as seconds that is the
/// year 5138, as milliseconds it is 1973.
pub fn verify_timestamp_freshness(
    timestamp_header: &str,
    tolerance_seconds: i64,
    provider_tag: &str,
) -> AppResult<()> {
    let raw: i64 = timestamp_header
        .trim()
        .parse()
        .map_err(|_| AppError::Provider {
            provider: provider_tag.into(),
            message: format!("invalid webhook timestamp header: {timestamp_header:?}"),
        })?;
    let seconds = if raw.abs() >= 100_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    let age = chrono::Utc::now().timestamp().saturating_sub(seconds);
    if age.saturating_abs() > tolerance_seconds.max(0) {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

/// Map a crypto symbol to a fiat-equivalent ISO 4217 currency where
/// the 1:1 peg is reliable enough to post to a ledger.
///
/// Returns `None` for floating-rate crypto (BTC/ETH/SOL/etc.) — the
/// caller should record those to `provider_balance_snapshots` for
/// observability instead of inventing a USD price.
///
/// Be conservative: only include peg-stable tokens with documented
/// 1:1 redemption from their issuer.
pub fn stablecoin_to_fiat(symbol: &str) -> Option<&'static str> {
    match symbol {
        // USD-pegged stablecoins with documented 1:1 redemption.
        "USD" | "USDC" | "USDT" | "PYUSD" | "DAI" => Some("USD"),
        // EUR-pegged.
        "EURC" | "EUR" | "EURI" => Some("EUR"),
        // GBP-pegged (Circle / Mona / Lugh have issued these intermittently).
        "GBP" | "GBPT" => Some("GBP"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_decimals() {
        assert_eq!(parse_decimal_to_minor("1.00", "x").unwrap(), 100);
        assert_eq!(parse_decimal_to_minor("10.5", "x").unwrap(), 1050);
        assert_eq!(parse_decimal_to_minor("0", "x").unwrap(), 0);
        assert_eq!(parse_decimal_to_minor("-3.14", "x").unwrap(), -314);
        assert_eq!(parse_decimal_to_minor("+9.99", "x").unwrap(), 999);
        assert_eq!(parse_decimal_to_minor("1,234.50", "x").unwrap(), 123_450);
        assert_eq!(parse_decimal_to_minor("1_000.00", "x").unwrap(), 100_000);
    }

    #[test]
    fn truncates_extra_fractional_digits() {
        assert_eq!(parse_decimal_to_minor("1.999", "x").unwrap(), 199);
    }

    #[test]
    fn rejects_double_decimal() {
        assert!(parse_decimal_to_minor("1.2.3", "x").is_err());
    }

    #[test]
    fn multibyte_fractional_does_not_panic() {
        // Previously `f[..2]` panicked on a non-char-boundary byte slice.
        assert!(parse_decimal_to_minor("5.½", "x").is_err());
        assert!(parse_decimal_to_minor("5½", "x").is_err());
    }

    #[test]
    fn rejects_overflowing_whole_part() {
        let huge = "1".repeat(40); // > i128::MAX once scaled by 100
        assert!(parse_decimal_to_minor(&huge, "x").is_err());
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq_str("abc", "abc"));
        assert!(!constant_time_eq_str("abc", "abd"));
        assert!(!constant_time_eq_str("abc", "ab"));
    }

    #[test]
    fn freshness_accepts_now_in_seconds_and_millis() {
        let now = chrono::Utc::now();
        verify_timestamp_freshness(&now.timestamp().to_string(), 300, "x").unwrap();
        verify_timestamp_freshness(&now.timestamp_millis().to_string(), 300, "x").unwrap();
    }

    #[test]
    fn freshness_rejects_stale_and_future() {
        let now = chrono::Utc::now().timestamp();
        assert!(matches!(
            verify_timestamp_freshness(&(now - 400).to_string(), 300, "x").unwrap_err(),
            AppError::Unauthorized
        ));
        assert!(matches!(
            verify_timestamp_freshness(&(now + 400).to_string(), 300, "x").unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn freshness_accepts_edge_of_window() {
        let now = chrono::Utc::now().timestamp();
        // Signed 299s ago against a 300s tolerance: inside the window, and far
        // enough from the boundary that a slow test run cannot flake it out.
        verify_timestamp_freshness(&(now - 299).to_string(), 300, "x").unwrap();
    }

    #[test]
    fn freshness_rejects_unparseable_and_tolerates_whitespace() {
        for bad in ["", "abc", "12.5", "1e9", "0x10"] {
            assert!(
                verify_timestamp_freshness(bad, 300, "x").is_err(),
                "should reject {bad:?}"
            );
        }
        let padded = format!("  {}  ", chrono::Utc::now().timestamp());
        verify_timestamp_freshness(&padded, 300, "x").unwrap();
    }

    #[test]
    fn freshness_with_zero_tolerance_rejects_anything_stale() {
        let now = chrono::Utc::now().timestamp();
        assert!(verify_timestamp_freshness(&(now - 5).to_string(), 0, "x").is_err());
        // A negative tolerance clamps to 0 rather than inverting the check.
        assert!(verify_timestamp_freshness(&(now - 5).to_string(), -100, "x").is_err());
    }

    #[test]
    fn stablecoin_mapping() {
        assert_eq!(stablecoin_to_fiat("USDC"), Some("USD"));
        assert_eq!(stablecoin_to_fiat("EURC"), Some("EUR"));
        assert_eq!(stablecoin_to_fiat("BTC"), None);
        assert_eq!(stablecoin_to_fiat("ETH"), None);
    }
}
