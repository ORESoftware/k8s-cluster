use crate::auth::AuthContext;
use maud::{html, Markup, DOCTYPE};

fn layout(title: &str, body: Markup, csrf: Option<&str>, account_key: Option<&str>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                @if let Some(csrf) = csrf {
                    meta name="csrf-token" content=(csrf);
                }
                @if let Some(account_key) = account_key {
                    meta name="canonical-account-key" content=(account_key);
                }
                title { (title) " · canonical.cloud" }
                style {
                    "body{font-family:ui-sans-serif,system-ui,sans-serif;max-width:72rem;margin:0 auto;padding:2rem;line-height:1.5}nav{display:flex;justify-content:space-between;align-items:center}main{margin-top:3rem}.card{border:1px solid #8886;border-radius:.75rem;padding:1.25rem;margin:1rem 0}label{display:block;margin:.75rem 0}input,textarea,button{font:inherit;padding:.65rem}input,textarea{box-sizing:border-box;width:100%}button{cursor:pointer}.muted{opacity:.7}.error{color:#b42318}#sync-status[data-state=offline]{color:#b54708}#sync-status[data-state=synced]{color:#067647}"
                }
                script type="module" src="/app-assets/app.js" {}
            }
            body {
                nav {
                    a href="/" { strong { "canonical.cloud" } }
                    a href="/app" { "Application" }
                }
                (body)
            }
        }
    }
}

pub fn login(csrf: &str, error: Option<&str>) -> Markup {
    layout(
        "Sign in",
        html! {
            main {
                h1 { "Sign in" }
                p class="muted" { "Authentication is handled by Supabase; credentials are never stored by this server." }
                @if let Some(error) = error {
                    p class="error" role="alert" { (error) }
                }
                form method="post" action="/auth/login" hx-post="/auth/login" hx-target="#login-result" hx-swap="innerHTML" {
                    input type="hidden" name="csrf" value=(csrf);
                    label { "Email" input type="email" name="email" autocomplete="email" required; }
                    label { "Password" input type="password" name="password" autocomplete="current-password" required; }
                    button type="submit" { "Sign in" }
                }
                div id="login-result" aria-live="polite" {}
            }
        },
        None,
        None,
    )
}

pub fn login_error(message: &str) -> Markup {
    html! {
        p class="error" role="alert" { (message) }
    }
}

pub fn dashboard(actor: &AuthContext) -> Markup {
    let account_key = actor.user_id.to_string();
    layout(
        "Application",
        html! {
            main data-sync-root="draft_note" hx-ext="ws" ws-connect="/ws" {
                header {
                    h1 { "Application" }
                    p { "Signed in as " (actor.email) }
                    p id="sync-status" class="muted" data-state="starting" aria-live="polite" { "Starting offline sync…" }
                    form method="post" action="/auth/logout" {
                        input type="hidden" name="csrf" value=(actor.csrf_token.as_deref().unwrap_or_default());
                        button type="submit" { "Sign out" }
                    }
                }
                section class="card" {
                    h2 { "Optimistic draft note" }
                    p class="muted" { "Edits are committed to IndexedDB first, then reconciled through the REST API. WebSockets only wake the pull loop." }
                    form data-sync-form="draft_note" {
                        input type="hidden" name="id";
                        label { "Title" input name="title" required; }
                        label { "Body" textarea name="body" rows="8" {} }
                        button type="submit" { "Save locally" }
                    }
                    div data-sync-list="draft_note" {}
                    div data-sync-conflicts="draft_note" {}
                }
                section class="card" hx-get="/app/fragments/session" hx-trigger="load" hx-swap="innerHTML" {
                    p class="muted" { "Loading server session fragment…" }
                }
            }
        },
        actor.csrf_token.as_deref(),
        Some(&account_key),
    )
}

pub fn session_fragment(actor: &AuthContext) -> Markup {
    html! {
        div id="session-fragment" {
            strong { "HTMX connected" }
            p class="muted" { "Maud rendered this fragment for user " (actor.user_id) "." }
        }
    }
}

pub fn html_not_found() -> Markup {
    layout(
        "Not found",
        html! { main { h1 { "Not found" } p { "That application page does not exist." } } },
        None,
        None,
    )
}
