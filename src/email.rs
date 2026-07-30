//! Optional SendGrid delivery for RDS-backed passwordless sign-in.
//!
//! The raw one-time token is never logged or stored. Configuration is optional:
//! when it is absent, only the magic-link request endpoint is unavailable.

use serde_json::json;

use crate::config::MagicLinkConfig;
use crate::error::AuthError;

const SENDGRID_MAIL_SEND_URL: &str = "https://api.sendgrid.com/v3/mail/send";

pub async fn send_magic_link(
    http: &reqwest::Client,
    config: &MagicLinkConfig,
    recipient: &str,
    token: &str,
    otp: &str,
) -> Result<(), AuthError> {
    let api_key = config
        .sendgrid_api_key
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let from_email = config.from_email.as_deref().ok_or(AuthError::Unavailable)?;
    let link = magic_link_url(config, token)?;
    let escaped_link = escape_html(&link);
    let text = format!(
        "Use this one-time link or six-digit code to sign in. They expire in {} minutes:\n\n\
         {}\n\nCode: {}\n\n\
         If you did not request this email, you can ignore it.",
        config.ttl_secs / 60,
        link,
        otp
    );
    let html = format!(
        "<p>Use this one-time link or six-digit code to sign in. They expire in {} minutes.</p>\
         <p><a href=\"{}\">Sign in</a></p>\
         <p>Code: <strong>{}</strong></p>\
         <p>If you did not request this email, you can ignore it.</p>",
        config.ttl_secs / 60,
        escaped_link,
        otp
    );
    let payload = json!({
        "personalizations": [{
            "to": [{ "email": recipient }]
        }],
        "from": {
            "email": from_email,
            "name": config.from_name
        },
        "subject": "Your sign-in link",
        "content": [
            { "type": "text/plain", "value": text },
            { "type": "text/html", "value": html }
        ]
    });

    let response = http
        .post(SENDGRID_MAIL_SEND_URL)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "SendGrid magic-link request failed");
            AuthError::Upstream
        })?;
    if response.status() != reqwest::StatusCode::ACCEPTED {
        tracing::warn!(
            status = response.status().as_u16(),
            "SendGrid rejected magic-link email"
        );
        return Err(AuthError::Upstream);
    }
    Ok(())
}

fn magic_link_url(config: &MagicLinkConfig, token: &str) -> Result<String, AuthError> {
    let base = config
        .link_base_url
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let mut url = reqwest::Url::parse(base).map_err(|_| AuthError::Internal)?;
    url.query_pairs_mut().append_pair("token", token);
    Ok(url.into())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MagicLinkConfig {
        MagicLinkConfig {
            sendgrid_api_key: Some("secret".into()),
            otp_pepper: Some("test-pepper-at-least-thirty-two-bytes".into()),
            from_email: Some("auth@example.com".into()),
            from_name: "Example".into(),
            link_base_url: Some("https://app.example/auth/magic-link".into()),
            ttl_secs: 900,
            allow_signup: true,
        }
    }

    #[test]
    fn link_contains_only_the_one_time_token_query() {
        let link = magic_link_url(&config(), "sat_magic_a-b_C").unwrap();
        assert_eq!(
            link,
            "https://app.example/auth/magic-link?token=sat_magic_a-b_C"
        );
    }

    #[test]
    fn html_escaping_protects_the_anchor_attribute() {
        assert_eq!(
            escape_html("https://example.test/?a=1&b=\"x\""),
            "https://example.test/?a=1&amp;b=&quot;x&quot;"
        );
    }
}
