//! Hand-rolled RFC 4648 base32 and RFC 6238 TOTP (HOTP over HMAC-SHA-1).
//!
//! Deliberately dependency-light: `hmac` + `sha1` only, no `totp-rs`. The
//! implementation follows the same shape as the cluster's `auth-server-rs`
//! operator-login TOTP gate.

use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// RFC 4648 base32 alphabet (the one every authenticator app expects).
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// TOTP time step in seconds (RFC 6238 default).
pub const STEP_SECONDS: u64 = 30;

/// Accepted clock skew, in steps, on either side of "now".
pub const SKEW_STEPS: i64 = 1;

/// Base32-encode without padding (otpauth secrets are conventionally unpadded).
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// Base32-decode. Case-insensitive; ignores `=` padding and spaces.
/// Returns `None` on any character outside the RFC 4648 alphabet.
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        if ch == '=' || ch == ' ' {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        let index = ALPHABET.iter().position(|&a| a as char == upper)? as u32;
        buffer = (buffer << 5) | index;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// RFC 4226 HOTP: HMAC-SHA-1 over the big-endian counter, dynamic truncation,
/// `digits` decimal digits (zero-padded).
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> String {
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let code = u64::from(binary) % 10u64.pow(digits);
    format!("{code:0width$}", width = digits as usize)
}

/// Six-digit TOTP code for the step containing `unix_seconds`.
#[cfg(test)]
pub fn totp_code(secret: &[u8], unix_seconds: u64) -> String {
    hotp(secret, unix_seconds / STEP_SECONDS, 6)
}

/// Verify a submitted 6-digit code against the secret at `unix_seconds`,
/// allowing ±[`SKEW_STEPS`] steps of clock skew. Constant-time comparison.
///
/// The enrollment handler calls [`matching_counter`] directly so it can also
/// reject replays; this boolean form stays as the simple verification API
/// (tests and the in-flight TOTP regression suite use it).
#[cfg_attr(not(test), allow(dead_code))]
pub fn verify_totp(secret: &[u8], submitted: &str, unix_seconds: u64) -> bool {
    matching_counter(secret, submitted, unix_seconds).is_some()
}

/// Like [`verify_totp`], but return the HOTP counter the submitted code
/// matched, so callers can reject replays of an already-accepted counter
/// (RFC 6238 §5.2). `None` means the code is malformed or matches no counter
/// within ±[`SKEW_STEPS`] of the step containing `unix_seconds`.
pub fn matching_counter(secret: &[u8], submitted: &str, unix_seconds: u64) -> Option<u64> {
    let submitted = submitted.trim();
    if submitted.len() != 6 || !submitted.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let counter = unix_seconds / STEP_SECONDS;
    for offset in -SKEW_STEPS..=SKEW_STEPS {
        let Some(candidate_counter) = counter.checked_add_signed(offset) else {
            continue;
        };
        let candidate = hotp(secret, candidate_counter, 6);
        if constant_time_eq(submitted.as_bytes(), candidate.as_bytes()) {
            return Some(candidate_counter);
        }
    }
    None
}

/// Provisioning URI understood by Authy, Google Authenticator, 1Password, etc.
pub fn otpauth_uri(email: &str, secret_base32: &str) -> String {
    format!("otpauth://totp/3FA:{email}?secret={secret_base32}&issuer=3FA&digits=6&period=30")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B test vectors (SHA-1, 8 digits).
    const RFC6238_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_sha1_vectors() {
        for (t, expected) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
        ] {
            assert_eq!(hotp(RFC6238_SECRET, t / STEP_SECONDS, 8), expected, "T={t}");
        }
    }

    #[test]
    fn six_digit_codes_match_vector_suffixes() {
        // The 6-digit code is the 8-digit vector modulo 10^6.
        assert_eq!(totp_code(RFC6238_SECRET, 59), "287082");
        assert_eq!(totp_code(RFC6238_SECRET, 1_234_567_890), "005924");
    }

    #[test]
    fn base32_roundtrips() {
        let cases: &[&[u8]] = &[
            b"",
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            RFC6238_SECRET,
            &[0x00, 0xff, 0x10, 0x81, 0x3c],
        ];
        for &case in cases {
            let encoded = base32_encode(case);
            assert_eq!(
                base32_decode(&encoded).expect("decodes"),
                case,
                "roundtrip for {case:?}"
            );
        }
    }

    #[test]
    fn base32_known_values() {
        // RFC 4648 §10 vectors, sans padding.
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("mzxw6ytboi").unwrap(), b"foobar");
        assert_eq!(base32_decode("MZXW6YTBOI======").unwrap(), b"foobar");
    }

    #[test]
    fn base32_rejects_invalid_characters() {
        assert!(base32_decode("MZXW1").is_none()); // '1' not in alphabet
        assert!(base32_decode("MZ$W").is_none());
    }

    #[test]
    fn otpauth_uri_format() {
        let secret = base32_encode(RFC6238_SECRET);
        assert_eq!(
            otpauth_uri("sam@example.com", &secret),
            format!(
                "otpauth://totp/3FA:sam@example.com?secret={secret}&issuer=3FA&digits=6&period=30"
            )
        );
    }

    #[test]
    fn verify_accepts_adjacent_steps_and_rejects_garbage() {
        let now = 1_234_567_890u64;
        let current = totp_code(RFC6238_SECRET, now);
        let previous = hotp(RFC6238_SECRET, now / STEP_SECONDS - 1, 6);
        let next = hotp(RFC6238_SECRET, now / STEP_SECONDS + 1, 6);
        let far = hotp(RFC6238_SECRET, now / STEP_SECONDS + 5, 6);

        assert!(verify_totp(RFC6238_SECRET, &current, now));
        assert!(verify_totp(RFC6238_SECRET, &previous, now));
        assert!(verify_totp(RFC6238_SECRET, &next, now));
        if far != current && far != previous && far != next {
            assert!(!verify_totp(RFC6238_SECRET, &far, now));
        }
        assert!(!verify_totp(RFC6238_SECRET, "12345", now));
        assert!(!verify_totp(RFC6238_SECRET, "abcdef", now));
        assert!(!verify_totp(RFC6238_SECRET, "1234567", now));
    }

    #[test]
    fn matching_counter_identifies_the_exact_step_within_the_skew_window() {
        let now = 1_234_567_890u64;
        let counter = now / STEP_SECONDS;
        for offset in [-1i64, 0, 1] {
            let candidate_counter = counter.checked_add_signed(offset).unwrap();
            let code = hotp(RFC6238_SECRET, candidate_counter, 6);
            // A colliding adjacent code would legitimately match the nearer
            // counter first; only assert when the codes are distinct.
            let distinct = (-SKEW_STEPS..=SKEW_STEPS)
                .filter(|&other| other != offset)
                .all(|other| {
                    hotp(
                        RFC6238_SECRET,
                        counter.checked_add_signed(other).unwrap(),
                        6,
                    ) != code
                });
            if distinct {
                assert_eq!(
                    matching_counter(RFC6238_SECRET, &code, now),
                    Some(candidate_counter),
                    "offset {offset}"
                );
            }
        }
    }

    #[test]
    fn matching_counter_rejects_out_of_window_and_malformed_codes() {
        let now = 1_234_567_890u64;
        let counter = now / STEP_SECONDS;
        let in_window: Vec<String> = (-SKEW_STEPS..=SKEW_STEPS)
            .map(|offset| {
                hotp(
                    RFC6238_SECRET,
                    counter.checked_add_signed(offset).unwrap(),
                    6,
                )
            })
            .collect();
        let far = hotp(RFC6238_SECRET, counter + 5, 6);
        if !in_window.contains(&far) {
            assert_eq!(matching_counter(RFC6238_SECRET, &far, now), None);
        }
        assert_eq!(matching_counter(RFC6238_SECRET, "12345", now), None);
        assert_eq!(matching_counter(RFC6238_SECRET, "abcdef", now), None);
        assert_eq!(matching_counter(RFC6238_SECRET, "", now), None);
    }
}
