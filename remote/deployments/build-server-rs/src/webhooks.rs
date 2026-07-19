//! Inbound webhooks.
//!
//! `POST /webhooks/github` — GitHub push / workflow_run events, verified with
//! the `X-Hub-Signature-256` HMAC (BUILD_SERVER_GITHUB_WEBHOOK_SECRET,
//! constant-time compare) and deduped on `X-GitHub-Delivery`. Matching rules
//! (BUILD_SERVER_WEBHOOK_RULES / _PATH, JSON) map repo+branch to a
//! build-server.v1 job, so GitHub Actions or plain repo pushes can trigger
//! builds without holding credentials for this server.
//!
//! `POST /webhooks/registry` — container registry events (ECR EventBridge
//! `ECR Image Action` detail or docker distribution v2 event envelopes),
//! authenticated with a shared secret header (registries cannot HMAC-sign),
//! recorded for audit and relayed to NATS `dd.remote.build_server.images`.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use std::sync::atomic::Ordering;
use subtle::ConstantTimeEq;

use crate::{db, events, fiducia, AppState, BuildRequest, DeployRequest};

/// One mapping from a GitHub repo/branch to a build job. Loaded from
/// BUILD_SERVER_WEBHOOK_RULES (inline JSON array) or
/// BUILD_SERVER_WEBHOOK_RULES_PATH (mounted ConfigMap file).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookRule {
    /// GitHub `owner/name`, matched case-insensitively.
    pub repo: String,
    /// Branch to react to (default: any). Tag pushes only match when `tags` is true.
    pub branch: Option<String>,
    /// Also react to tag pushes.
    #[serde(default)]
    pub tags: bool,
    /// Events to react to: "push" (default) and/or "workflow_run" (completed+success).
    pub events: Option<Vec<String>>,
    /// Image template for image jobs. `{sha}` / `{shortSha}` / `{ref}` are substituted.
    pub image: Option<String>,
    /// Fixed CI profile for run-profile jobs. Exactly one of image/profile is required.
    pub profile: Option<String>,
    pub context_dir: Option<String>,
    pub dockerfile: Option<String>,
    #[serde(default)]
    pub push: bool,
    pub executor: Option<String>,
    pub deploy: Option<DeployRequest>,
}

pub fn parse_rules(raw: &str) -> Result<Vec<WebhookRule>, String> {
    let rules = serde_json::from_str::<Vec<WebhookRule>>(raw)
        .map_err(|error| format!("invalid webhook rules JSON: {error}"))?;
    for rule in &rules {
        if rule.image.is_some() == rule.profile.is_some() {
            return Err(format!(
                "webhook rule for {:?} must set exactly one of image or profile",
                rule.repo
            ));
        }
        if rule.profile.is_some()
            && (rule.push || rule.deploy.is_some() || rule.dockerfile.is_some())
        {
            return Err(format!(
                "profile webhook rule for {:?} cannot push, deploy, or set dockerfile",
                rule.repo
            ));
        }
    }
    Ok(rules)
}

fn verify_github_signature(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Some(hex_signature) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected) = hex::decode(hex_signature) else {
        return false;
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(body);
    let computed = mac.finalize().into_bytes();
    computed.as_slice().ct_eq(expected.as_slice()).into()
}

fn substitute_image(template: &str, sha: &str, git_ref: &str) -> String {
    let short_sha = &sha[..sha.len().min(12)];
    template
        .replace("{sha}", sha)
        .replace("{shortSha}", short_sha)
        .replace("{ref}", git_ref)
}

fn branch_from_ref(git_ref: &str) -> Option<&str> {
    git_ref.strip_prefix("refs/heads/")
}

