use axum::{response::Html, Json};
use serde_json::{json, Value};

pub async fn openapi() -> Json<Value> {
    Json(document())
}

pub async fn api_docs() -> Html<String> {
    Html(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>shared-auth API</title><style>body{font:15px/1.5 ui-monospace,monospace;max-width:52rem;margin:3rem auto;padding:0 1rem}code{background:#8882;padding:.1rem .3rem}li{margin:.5rem 0}</style></head><body><h1>shared-auth API</h1><p>Postgres-primary, provider-neutral authentication. The machine-readable OpenAPI contract is at <a href="/api/docs.json"><code>/api/docs.json</code></a>.</p><ul><li><code>POST /auth/register</code> — optional local registration</li><li><code>POST /auth/login</code> — local login</li><li><code>POST /auth/passwordless/request</code> — send a SendGrid magic link and email OTP</li><li><code>POST /auth/passwordless/consume</code> — exchange a link token or email OTP</li><li><code>POST /auth/mfa/sms/request</code> — start Twilio Verify SMS enrollment/challenge</li><li><code>POST /auth/mfa/sms/verify</code> — verify SMS and issue an AAL2 session</li><li><code>POST /auth/exchange</code> — exchange a secondary-provider token</li><li><code>POST /auth/refresh</code> — rotate a refresh token</li><li><code>POST /auth/logout</code> — revoke a refresh session</li><li><code>POST /auth/introspect</code> — inspect a shared-auth access token</li><li><code>GET /.well-known/jwks.json</code> — public ES256 keys</li></ul></body></html>"#.to_owned(),
    )
}

fn document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "shared-auth API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Postgres-primary authentication with passwordless email, SMS MFA, provider adapters, rotated sessions, and ES256 access tokens."
        },
        "paths": {
            "/auth/register": { "post": { "summary": "Register a local principal", "responses": { "200": { "description": "Token pair" }, "403": { "description": "Registration disabled" } } } },
            "/auth/login": { "post": { "summary": "Authenticate local credentials", "responses": { "200": { "description": "Token pair" }, "401": { "description": "Uniform authentication failure" } } } },
            "/auth/passwordless/request": { "post": { "summary": "Send a magic link and email OTP through SendGrid", "responses": { "202": { "description": "Enumeration-resistant acceptance" }, "503": { "description": "Passwordless email is not configured" } } } },
            "/auth/passwordless/consume": { "post": { "summary": "Consume a one-time link token or six-digit email OTP", "responses": { "200": { "description": "AAL1 shared-auth token pair" }, "401": { "description": "Invalid, expired, or consumed credential" } } } },
            "/auth/mfa/sms/request": { "post": { "summary": "Start a Twilio Verify SMS challenge for an authenticated user", "responses": { "202": { "description": "Challenge started" }, "401": { "description": "A valid shared-auth bearer token is required" }, "503": { "description": "Twilio Verify is not configured" } } } },
            "/auth/mfa/sms/verify": { "post": { "summary": "Verify an SMS code and upgrade to AAL2", "responses": { "200": { "description": "AAL2 shared-auth token pair" }, "401": { "description": "Invalid bearer token or SMS code" } } } },
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
    fn openapi_has_stable_auth_paths() {
        let value = document();
        assert_eq!(value["openapi"], "3.1.0");
        for path in [
            "/auth/login",
            "/auth/passwordless/request",
            "/auth/passwordless/consume",
            "/auth/mfa/sms/request",
            "/auth/mfa/sms/verify",
            "/auth/exchange",
            "/auth/refresh",
            "/.well-known/jwks.json",
        ] {
            assert!(value["paths"].get(path).is_some(), "missing {path}");
        }
    }
}
