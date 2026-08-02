#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_matches_rfc4648_vectors_without_padding() {
        assert_eq!(encode_base32(b"foo"), "MZXW6");
        assert_eq!(encode_base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn rfc6238_sha1_vector_is_correct() {
        let secret = b"12345678901234567890";
        assert_eq!(totp_code(secret, 59 / 30), "287082");
    }

    #[test]
    fn totp_ciphertext_is_bound_to_user_and_factor() {
        let key = [7_u8; 32];
        let nonce = [9_u8; 12];
        let user_id = Uuid::new_v4();
        let factor_id = Uuid::new_v4();
        let secret = b"12345678901234567890";
        let ciphertext =
            encrypt_totp_secret(&key, user_id, factor_id, nonce, secret).expect("encrypt");
        assert_eq!(
            decrypt_totp_secret(&key, user_id, factor_id, nonce, &ciphertext).expect("decrypt"),
            secret
        );
        assert!(
            decrypt_totp_secret(&key, Uuid::new_v4(), factor_id, nonce, &ciphertext).is_err()
        );
        assert!(
            decrypt_totp_secret(&key, user_id, Uuid::new_v4(), nonce, &ciphertext).is_err()
        );
    }

    #[test]
    fn otp_comparisons_are_exact() {
        let key = b"a sufficiently long test-only OTP pepper";
        let challenge_id = Uuid::new_v4();
        let tag = otp_tag(key, challenge_id, "123456").expect("tag");
        assert!(otp_tag_matches(key, challenge_id, "123456", &tag));
        assert!(!otp_tag_matches(key, challenge_id, "123457", &tag));
        assert!(constant_time_code_eq("654321", "654321", key));
        assert!(!constant_time_code_eq("654321", "654320", key));
    }

    #[test]
    fn generated_codes_are_six_decimal_digits() {
        for _ in 0..64 {
            let code = generate_code().expect("secure random code");
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn webauthn_configuration_enforces_secure_origin_and_rp_binding() {
        let local = Url::parse("http://localhost:4173").unwrap();
        validate_webauthn_config("localhost", &local, "Local test").unwrap();

        let production = Url::parse("https://login.example.com").unwrap();
        validate_webauthn_config("example.com", &production, "Example").unwrap();

        let insecure = Url::parse("http://login.example.com").unwrap();
        assert!(validate_webauthn_config("example.com", &insecure, "Example").is_err());

        let unrelated = Url::parse("https://attacker.example.net").unwrap();
        assert!(validate_webauthn_config("example.com", &unrelated, "Example").is_err());

        let path = Url::parse("https://login.example.com/webauthn").unwrap();
        assert!(validate_webauthn_config("example.com", &path, "Example").is_err());
    }

    #[test]
    fn destination_masks_do_not_disclose_the_full_value() {
        assert_eq!(mask_destination("alex@example.com"), "a•••@example.com");
        assert_eq!(mask_destination("+14155550100"), "••••0100");
    }

    #[test]
    fn invalid_factor_key_is_rejected() {
        assert!(hex_nibble(b'g').is_err());
    }
}
