//! Channel drivers.
//!
//! Webhook is the reference impl — POST JSON to a tenant URL with an
//! HMAC-SHA256 signature in `X-Billing-Signature` so the tenant can verify
//! the call came from us. Email/Slack/SMS share the same shape and are
//! intentionally stubbed for now; wire them when the platform's outbound
//! email provider (SES/SendGrid) is selected.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::types::NotificationChannel;

pub struct WebhookDeliveryResult {
    pub provider_message_id: Option<String>,
    pub http_status: u16,
}

pub async fn deliver_webhook(
    target_url: &str,
    payload: &serde_json::Value,
    signing_secret: Option<&str>,
) -> Result<WebhookDeliveryResult, String> {
    let body = serde_json::to_vec(payload).map_err(|e| format!("encode: {e}"))?;
    let timestamp = chrono::Utc::now().timestamp().to_string();

    let mut req = reqwest::Client::new()
        .post(target_url)
        .header("content-type", "application/json")
        .header("x-billing-timestamp", &timestamp);

    if let Some(secret) = signing_secret {
        let signed_payload = format!("{timestamp}.");
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
            .map_err(|e| format!("hmac init: {e}"))?;
        Mac::update(&mut mac, signed_payload.as_bytes());
        Mac::update(&mut mac, &body);
        let tag = Mac::finalize(mac).into_bytes();
        req = req.header("x-billing-signature", format!("v1={}", hex::encode(tag)));
    }

    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| format!("http: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("webhook returned {}", status));
    }
    Ok(WebhookDeliveryResult {
        provider_message_id: resp
            .headers()
            .get("x-message-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        http_status: status.as_u16(),
    })
}

pub async fn deliver_email(
    _target: &str,
    _payload: &serde_json::Value,
    _api_key: Option<&str>,
) -> Result<WebhookDeliveryResult, String> {
    // TODO(real impl): SES SendEmail / SendGrid /mail/send.
    Err("email channel not implemented".into())
}

pub async fn deliver_slack(
    target_url: &str,
    payload: &serde_json::Value,
    _signing_secret: Option<&str>,
) -> Result<WebhookDeliveryResult, String> {
    // Slack incoming webhooks accept a `{"text": "..."}` shape; just POST.
    let body = serde_json::json!({
        "text": payload.get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("billing-server notification"),
        "attachments": [{
            "color": "#36a64f",
            "text": serde_json::to_string(payload).unwrap_or_default(),
        }],
    });
    let resp = reqwest::Client::new()
        .post(target_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("http: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("slack returned {}", status));
    }
    Ok(WebhookDeliveryResult {
        provider_message_id: None,
        http_status: status.as_u16(),
    })
}

pub async fn deliver_sms(
    _target: &str,
    _payload: &serde_json::Value,
    _credential: Option<&str>,
) -> Result<WebhookDeliveryResult, String> {
    Err("sms channel not implemented".into())
}

pub fn channel_name(c: NotificationChannel) -> &'static str {
    match c {
        NotificationChannel::Email => "email",
        NotificationChannel::Webhook => "webhook",
        NotificationChannel::Slack => "slack",
        NotificationChannel::Sms => "sms",
    }
}