fn rule_matches(rule: &WebhookRule, event: &str, repo: &str, git_ref: &str) -> bool {
    if !rule.repo.eq_ignore_ascii_case(repo) {
        return false;
    }
    let events = rule
        .events
        .clone()
        .unwrap_or_else(|| vec!["push".to_string()]);
    if !events.iter().any(|candidate| candidate == event) {
        return false;
    }
    if let Some(branch) = branch_from_ref(git_ref) {
        match rule.branch.as_deref() {
            Some(want) => want == branch,
            None => true,
        }
    } else if git_ref.starts_with("refs/tags/") {
        rule.tags
    } else {
        // workflow_run carries a bare branch name.
        match rule.branch.as_deref() {
            Some(want) => want == git_ref,
            None => true,
        }
    }
}

fn build_request_from_rule(
    rule: &WebhookRule,
    repo: &str,
    git_ref: &str,
    sha: &str,
) -> BuildRequest {
    let branch = branch_from_ref(git_ref).unwrap_or(git_ref).to_string();
    BuildRequest {
        schema_version: Some("build-server.v1".to_string()),
        job_kind: Some(if rule.profile.is_some() {
            "run-profile".to_string()
        } else if rule.deploy.is_some() {
            "build-and-deploy".to_string()
        } else {
            "build-image".to_string()
        }),
        repo_url: format!("https://github.com/{repo}.git"),
        git_ref: Some(branch.clone()),
        image: rule
            .image
            .as_deref()
            .map(|image| substitute_image(image, sha, &branch))
            .unwrap_or_default(),
        profile: rule.profile.clone(),
        context_dir: rule.context_dir.clone(),
        dockerfile: rule.dockerfile.clone(),
        build_args: None,
        push: Some(rule.push),
        deploy: rule.deploy.clone(),
        executor: rule.executor.clone(),
        request_id: None,
    }
}

pub async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(secret) = state.config.github_webhook_secret.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "BUILD_SERVER_GITHUB_WEBHOOK_SECRET is not configured" })),
        )
            .into_response();
    };
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !verify_github_signature(secret, &body, signature) {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid or missing X-Hub-Signature-256" })),
        )
            .into_response();
    }

    let event = headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let delivery_id = headers
        .get("x-github-delivery")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if delivery_id.is_empty() || delivery_id.len() > 128 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing or invalid X-GitHub-Delivery" })),
        )
            .into_response();
    }
    if event == "ping" {
        return (StatusCode::OK, Json(json!({ "ok": true, "event": "ping" }))).into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid webhook JSON: {error}") })),
            )
                .into_response();
        }
    };

    let repo = payload
        .pointer("/repository/full_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();

    let (git_ref, sha, actionable) = match event.as_str() {
        "push" => {
            let git_ref = payload
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let sha = payload
                .get("after")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let deleted = payload.get("deleted").and_then(serde_json::Value::as_bool) == Some(true);
            (git_ref, sha, !deleted)
        }
        "workflow_run" => {
            let run = payload.get("workflow_run").cloned().unwrap_or_default();
            let git_ref = run
                .get("head_branch")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let sha = run
                .get("head_sha")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let completed_ok = payload.get("action").and_then(serde_json::Value::as_str)
                == Some("completed")
                && run.get("conclusion").and_then(serde_json::Value::as_str) == Some("success");
            (git_ref, sha, completed_ok)
        }
        _ => (String::new(), String::new(), false),
    };

    state
        .counters
        .webhooks_received
        .fetch_add(1, Ordering::Relaxed);

    // Local dedupe (Postgres unique) + multi-replica dedupe (fiducia lease).
    if let Some(db) = state.db.as_ref() {
        let fresh = db::record_webhook_delivery(
            db,
            "github",
            &delivery_id,
            Some(&event),
            Some(&repo),
            Some(&git_ref),
            "received",
        )
        .await;
        if !fresh {
            return (
                StatusCode::OK,
                Json(json!({ "ok": true, "action": "duplicate", "deliveryId": delivery_id })),
            )
                .into_response();
        }
    }
    let idem_key = format!("build-server/webhook/github/{delivery_id}");
    match fiducia::idempotency_claim(&state.http, &state.config, &idem_key, &state.holder).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::OK,
                Json(json!({ "ok": true, "action": "duplicate", "deliveryId": delivery_id })),
            )
                .into_response();
        }
        Err(error) => {
            // Fail open: Postgres dedupe above remains the local guard.
            tracing::warn!("webhook idempotency claim failed (continuing): {error}");
        }
    }

    if !actionable || repo.is_empty() || sha.is_empty() {
        fiducia::idempotency_finish(&state.http, &state.config, &idem_key, &state.holder, true)
            .await;
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "action": "ignored", "event": event })),
        )
            .into_response();
    }

    let matched = state
        .config
        .webhook_rules
        .iter()
        .find(|rule| rule_matches(rule, &event, &repo, &git_ref));
    let Some(rule) = matched else {
        fiducia::idempotency_finish(&state.http, &state.config, &idem_key, &state.holder, true)
            .await;
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "action": "ignored", "reason": "no matching rule" })),
        )
            .into_response();
    };

    let request = build_request_from_rule(rule, &repo, &git_ref, &sha);
    let outcome = crate::enqueue_build(&state, request, "webhook").await;
    fiducia::idempotency_finish(
        &state.http,
        &state.config,
        &idem_key,
        &state.holder,
        outcome.is_ok(),
    )
    .await;
    match outcome {
        Ok(record) => {
            if let Some(db) = state.db.as_ref() {
                db::record_webhook_delivery(
                    db,
                    "github",
                    &format!("{delivery_id}:enqueued"),
                    Some(&event),
                    Some(&repo),
                    Some(&git_ref),
                    &format!("enqueued:{}", record.id),
                )
                .await;
            }
            (StatusCode::ACCEPTED, Json(record)).into_response()
        }
        Err((status, message)) => (
            status,
            Json(json!({ "error": message, "deliveryId": delivery_id })),
        )
            .into_response(),
    }
}

