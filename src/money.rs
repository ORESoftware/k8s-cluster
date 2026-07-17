use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// ISO-4217 currency code (or `XCH` style for crypto rails we model as currency).
///
/// We deliberately keep this an opaque 3-char string rather than enum so adding a
/// new currency (SOL, USDC-SOL, etc.) is a config change, not a code change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Currency(pub [u8; 3]);

impl Currency {
    pub fn new(s: &str) -> anyhow::Result<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_alphabetic()) {
            anyhow::bail!("currency must be 3 ASCII letters: {s:?}");
        }
        let mut out = [0u8; 3];
        for (i, b) in bytes.iter().enumerate() {
            out[i] = b.to_ascii_uppercase();
        }
        Ok(Self(out))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("currency is 3 ascii letters")
    }

    pub fn usd() -> Self {
        Self(*b"USD")
    }
    pub fn eur() -> Self {
        Self(*b"EUR")
    }
    pub fn usdc() -> Self {
        Self(*b"USC")
    } // canonicalize USDC-on-X as 'USC' (3 chars)
    pub fn sol() -> Self {
        Self(*b"SOL")
    }
}

impl FromStr for Currency {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::new(s)
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An exact monetary amount stored in minor units. NEVER use floats for money.
///
/// `minor` is i128 to leave headroom for crypto (e.g. SOL is 9 decimals, USDC
/// on Solana is 6 decimals; sums across millions of users still fit comfortably).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub minor: i128,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i128, currency: Currency) -> Self {
        Self { minor, currency }
    }

    pub fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    pub fn checked_add(self, other: Self) -> anyhow::Result<Self> {
        if self.currency != other.currency {
            anyhow::bail!("currency mismatch: {} vs {}", self.currency, other.currency);
        }
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or_else(|| anyhow::anyhow!("money overflow"))?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    pub fn checked_sub(self, other: Self) -> anyhow::Result<Self> {
        if self.currency != other.currency {
            anyhow::bail!("currency mismatch: {} vs {}", self.currency, other.currency);
        }
        let minor = self
            .minor
            .checked_sub(other.minor)
            .ok_or_else(|| anyhow::anyhow!("money overflow"))?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.minor, self.currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_new_uppercases_input() {
        let c = Currency::new("usd").unwrap();
        assert_eq!(c, Currency::usd());
        assert_eq!(c.as_str(), "USD");
        // Mixed case normalizes too.
        assert_eq!(Currency::new("eUr").unwrap(), Currency::eur());
    }

    #[test]
    fn currency_new_rejects_invalid_codes() {
        assert!(Currency::new("").is_err());
        assert!(Currency::new("US").is_err());
        assert!(Currency::new("USDC").is_err());
        assert!(Currency::new("U5D").is_err());
        assert!(Currency::new("U D").is_err());
        // Multi-byte UTF-8 that is 3 bytes but not 3 ASCII letters.
        assert!(Currency::new("€").is_err());
    }

    #[test]
    fn currency_from_str_and_display_roundtrip() {
        let c: Currency = "sol".parse().unwrap();
        assert_eq!(c, Currency::sol());
        assert_eq!(c.to_string(), "SOL");
        let back: Currency = c.to_string().parse().unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn currency_serde_roundtrip() {
        let c = Currency::usdc();
        let json = serde_json::to_string(&c).unwrap();
        let back: Currency = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn checked_add_sums_minor_units_of_same_currency() {
        let a = Money::new(150, Currency::usd());
        let b = Money::new(-49, Currency::usd());
        let sum = a.checked_add(b).unwrap();
        assert_eq!(sum, Money::new(101, Currency::usd()));
    }

    #[test]
    fn checked_add_rejects_currency_mismatch() {
        let usd = Money::new(100, Currency::usd());
        let eur = Money::new(100, Currency::eur());
        let err = usd.checked_add(eur).unwrap_err();
        assert!(err.to_string().contains("currency mismatch"));
    }

    #[test]
    fn checked_add_detects_overflow() {
        let max = Money::new(i128::MAX, Currency::usd());
        let one = Money::new(1, Currency::usd());
        assert!(max.checked_add(one).is_err());
        // Adding zero at the boundary is still fine.
        assert_eq!(max.checked_add(Money::zero(Currency::usd())).unwrap(), max);
    }

    #[test]
    fn checked_sub_allows_negative_results() {
        let a = Money::new(10, Currency::eur());
        let b = Money::new(25, Currency::eur());
        let diff = a.checked_sub(b).unwrap();
        assert_eq!(diff.minor, -15);
        assert_eq!(diff.currency, Currency::eur());
    }

    #[test]
    fn checked_sub_rejects_mismatch_and_overflow() {
        let a = Money::new(1, Currency::usd());
        assert!(a.checked_sub(Money::new(1, Currency::sol())).is_err());
        let min = Money::new(i128::MIN, Currency::usd());
        assert!(min.checked_sub(Money::new(1, Currency::usd())).is_err());
    }

    #[test]
    fn zero_and_display_format() {
        let z = Money::zero(Currency::usd());
        assert_eq!(z.minor, 0);
        assert_eq!(z.to_string(), "0 USD");
        assert_eq!(Money::new(-1500, Currency::eur()).to_string(), "-1500 EUR");
    }
}
