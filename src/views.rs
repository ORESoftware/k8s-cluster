//! Server-rendered UI (Maud). The auth API is JSON; these are the
//! human-facing HTML blobs: a status landing page and a token-exchange helper
//! whose form posts to a normal same-origin endpoint.
//! No websockets — plain HTTP request/response.

use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::token::OreClaims;

const CSS: &str = r#"
  :root { color-scheme: light dark; }
  body { font: 15px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; max-width: 46rem;
         margin: 3rem auto; padding: 0 1rem; }
  h1 { font-size: 1.3rem; } h2 { font-size: 1.05rem; margin-top: 2rem; }
  code, pre { background: color-mix(in srgb, currentColor 8%, transparent); border-radius: 4px; }
  code { padding: .1rem .3rem; } pre { padding: .75rem; overflow-x: auto; }
  textarea { width: 100%; min-height: 5rem; font: inherit; }
  button { font: inherit; padding: .4rem .9rem; cursor: pointer; }
  .ok { color: #2a7; } .err { color: #c33; }
  .muted { opacity: .7; }
  table { border-collapse: collapse; } td { padding: .1rem .6rem .1rem 0; vertical-align: top; }
"#;

/// Page layout. It intentionally has no third-party script dependency.
pub fn page(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · shared-auth" }
                style { (PreEscaped(CSS)) }
            }
            body { (body) }
        }
    }
}

/// GET / — status landing.
pub fn landing(project_count: usize, issuer: &str, db_enabled: bool) -> Markup {
    page(
        "status",
        html! {
            h1 { "shared-auth-server" }
            p class="muted" {
                "Postgres-primary OreSoftware auth with " (project_count)
                " configured Supabase secondary project(s), provider-neutral "
                "identities, passwordless email, SMS MFA, rotated sessions, roles, "
                "and unified access tokens."
            }
            h2 { "status" }
            table {
                tr { td { "issuer" } td { code { (issuer) } } }
                tr { td { "supabase projects" } td { (project_count) } }
                tr { td { "Postgres authority" } td { @if db_enabled { span class="ok" { "ready" } } @else { span class="muted" { "development-only DB-less mode" } } } }
            }
            h2 { "endpoints" }
            table {
                tr { td { code { "POST /auth/exchange" } } td class="muted" { "Supabase token → OreSoftware JWT (JSON)" } }
                tr { td { code { "POST /auth/login" } } td class="muted" { "local credentials → access and refresh tokens" } }
                tr { td { code { "POST /auth/passwordless/request" } } td class="muted" { "SendGrid magic link and email OTP" } }
                tr { td { code { "POST /auth/passwordless/consume" } } td class="muted" { "one-time email credential → AAL1 session" } }
                tr { td { code { "POST /auth/mfa/sms/request" } } td class="muted" { "start a Twilio Verify SMS challenge" } }
                tr { td { code { "POST /auth/mfa/sms/verify" } } td class="muted" { "verify SMS and issue an AAL2 session" } }
                tr { td { code { "POST /auth/refresh" } } td class="muted" { "rotate a one-time refresh token" } }
                tr { td { code { "POST /auth/introspect" } } td class="muted" { "validate an OreSoftware JWT" } }
                tr { td { code { "GET /.well-known/jwks.json" } } td class="muted" { "our public JWKS" } }
                tr { td { code { "GET /ui" } } td class="muted" { "token-exchange helper (this UI)" } }
            }
            p { a href="ui" { "→ token exchange helper" } }
        },
    )
}

/// GET /ui — a helper that posts a Supabase token to /ui/exchange.
pub fn sign_in() -> Markup {
    page(
        "token exchange",
        html! {
            h1 { "token exchange" }
            p class="muted" { "Paste a Supabase access token (from any configured project) to mint a unified OreSoftware JWT." }
            form method="post" action="ui/exchange" {
                textarea name="access_token" placeholder="eyJ… Supabase access token" required {}
                p { button type="submit" { "Exchange" } }
            }
            div id="result" {}
            p class="muted" { a href="." { "← status" } }
        },
    )
}

/// Successful exchange result.
pub fn exchange_result(minted_token: &str, project: &str, claims: &OreClaims) -> Markup {
    html! {
        h2 class="ok" { "✓ exchanged" }
        table {
            tr { td { "project" } td { code { (project) } } }
            tr { td { "shared_user_id" } td { code { (claims.sub) } } }
            @if let Some(email) = &claims.email { tr { td { "email" } td { code { (email) } } } }
            tr { td { "expires" } td { (claims.exp) } }
        }
        h2 { "OreSoftware JWT" }
        pre { code { (minted_token) } }
    }
}

/// Failure response (uniform, no detail).
pub fn error_fragment() -> Markup {
    html! {
        h2 class="err" { "✗ unauthorized" }
        p class="muted" { "The token could not be verified against any configured Supabase project." }
    }
}