fn registry_secret_ok(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(secret) = state.config.registry_webhook_secret.as_deref() else {
        return false;
    };
    headers
        .get("x-registry-webhook-secret")
        .or_else(|| headers.get("x-webhook-secret"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.as_bytes().ct_eq(secret.as_bytes()).into())
}

/// Normalize either an ECR EventBridge event or a docker distribution v2
/// notification into compact (repository, tag, digest, action) tuples.
fn registry_events(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    // ECR EventBridge: {"detail-type": "ECR Image Action", "detail": {...}}
    if payload
        .get("detail-type")
        .and_then(serde_json::Value::as_str)
        == Some("ECR Image Action")
    {
        let detail = payload.get("detail").cloned().unwrap_or_default();
        return vec![json!({
            "provider": "ecr",
            "action": detail.get("action-type").and_then(serde_json::Value::as_str),
            "result": detail.get("result").and_then(serde_json::Value::as_str),
            "repository": detail.get("repository-name").and_then(serde_json::Value::as_str),
            "tag": detail.get("image-tag").and_then(serde_json::Value::as_str),
            "digest": detail.get("image-digest").and_then(serde_json::Value::as_str),
        })];
    }
    // Docker distribution v2: {"events": [{action, target: {repository, tag, digest}}]}
    payload
        .get("events")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    json!({
                        "provider": "registry",
                        "action": entry.get("action").and_then(serde_json::Value::as_str),
                        "repository": entry
                            .pointer("/target/repository")
                            .and_then(serde_json::Value::as_str),
                        "tag": entry.pointer("/target/tag").and_then(serde_json::Value::as_str),
                        "digest": entry
                            .pointer("/target/digest")
                            .and_then(serde_json::Value::as_str),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn registry_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if state.config.registry_webhook_secret.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "BUILD_SERVER_REGISTRY_WEBHOOK_SECRET is not configured" })),
        )
            .into_response();
    }
    if !registry_secret_ok(&state, &headers) {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "missing or invalid registry webhook secret header" })),
        )
            .into_response();
    }
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid webhook JSON: {error}") })),
            )
                .into_response();
        }
    };

    state
        .counters
        .webhooks_received
        .fetch_add(1, Ordering::Relaxed);
    let events = registry_events(&payload);
    let delivery_id = headers
        .get("x-delivery-id")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("registry-{}", crate::now_ms()));

    if let Some(db) = state.db.as_ref() {
        let repo = events
            .first()
            .and_then(|event| event.get("repository").and_then(serde_json::Value::as_str));
        let fresh = db::record_webhook_delivery(
            db,
            "registry",
            &delivery_id,
            Some("image"),
            repo,
            None,
            &format!("relayed:{}", events.len()),
        )
        .await;
        if !fresh {
            return (
                StatusCode::OK,
                Json(json!({ "ok": true, "action": "duplicate" })),
            )
                .into_response();
        }
    }

    for event in &events {
        events::publish_image_event(
            &state,
            json!({
                "schemaVersion": "build-server.image-event.v1",
                "service": crate::SERVICE_NAME,
                "event": event,
                "tsMs": crate::now_ms() as u64,
            }),
        )
        .await;
    }

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "relayed": events.len() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_signature_verification_is_exact() {
        // echo -n 'hello' | openssl dgst -sha256 -hmac 'secret'
        let signature = "sha256=88aab3ede8d3adf94d26ab90d3bafd4a2083070c3bcce9c014ee04a443847c0b";
        assert!(verify_github_signature("secret", b"hello", signature));
        assert!(!verify_github_signature("secret", b"hello2", signature));
        assert!(!verify_github_signature("wrong", b"hello", signature));
        assert!(!verify_github_signature("secret", b"hello", "sha256=zz"));
        assert!(!verify_github_signature("secret", b"hello", ""));
    }

    #[test]
    fn webhook_rules_match_repo_branch_and_event() {
        let rule: WebhookRule = serde_json::from_value(json!({
            "repo": "ORESoftware/example",
            "branch": "dev",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/example:{shortSha}",
            "push": true
        }))
        .unwrap();
        assert!(rule_matches(
            &rule,
            "push",
            "oresoftware/example",
            "refs/heads/dev"
        ));
        assert!(!rule_matches(
            &rule,
            "push",
            "oresoftware/example",
            "refs/heads/main"
        ));
        assert!(!rule_matches(&rule, "push", "other/repo", "refs/heads/dev"));
        assert!(!rule_matches(
            &rule,
            "workflow_run",
            "oresoftware/example",
            "dev"
        ));

        let request = build_request_from_rule(
            &rule,
            "ORESoftware/example",
            "refs/heads/dev",
            "0123456789abcdef0123",
        );
        assert_eq!(
            request.repo_url,
            "https://github.com/ORESoftware/example.git"
        );
        assert_eq!(request.git_ref.as_deref(), Some("dev"));
        assert!(request.image.ends_with(":0123456789ab"));

        let profile_rules = parse_rules(
            r#"[{"repo":"sonus-auris/sonus-auris-ui.dart","branch":"main","profile":"flutter-android-debug"}]"#,
        )
        .expect("profile rule parses");
        let profile_request = build_request_from_rule(
            &profile_rules[0],
            "sonus-auris/sonus-auris-ui.dart",
            "refs/heads/main",
            "0123456789abcdef0123",
        );
        assert_eq!(profile_request.job_kind.as_deref(), Some("run-profile"));
        assert_eq!(
            profile_request.profile.as_deref(),
            Some("flutter-android-debug")
        );
        assert!(profile_request.image.is_empty());
    }

    #[test]
    fn registry_events_normalize_ecr_and_distribution_payloads() {
        let ecr = json!({
            "detail-type": "ECR Image Action",
            "detail": {
                "action-type": "PUSH",
                "result": "SUCCESS",
                "repository-name": "example",
                "image-tag": "dev",
                "image-digest": "sha256:abc"
            }
        });
        let events = registry_events(&ecr);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["repository"], "example");

        let distribution = json!({
            "events": [
                { "action": "push", "target": { "repository": "a/b", "tag": "v1", "digest": "sha256:def" } }
            ]
        });
        let events = registry_events(&distribution);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["tag"], "v1");
    }
}
