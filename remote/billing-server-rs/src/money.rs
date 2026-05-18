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

    pub fn usd() -> Self { Self(*b"USD") }
    pub fn eur() -> Self { Self(*b"EUR") }
    pub fn usdc() -> Self { Self(*b"USC") } // canonicalize USDC-on-X as 'USC' (3 chars)
    pub fn sol() -> Self { Self(*b"SOL") }
}

impl FromStr for Currency {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> { Self::new(s) }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.as_str()) }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.as_str()) }
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
    pub fn new(minor: i128, currency: Currency) -> Self { Self { minor, currency } }

    pub fn zero(currency: Currency) -> Self { Self { minor: 0, currency } }

    pub fn checked_add(self, other: Self) -> anyhow::Result<Self> {
        if self.currency != other.currency {
            anyhow::bail!("currency mismatch: {} vs {}", self.currency, other.currency);
        }
        let minor = self.minor.checked_add(other.minor)
            .ok_or_else(|| anyhow::anyhow!("money overflow"))?;
        Ok(Self { minor, currency: self.currency })
    }

    pub fn checked_sub(self, other: Self) -> anyhow::Result<Self> {
        if self.currency != other.currency {
            anyhow::bail!("currency mismatch: {} vs {}", self.currency, other.currency);
        }
        let minor = self.minor.checked_sub(other.minor)
            .ok_or_else(|| anyhow::anyhow!("money overflow"))?;
        Ok(Self { minor, currency: self.currency })
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.minor, self.currency)
    }
}
