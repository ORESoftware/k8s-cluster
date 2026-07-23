//! Server-rendered Maud views.

use axum::response::Html;
use maud::{html, Markup, PreEscaped, DOCTYPE};

const CSS: &str = r#"
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body {
  margin: 0; min-height: 100vh; display: grid; place-items: center;
  background: #0b1020; color: #e6e9f2;
  font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
}
main { width: min(26rem, 92vw); padding: 2rem 0 3rem; }
.card {
  background: #141a30; border: 1px solid #26304f; border-radius: 12px;
  padding: 1.75rem; box-shadow: 0 8px 30px rgba(0, 0, 0, 0.35);
}
h1 { font-size: 1.35rem; margin: 0 0 0.5rem; }
.brand { letter-spacing: 0.08em; font-weight: 700; color: #7aa2ff; margin: 0 0 1.25rem; }
p.sub { color: #9aa4c0; margin: 0 0 1.25rem; }
label { display: block; font-size: 0.85rem; color: #9aa4c0; margin: 0.75rem 0 0.25rem; }
input {
  width: 100%; padding: 0.6rem 0.7rem; border-radius: 8px;
  border: 1px solid #2c375f; background: #0d1226; color: inherit; font: inherit;
}
button {
  margin-top: 1.1rem; width: 100%; padding: 0.65rem; border: 0; border-radius: 8px;
  background: #3b6cff; color: #fff; font: inherit; font-weight: 600; cursor: pointer;
}
button:hover { background: #2f57d6; }
.error { color: #ff8f8f; margin: 0.75rem 0 0; }
.notice { color: #ffd479; margin: 0.75rem 0 0; }
.success { color: #7fe0a7; margin: 0.75rem 0 0; }
.qr { background: #fff; border-radius: 8px; padding: 0.75rem; margin: 1rem 0; }
.qr svg { display: block; width: 100%; height: auto; }
code.secret {
  display: block; word-break: break-all; background: #0d1226; border-radius: 8px;
  padding: 0.6rem 0.7rem; font-size: 0.85rem; color: #b7c2e0;
}
"#;

pub(crate) fn page(title: &str, body: Markup) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " — 3FA" }
                    style { (PreEscaped(CSS)) }
                    script defer="defer" src="https://unpkg.com/htmx.org@2.0.4" {}
                }
                body {
                    main {
                        p class="brand" { "3FA" }
                        (body)
                    }
                }
            }
        }
        .into_string(),
    )
}

pub(crate) fn login_form(error: Option<&str>, configured: bool) -> Markup {
    html! {
        div class="card" id="login-box" {
            h1 { "Sign in to 3FA" }
            p class="sub" { "Use your 3FA account to open the web authenticator." }
            form hx-post="/login" hx-target="#login-box" hx-swap="outerHTML" {
                label for="email" { "Email" }
                input id="email" name="email" type="email" required autocomplete="username";
                label for="password" { "Password" }
                input id="password" name="password" type="password" required autocomplete="current-password";
                button type="submit" { "Sign in" }
            }
            @if let Some(error) = error {
                p class="error" { (error) }
            }
            @if !configured {
                p class="notice" {
                    "Authentication not configured — set the Supabase provider and SHARED_AUTH_BASE_URL to enable sign-in."
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_form_reflects_configuration_without_exposing_values() {
        let unconfigured = login_form(None, false).into_string();
        assert!(unconfigured.contains("Authentication not configured"));
        let configured = login_form(None, true).into_string();
        assert!(!configured.contains("Authentication not configured"));
        assert!(configured.contains(r#"autocomplete="current-password""#));
    }

    #[test]
    fn dynamic_view_content_is_html_escaped() {
        let form = login_form(Some("<script>unsafe()</script>"), true).into_string();
        assert!(!form.contains("<script>unsafe()</script>"));
        assert!(form.contains("&lt;script&gt;unsafe()&lt;/script&gt;"));

        let document = page("<unsafe>", html! { p { "body" } }).0;
        assert!(document.contains("&lt;unsafe&gt; — 3FA"));
    }
}
