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
    link_state: Option<&str>,
) -> Result<(), AuthError> {
    send_magic_link_to(
        http,
        config,
        SENDGRID_MAIL_SEND_URL,
        recipient,
        token,
        otp,
        link_state,
    )
    .await
}

async fn send_magic_link_to(
    http: &reqwest::Client,
    config: &MagicLinkConfig,
    endpoint: &str,
    recipient: &str,
    token: &str,
    otp: &str,
    link_state: Option<&str>,
) -> Result<(), AuthError> {
    let api_key = config
        .sendgrid_api_key
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let from_email = config.from_email.as_deref().ok_or(AuthError::Unavailable)?;
    let link = magic_link_url(config, token, link_state)?;
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
        .post(endpoint)
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

fn magic_link_url(
    config: &MagicLinkConfig,
    token: &str,
    link_state: Option<&str>,
) -> Result<String, AuthError> {
    let base = config
        .link_base_url
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let mut url = reqwest::Url::parse(base).map_err(|_| AuthError::Internal)?;
    let mut query = url.query_pairs_mut();
    query.append_pair("token", token);
    if let Some(state) = link_state {
        query.append_pair("state", state);
    }
    drop(query);
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
#[path = "email_tests.rs"]
mod tests;
