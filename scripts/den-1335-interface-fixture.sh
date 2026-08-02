#!/usr/bin/env bash
set -euo pipefail

root="../sonus-auris-interfaces/generated/rust"
mkdir -p "$root/src"
cat > "$root/Cargo.toml" <<'TOML'
[package]
name = "sonus-auris-interfaces"
version = "0.1.0"
edition = "2021"
description = "Generated Rust adapters for the Sonus Auris Supabase schema."

[dependencies]
chrono = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = "1"
TOML
cat > "$root/src/lib.rs" <<'RS'
use serde::{Deserialize, Serialize};

pub const ACOUSTIC_EVENTS_TABLE: &str = "acoustic_events";
pub const ACOUSTIC_EVENTS_COLUMNS: &[&str] = &["id", "user_id", "device_id", "kind", "started_at", "ended_at", "confidence", "details", "created_at"];
pub const DEVICES_TABLE: &str = "devices";
pub const DEVICES_COLUMNS: &[&str] = &["user_id", "device_id", "display_name", "platform", "role", "app_version", "last_seen_at", "revoked_at", "created_at"];
pub const USER_CONSENTS_TABLE: &str = "user_consents";
pub const USER_CONSENTS_COLUMNS: &[&str] = &["id", "user_id", "device_id", "consent_version", "platform", "granted", "accepted_at", "created_at"];
pub const USER_SETTINGS_TABLE: &str = "user_settings";
pub const USER_SETTINGS_COLUMNS: &[&str] = &["user_id", "preferred_use_case", "device_retention_hours", "cloud_retention_hours", "segment_minutes", "overlap_seconds", "bit_rate", "sample_rate", "channels", "upload_enabled", "cloud_provider", "mic_sensitivity", "noise_trigger_sensitivity", "bass_gain_db", "mid_gain_db", "treble_gain_db", "auto_gain", "noise_suppress", "acoustic_analysis_enabled", "analysis_activation_db", "analysis_sustain_seconds", "analysis_hold_seconds", "snore_detection_enabled", "sleep_analysis_enabled", "music_detection_enabled", "speech_detection_enabled", "adaptive_quality_enabled", "capture_sample_rate", "quiet_sample_rate", "adaptive_loudness_db", "updated_at"];
pub const USER_SETTINGS_PREFERRED_USE_CASE_VALUES: &[&str] = &["security", "music", "meeting", "voice_note", "ambient"];
pub const USER_SETTINGS_CLOUD_PROVIDER_VALUES: &[&str] = &["s3", "googleDrive", "oneDrive", "iCloudDrive", "dropbox"];
pub const CLOUD_CONNECTIONS_TABLE: &str = "cloud_connections";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AcousticEvent(pub serde_json::Value);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceRecord(pub serde_json::Value);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserConsent(pub serde_json::Value);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSettings {
    pub user_id: String,
    pub preferred_use_case: String,
    pub device_retention_hours: i64,
    pub cloud_retention_hours: i64,
    pub segment_minutes: i64,
    pub overlap_seconds: i64,
    pub bit_rate: i64,
    pub sample_rate: i64,
    pub channels: i64,
    pub upload_enabled: bool,
    pub cloud_provider: String,
    pub mic_sensitivity: f64,
    pub noise_trigger_sensitivity: f64,
    pub bass_gain_db: f64,
    pub mid_gain_db: f64,
    pub treble_gain_db: f64,
    pub auto_gain: bool,
    pub noise_suppress: bool,
    pub acoustic_analysis_enabled: bool,
    pub analysis_activation_db: f64,
    pub analysis_sustain_seconds: f64,
    pub analysis_hold_seconds: f64,
    pub snore_detection_enabled: bool,
    pub sleep_analysis_enabled: bool,
    pub music_detection_enabled: bool,
    pub speech_detection_enabled: bool,
    pub adaptive_quality_enabled: bool,
    pub capture_sample_rate: i64,
    pub quiet_sample_rate: i64,
    pub adaptive_loudness_db: f64,
    pub updated_at: String,
}
RS
