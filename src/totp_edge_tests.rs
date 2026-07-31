use crate::totp::{
    STEP_SECONDS, base32_decode, base32_encode, hotp, otpauth_uri, verify_totp,
};

const RFC6238_SECRET: &[u8] = b"12345678901234567890";

#[test]
fn complete_rfc4648_base32_vectors_roundtrip() {
    for (plain, encoded) in [
        (b"".as_slice(), ""),
        (b"f".as_slice(), "MY"),
        (b"fo".as_slice(), "MZXQ"),
        (b"foo".as_slice(), "MZXW6"),
        (b"foob".as_slice(), "MZXW6YQ"),
        (b"fooba".as_slice(), "MZXW6YTB"),
        (b"foobar".as_slice(), "MZXW6YTBOI"),
    ] {
        assert_eq!(base32_encode(plain), encoded);
        assert_eq!(base32_decode(encoded).as_deref(), Some(plain));
    }
}

#[test]
fn decoder_accepts_human_friendly_case_spacing_and_padding() {
    assert_eq!(
        base32_decode("mzxw 6ytb oi======").as_deref(),
        Some(b"foobar".as_slice())
    );
}

#[test]
fn verification_at_epoch_handles_negative_skew_without_underflow() {
    let current = hotp(RFC6238_SECRET, 0, 6);
    let next = hotp(RFC6238_SECRET, 1, 6);

    assert!(verify_totp(RFC6238_SECRET, &current, 0));
    assert!(verify_totp(RFC6238_SECRET, &next, 0));
}

#[test]
fn verification_changes_only_after_the_step_boundary() {
    let counter = 1234;
    let code = hotp(RFC6238_SECRET, counter, 6);
    let start = counter * STEP_SECONDS;

    assert!(verify_totp(RFC6238_SECRET, &code, start));
    assert!(verify_totp(
        RFC6238_SECRET,
        &code,
        start + STEP_SECONDS - 1
    ));
}

#[test]
fn provisioning_uri_keeps_the_interoperability_contract_explicit() {
    let secret = base32_encode(RFC6238_SECRET);
    let uri = otpauth_uri("alex@example.com", &secret);

    assert!(uri.starts_with("otpauth://totp/3FA:alex@example.com?"));
    assert!(uri.contains(&format!("secret={secret}")));
    assert!(uri.contains("issuer=3FA"));
    assert!(uri.contains("digits=6"));
    assert!(uri.contains("period=30"));
}
