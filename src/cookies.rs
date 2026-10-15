//! Minimal signed-cookie helpers.

use axum::http::{header, HeaderMap, HeaderValue};
use hmac::{Hmac, Mac};
use sha1::Sha1;

pub(crate) const SESSION_COOKIE: &str = "threefa_session";
pub(crate) const EMAIL_COOKIE: &str = "threefa_email";
pub(crate) const ENROLL_COOKIE: &str = "threefa_enroll";

pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}

pub(crate) fn set_cookie(name: &str, value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Secure"
    ))
    .expect("cookie header value")
}

pub(crate) fn sign(key: &[u8], message: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .fold(String::with_capacity(40), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("write to string");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_parser_selects_exact_names_from_a_cookie_jar() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; threefa_session=abc123; other=value"),
        );
        assert_eq!(
            cookie_value(&headers, SESSION_COOKIE).as_deref(),
            Some("abc123")
        );
        assert_eq!(cookie_value(&headers, "session"), None);
        assert_eq!(cookie_value(&HeaderMap::new(), SESSION_COOKIE), None);
    }

    #[test]
    fn response_cookies_keep_the_security_attributes() {
        let header = set_cookie(SESSION_COOKIE, "opaque");
        let cookie = header.to_str().unwrap();
        assert_eq!(
            cookie,
            "threefa_session=opaque; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }

    #[test]
    fn signatures_are_stable_and_bind_key_and_message() {
        let signature = sign(b"key-a", "message-a");
        assert_eq!(signature.len(), 40);
        assert_eq!(signature, sign(b"key-a", "message-a"));
        assert_ne!(signature, sign(b"key-b", "message-a"));
        assert_ne!(signature, sign(b"key-a", "message-b"));
    }
}
