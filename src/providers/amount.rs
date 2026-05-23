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
            let f = if f.len() >= 2 {
                f[..2].to_string()
            } else {
                format!("{f}{}", "0".repeat(2 - f.len()))
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
    let v = whole_i * 100 + frac_i;
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
    fn constant_time_eq_basic() {
        assert!(constant_time_eq_str("abc", "abc"));
        assert!(!constant_time_eq_str("abc", "abd"));
        assert!(!constant_time_eq_str("abc", "ab"));
    }

    #[test]
    fn stablecoin_mapping() {
        assert_eq!(stablecoin_to_fiat("USDC"), Some("USD"));
        assert_eq!(stablecoin_to_fiat("EURC"), Some("EUR"));
        assert_eq!(stablecoin_to_fiat("BTC"), None);
        assert_eq!(stablecoin_to_fiat("ETH"), None);
    }
}
