use axum::{response::Html, Json};
use serde_json::{json, Value};

pub async fn openapi() -> Json<Value> {
    Json(document())
}

pub async fn api_docs() -> Html<String> {
    Html(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>shared-auth API</title><style>body{font:15px/1.5 ui-monospace,monospace;max-width:58rem;margin:3rem auto;padding:0 1rem}code{background:#8882;padding:.1rem .3rem}li{margin:.5rem 0}</style></head><body><h1>shared-auth API</h1><p>Postgres-primary, provider-neutral authentication. The machine-readable OpenAPI contract is at <a href="/api/docs.json"><code>/api/docs.json</code></a>.</p><ul><li><code>POST /auth/register</code> — optional local registration</li><li><code>POST /auth/login</code> — local login</li><li><code>POST /auth/passwordless/request</code> — send a SendGrid magic link and email OTP</li><li><code>POST /auth/passwordless/consume</code> — exchange a link token or email OTP</li><li><code>POST /auth/mfa/sms/request</code> — start Twilio Verify SMS enrollment/challenge</li><li><code>POST /auth/mfa/sms/verify</code> — verify SMS and issue an AAL2 session</li><li><code>GET /auth/recovery/capabilities</code> — read recovery policy and consent version</li><li><code>POST /auth/recovery/enrollment</code> — AAL2 government-ID, face, and voice enrollment</li><li><code>POST /auth/recovery/ceremonies</code> — start an enumeration-resistant recovery ceremony</li><li><code>POST /auth/recovery/ceremonies/{ceremonyId}/complete</code> — normalize provider decisions</li><li><code>POST /auth/recovery/ceremonies/{ceremonyId}/redeem</code> — reset password after approval and cooldown</li><li><code>POST /auth/exchange</code> — exchange a secondary-provider token</li><li><code>POST /auth/refresh</code> — rotate a refresh token</li><li><code>POST /auth/logout</code> — revoke a refresh session</li><li><code>POST /auth/introspect</code> — inspect a shared-auth access token</li><li><code>GET /.well-known/jwks.json</code> — public ES256 keys</li></ul><p>Biometric media is captured only on short-lived provider pages. Shared-auth stores no government-ID images, face templates, voice audio, or speaker embeddings. Voice speaker comparison is advisory only.</p></body></html>"#.to_owned(),
    )
}

fn document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "shared-auth API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Postgres-primary authentication with passwordless email, MFA, provider adapters, rotated sessions, and privacy-preserving government-ID/face/Voxletra account recovery. Biometric media is never accepted by this API; voice speaker comparison is advisory only."
        },
        "paths": {
            "/auth/register": { "post": { "summary": "Register a local principal", "responses": { "200": { "description": "Token pair" }, "403": { "description": "Registration disabled" } } } },
            "/auth/login": { "post": { "summary": "Authenticate local credentials", "responses": { "200": { "description": "Token pair" }, "401": { "description": "Uniform authentication failure" } } } },
            "/auth/passwordless/request": { "post": { "summary": "Send a magic link and email OTP through SendGrid", "responses": { "202": { "description": "Enumeration-resistant acceptance" }, "503": { "description": "Passwordless email is not configured" } } } },
            "/auth/passwordless/consume": { "post": { "summary": "Consume a one-time link token or six-digit email OTP", "responses": { "200": { "description": "AAL1 shared-auth token pair" }, "401": { "description": "Invalid, expired, or consumed credential" } } } },
            "/auth/mfa/sms/request": { "post": { "summary": "Start a Twilio Verify SMS challenge for an authenticated user", "responses": { "202": { "description": "Challenge started" }, "401": { "description": "A valid shared-auth bearer token is required" }, "503": { "description": "Twilio Verify is not configured" } } } },
            "/auth/mfa/sms/verify": { "post": { "summary": "Verify an SMS code and upgrade to AAL2", "responses": { "200": { "description": "AAL2 shared-auth token pair" }, "401": { "description": "Invalid bearer token or SMS code" } } } },
            "/auth/recovery/capabilities": { "get": { "summary": "Read recovery policy, consent version, and availability", "responses": { "200": { "description": "Recovery capabilities" } } } },
            "/auth/recovery/enrollment": {
                "post": { "summary": "Begin AAL2 biometric-recovery enrollment", "responses": { "201": { "description": "Short-lived identity and voice capture sessions" }, "401": { "description": "Authentication required" }, "403": { "description": "AAL2 or current consent required" }, "503": { "description": "Recovery providers are not configured" } } },
                "delete": { "summary": "Revoke biometric-recovery enrollment and active ceremonies", "responses": { "204": { "description": "Revoked" }, "401": { "description": "Authentication required" }, "403": { "description": "AAL2 required" } } }
            },
            "/auth/recovery/enrollment/{ceremonyId}/complete": { "post": { "summary": "Evaluate enrollment providers and persist opaque references", "responses": { "200": { "description": "Coarse ceremony status" }, "401": { "description": "Invalid bearer or ceremony token" } } } },
            "/auth/recovery/ceremonies": { "post": { "summary": "Begin enumeration-resistant account recovery", "responses": { "202": { "description": "Uniform short-lived capture sessions" }, "429": { "description": "Per-identifier limit exceeded" }, "503": { "description": "Recovery providers are not configured" } } } },
            "/auth/recovery/ceremonies/{ceremonyId}/status": { "post": { "summary": "Read coarse recovery status without putting the token in a URL", "responses": { "200": { "description": "pending, pending_review, cooldown, ready, rejected, expired, or consumed" }, "401": { "description": "Invalid ceremony token" } } } },
            "/auth/recovery/ceremonies/{ceremonyId}/complete": { "post": { "summary": "Poll and normalize identity, face, and voice provider decisions", "responses": { "200": { "description": "Coarse recovery status" }, "401": { "description": "Invalid ceremony token" }, "429": { "description": "Evaluation limit exceeded" } } } },
            "/auth/recovery/ceremonies/{ceremonyId}/redeem": { "post": { "summary": "Reset password after approval and cooldown; revoke all sessions", "responses": { "204": { "description": "Password reset; no session issued" }, "409": { "description": "Not ready, expired, or already consumed" } } } },
            "/auth/exchange": { "post": { "summary": "Exchange a secondary-provider bearer token", "responses": { "200": { "description": "Shared-auth token pair" }, "401": { "description": "Uniform authentication failure" } } } },
            "/auth/refresh": { "post": { "summary": "Atomically rotate a refresh token", "responses": { "200": { "description": "Rotated token pair" }, "401": { "description": "Invalid, expired, revoked, or replayed token" } } } },
            "/auth/logout": { "post": { "summary": "Revoke a refresh session", "responses": { "204": { "description": "Revoked or already absent" } } } },
            "/auth/introspect": { "post": { "summary": "Inspect a shared-auth access token", "responses": { "200": { "description": "Token activity and provider provenance" } } } },
            "/.well-known/jwks.json": { "get": { "summary": "Read public ES256 signing keys", "responses": { "200": { "description": "JWKS" } } } },
            "/healthz": { "get": { "summary": "Liveness", "responses": { "200": { "description": "Alive" } } } },
            "/readyz": { "get": { "summary": "Postgres-aware readiness", "responses": { "200": { "description": "Ready" }, "503": { "description": "Not ready" } } } }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_has_stable_auth_and_recovery_paths() {
        let value = document();
        assert_eq!(value["openapi"], "3.1.0");
        for path in [
            "/auth/login",
            "/auth/passwordless/request",
            "/auth/passwordless/consume",
            "/auth/mfa/sms/request",
            "/auth/mfa/sms/verify",
            "/auth/recovery/capabilities",
            "/auth/recovery/enrollment",
            "/auth/recovery/enrollment/{ceremonyId}/complete",
            "/auth/recovery/ceremonies",
            "/auth/recovery/ceremonies/{ceremonyId}/status",
            "/auth/recovery/ceremonies/{ceremonyId}/complete",
            "/auth/recovery/ceremonies/{ceremonyId}/redeem",
            "/auth/exchange",
            "/auth/refresh",
            "/.well-known/jwks.json",
        ] {
            assert!(value["paths"].get(path).is_some(), "missing {path}");
        }
    }
}
