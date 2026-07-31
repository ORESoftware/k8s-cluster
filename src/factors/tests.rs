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
    fn destination_masks_do_not_disclose_the_full_value() {
        assert_eq!(mask_destination("alex@example.com"), "a•••@example.com");
        assert_eq!(mask_destination("+14155550100"), "••••0100");
    }

    #[test]
    fn invalid_factor_key_is_rejected() {
        assert!(hex_nibble(b'g').is_err());
    }
}
