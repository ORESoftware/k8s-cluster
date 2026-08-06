use axum::{response::Html, Json};
use serde_json::{json, Value};

pub async fn openapi() -> Json<Value> {
    Json(document())
}

pub async fn api_docs() -> Html<String> {
    Html(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>shared-auth API</title><style>body{font:15px/1.5 ui-monospace,monospace;max-width:52rem;margin:3rem auto;padding:0 1rem}code{background:#8882;padding:.1rem .3rem}li{margin:.5rem 0}</style></head><body><h1>shared-auth API</h1><p>Postgres-primary, provider-neutral authentication. The machine-readable OpenAPI contract is at <a href="/api/docs.json"><code>/api/docs.json</code></a>.</p><ul><li><code>POST /auth/register</code> — optional local registration</li><li><code>POST /auth/login</code> — local login</li><li><code>POST /auth/passwordless/request</code> — send a SendGrid magic link and email OTP</li><li><code>POST /auth/passwordless/consume</code> — exchange a link token or email OTP</li><li><code>POST /auth/mfa/sms/request</code> — start Twilio Verify SMS enrollment/challenge</li><li><code>POST /auth/mfa/sms/verify</code> — verify SMS and issue an AAL2 session</li><li><code>POST /auth/exchange</code> — exchange a secondary-provider token</li><li><code>POST /auth/delegate</code> — exchange a user token for a configured audience/scope-limited product token</li><li><code>POST /auth/refresh</code> — rotate a refresh token</li><li><code>POST /auth/logout</code> — revoke a refresh session</li><li><code>POST /auth/introspect</code> — inspect a shared-auth or delegated access token</li><li><code>GET /.well-known/jwks.json</code> — public ES256 keys</li></ul></body></html>"#.to_owned(),
    )
}

fn document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "shared-auth API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Postgres-primary authentication with passwordless email, SMS MFA, provider adapters, rotated sessions, ES256 access tokens, and fail-closed product delegation."
        },
        "paths": {
            "/auth/register": { "post": { "summary": "Register a local principal", "responses": { "200": { "description": "Token pair" }, "403": { "description": "Registration disabled" } } } },
            "/auth/login": { "post": { "summary": "Authenticate local credentials", "responses": { "200": { "description": "Token pair" }, "401": { "description": "Uniform authentication failure" } } } },
            "/auth/passwordless/request": { "post": { "summary": "Send a magic link and email OTP through SendGrid", "responses": { "202": { "description": "Enumeration-resistant acceptance" }, "503": { "description": "Passwordless email is not configured" } } } },
            "/auth/passwordless/consume": { "post": { "summary": "Consume a one-time link token or six-digit email OTP", "responses": { "200": { "description": "AAL1 shared-auth token pair" }, "401": { "description": "Invalid, expired, or consumed credential" } } } },
            "/auth/browser/consume": { "get": { "summary": "Consume a first-party magic link and establish the product-scoped browser session", "responses": { "303": { "description": "Browser session established and redirected to the sealed return path" }, "401": { "description": "Invalid, expired, or consumed magic link" } } } },
            "/auth/browser/otp": { "post": { "summary": "Consume an email OTP and establish the product-scoped browser session", "responses": { "303": { "description": "Browser session established and redirected to the sealed return path" }, "401": { "description": "Invalid, expired, or consumed email OTP" } } } },
            "/auth/mfa/sms/request": { "post": { "summary": "Start a Twilio Verify SMS challenge for an authenticated user", "responses": { "202": { "description": "Challenge started" }, "401": { "description": "A valid shared-auth bearer token is required" }, "503": { "description": "Twilio Verify is not configured" } } } },
            "/auth/mfa/sms/verify": { "post": { "summary": "Verify an SMS code and upgrade to AAL2", "responses": { "200": { "description": "AAL2 shared-auth token pair" }, "401": { "description": "Invalid bearer token or SMS code" } } } },
            "/auth/capabilities": { "get": { "summary": "Discover the configured MFA methods and client integration schemes", "responses": { "200": { "description": "Configured email OTP, SMS OTP, TOTP, and passkey capabilities" } } } },
            "/auth/factors": { "get": { "summary": "List the authenticated principal's enrolled MFA factors", "responses": { "200": { "description": "Enrolled factor metadata without TOTP seeds or biometric material" }, "401": { "description": "A valid active shared-auth bearer token is required" }, "503": { "description": "Durable factor storage is unavailable" } } } },
            "/auth/exchange": { "post": { "summary": "Exchange a secondary-provider bearer token", "responses": { "200": { "description": "Shared-auth token pair" }, "401": { "description": "Uniform authentication failure" } } } },
            "/auth/delegate": { "post": { "summary": "Mint a short-lived configured audience/scope-limited product token", "description": "Uses the current shared-auth bearer as the subject credential. The allow-list is configured with AUTH_DELEGATION_POLICIES. Sensitive scopes can require recent LOA2 assurance; no factor application is called directly.", "responses": { "200": { "description": "Delegated bearer with a new jti, inherited session revocation, and preserved assurance provenance" }, "401": { "description": "Invalid or inactive subject token" }, "403": { "description": "Client, audience, scope, role, or assurance policy denied the exchange" } } } },
            "/auth/refresh": { "post": { "summary": "Atomically rotate a refresh token", "responses": { "200": { "description": "Rotated token pair" }, "401": { "description": "Invalid, expired, revoked, or replayed token" } } } },
            "/auth/logout": { "post": { "summary": "Revoke a refresh session", "responses": { "204": { "description": "Revoked or already absent" } } } },
            "/auth/introspect": { "post": { "summary": "Inspect a shared-auth or exact-audience delegated access token", "responses": { "200": { "description": "Token activity, audience, scope, delegation provenance, session, and authentication assurance" } } } },
            "/auth/verify": { "get": { "summary": "Bearer check for gateway auth_request", "responses": { "200": { "description": "Token accepted" }, "401": { "description": "Token rejected" } } } },
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
            "/auth/browser/consume",
            "/auth/browser/otp",
            "/auth/mfa/sms/request",
            "/auth/mfa/sms/verify",
            "/auth/capabilities",
            "/auth/factors",
            "/auth/exchange",
            "/auth/delegate",
            "/auth/refresh",
            "/auth/introspect",
            "/.well-known/jwks.json",
        ] {
            assert!(value["paths"].get(path).is_some(), "missing {path}");
        }
    }

    /// Every public route must appear in the OpenAPI document.
    ///
    /// Client SDKs are written against this contract, and they had drifted
    /// badly: eleven documented endpoints did not exist on the server, while
    /// the whole passwordless/SMS-MFA surface that does exist had no client
    /// coverage at all. Nothing detected it because no test compared the two.
    /// This reads the router source so a new `.route(...)` cannot ship
    /// undocumented.
    #[test]
    fn openapi_documents_every_public_route() {
        // Operational and browser-facing surfaces are deliberately excluded:
        // they are not part of the SDK contract.
        const NOT_PUBLIC_API: [&str; 8] = [
            "/",
            "/ui",
            "/ui/exchange",
            "/docs/api",
            "/api/docs",
            "/metrics",
            "/internal/webhook/sync",
            "/api/docs.json",
        ];

        let router_source = include_str!("mod.rs");
        let documented = document();
        let mut undocumented = Vec::new();

        for line in router_source.lines() {
            let Some(rest) = line.trim().strip_prefix(".route(\"") else {
                continue;
            };
            let Some(path) = rest.split('"').next() else {
                continue;
            };
            if NOT_PUBLIC_API.contains(&path) {
                continue;
            }
            if documented["paths"].get(path).is_none() {
                undocumented.push(path.to_owned());
            }
        }

        assert!(
            undocumented.is_empty(),
            "routes missing from the OpenAPI contract: {undocumented:?}"
        );
    }

    /// The inverse: the document must not promise routes that do not exist.
    #[test]
    fn openapi_documents_no_route_the_router_lacks() {
        let router_source = include_str!("mod.rs");
        let documented = document();
        let paths = documented["paths"].as_object().expect("paths object");

        let phantom = paths
            .keys()
            .filter(|path| path.as_str() != "/api/docs.json")
            .filter(|path| !router_source.contains(&format!(".route(\"{path}\"")))
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            phantom.is_empty(),
            "OpenAPI promises routes the server does not serve: {phantom:?}"
        );
    }
}
