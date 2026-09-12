use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "notification_channel", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannel {
    Email,
    Webhook,
    Slack,
    Sms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "notification_dispatch_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Throttled,
    Suppressed,
}

#[derive(Clone, Debug, Serialize)]
pub struct NotificationRule {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub kind: String,
    pub name: String,
    pub params: serde_json::Value,
    pub channel: NotificationChannel,
    pub target: String,
    pub template_id: Option<String>,
    pub throttle_per_day: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateNotificationRule {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub channel: NotificationChannel,
    pub target: String,
    pub template_id: Option<String>,
    #[serde(default = "default_throttle")]
    pub throttle_per_day: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Optional plaintext credential (e.g. webhook signing secret, SendGrid
    /// API key). Sealed via the platform sealer before storage. Wire when
    /// notifications credentials live alongside provider credentials.
    pub credential_plaintext_b64: Option<String>,
}
fn default_throttle() -> i32 {
    1
}
fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize)]
pub struct NotificationDispatch {
    pub id: i64,
    pub rule_id: Uuid,
    pub tenant_id: Uuid,
    pub target_resource: Option<String>,
    pub channel: NotificationChannel,
    pub target: String,
    pub payload: serde_json::Value,
    pub status: DispatchStatus,
    pub provider_message_id: Option<String>,
    pub error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
