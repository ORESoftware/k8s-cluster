use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::{
    app::AppState,
    data::{game_rows, session_rows, DashboardStats},
};

pub(crate) async fn home_body(state: &AppState) -> Markup {
    let stats = DashboardStats::load(state).await;
    html! {
        section class="hero" {
            img class="hero-mark" src=(state.path("/assets/akrion-emblem.png")) alt="Akrion Sim emblem";
            div class="hero-copy" {
                p class="eyebrow" { "Akrion Sim" }
                h1 { "Akrion Sim" }
                p class="dek" { "Realtime football simulation, live match telemetry, and session-aware team experiments." }
                div class="hero-actions" {
                    a class="button primary" href=(state.path("/portal")) {
                        i data-lucide="layout-dashboard" {}
                        span { "Open Portal" }
                    }
                    a class="button ghost" href=(state.backend_url) {
                        i data-lucide="radio" {}
                        span { "Game Backend" }
                    }
                }
            }
            div class="hero-signal" {
                (status_pill("backend", stats.backend_status))
                div class="signal-line" {
                    span { "ticks/sec" }
                    strong { (stats.sim_ticks_per_second) }
                }
                div class="signal-line" {
                    span { "goals/match" }
                    strong { (format!("{:.2}", stats.goals_per_match)) }
                }
            }
        }
        main class="content-shell" hx-ext="ws" ws-connect=(state.path("/ws/portal")) {
            section class="stat-strip" aria-label="Simulation stats" {
                div class="stat-tile" id="metric-running" {
                    (metric_value(stats.games_running, "running games"))
                }
                div class="stat-tile" id="metric-sessions" {
                    (metric_value(stats.active_sessions, "active sessions"))
                }
                div class="stat-tile" {
                    (metric_value(stats.games_finished_today, "finished today"))
                }
                div class="stat-tile" {
                    (metric_value(stats.sim_ticks_per_second, "ticks/sec"))
                }
            }
            div id="live-ticker" {
                (live_ticker(&stats))
            }
            section class="portal-grid" {
                div class="panel wide" {
                    div class="panel-head" {
                        h2 { "Match Control" }
                        div class="panel-actions" {
                            button class="icon-button" type="button" title="Refresh" hx-get=(state.path("/partials/overview")) hx-target="#home-overview" hx-swap="innerHTML" {
                                i data-lucide="refresh-cw" {}
                            }
                        }
                    }
                    div id="home-overview" {
                        (overview_panel(&stats))
                    }
                }
                div class="panel" {
                    (auth_panel(state))
                }
            }
        }
    }
}

pub(crate) async fn portal_body(state: &AppState) -> Markup {
    html! {
        main class="portal-page" hx-ext="ws" ws-connect=(state.path("/ws/portal")) {
            section class="portal-mast" {
                div {
                    p class="eyebrow" { "User Portal" }
                    h1 { "Sessions, games, and live stats" }
                }
                div id="live-ticker" {
                    (live_ticker(&DashboardStats::load(state).await))
                }
            }

            section class="portal-workspace" {
                aside class="auth-rail" {
                    (auth_panel(state))
                }
                section class="portal-main" {
                    nav class="tabbar" aria-label="Portal views" {
                        button class="tab is-active" type="button" hx-get=(state.path("/partials/overview")) hx-target="#portal-panel" hx-swap="innerHTML" {
                            i data-lucide="activity" {}
                            span { "Overview" }
                        }
                        button class="tab" type="button" hx-get=(state.path("/partials/games")) hx-target="#portal-panel" hx-swap="innerHTML" {
                            i data-lucide="trophy" {}
                            span { "Games" }
                        }
                        button class="tab" type="button" hx-get=(state.path("/partials/sessions")) hx-target="#portal-panel" hx-swap="innerHTML" {
                            i data-lucide="users" {}
                            span { "Sessions" }
                        }
                    }
                    div id="portal-panel" hx-get=(state.path("/partials/overview")) hx-trigger="load" hx-swap="innerHTML" {
                        div class="loading-line" { "Loading portal..." }
                    }
                }
            }
        }
    }
}

pub(crate) fn overview_panel(stats: &DashboardStats) -> Markup {
    html! {
        div class="overview-grid" {
            article class="info-block" {
                div class="info-icon" { i data-lucide="server" {} }
                h3 { "Backend" }
                p { (stats.backend_url) }
                (status_pill("game runtime", stats.backend_status))
            }
            article class="info-block" {
                div class="info-icon" { i data-lucide="timer" {} }
                h3 { "Session Clock" }
                p { (format_duration(stats.uptime_seconds)) }
                span class="muted" { "web portal uptime" }
            }
            article class="info-block" {
                div class="info-icon" { i data-lucide="target" {} }
                h3 { "Finishing" }
                p { (format!("{:.2} goals per match", stats.goals_per_match)) }
                span class="muted" { "rolling sample" }
            }
        }
    }
}

pub(crate) fn games_panel(stats: &DashboardStats) -> Markup {
    html! {
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Game" }
                        th { "State" }
                        th { "Score" }
                        th { "Tick" }
                        th { "Model" }
                    }
                }
                tbody {
                    @for row in game_rows(stats) {
                        tr {
                            td { strong { (row.name) } }
                            td { span class=(format!("badge {}", row.state_class)) { (row.state) } }
                            td { (row.score) }
                            td { (row.tick) }
                            td { (row.model) }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn sessions_panel(stats: &DashboardStats) -> Markup {
    html! {
        div class="session-list" {
            @for row in session_rows(stats) {
                article class="session-item" {
                    div {
                        h3 { (row.name) }
                        p { (row.detail) }
                    }
                    span class=(format!("badge {}", row.state_class)) { (row.state) }
                }
            }
        }
    }
}

pub(crate) fn auth_panel(state: &AppState) -> Markup {
    let readiness = if state.supabase_ready() {
        ("ready", "Supabase ready")
    } else {
        ("offline", "Supabase env missing")
    };
    html! {
        div class="panel-head" {
            h2 { "Supabase Login" }
            (status_pill("auth", readiness.1))
        }
        div class="auth-box" data-auth-ready=(state.supabase_ready().to_string()) {
            div class="auth-user" id="auth-user" {
                i data-lucide="shield-user" {}
                span { "Signed out" }
            }
            form id="auth-form" class="auth-form" {
                label for="auth-email" { "Email" }
                input id="auth-email" name="email" type="email" autocomplete="email" placeholder="captain@akrion.local";
                label for="auth-password" { "Password" }
                input id="auth-password" name="password" type="password" autocomplete="current-password" placeholder="password";
                div class="auth-actions" {
                    button class="button primary" type="submit" data-auth-action="password" {
                        i data-lucide="log-in" {}
                        span { "Sign In" }
                    }
                    button class="button ghost" type="button" data-auth-action="magic" {
                        i data-lucide="mail" {}
                        span { "Magic Link" }
                    }
                    button class="icon-button" type="button" title="Create account" data-auth-action="signup" {
                        i data-lucide="user-plus" {}
                    }
                    button class="icon-button" type="button" title="Sign out" data-auth-action="signout" {
                        i data-lucide="log-out" {}
                    }
                }
                p class=(format!("auth-status {}", readiness.0)) id="auth-status" {
                    (readiness.1)
                }
            }
        }
    }
}

pub(crate) fn render_page(state: &AppState, active: &str, body: Markup) -> Markup {
    let config = serde_json::to_string(&state.public_config()).expect("public config serializes");
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Akrion Sim" }
                script { (PreEscaped(THEME_BOOT_JS)) }
                link rel="preconnect" href="https://unpkg.com";
                link rel="preconnect" href="https://cdn.jsdelivr.net";
                link rel="stylesheet" href=(state.path("/assets/app.css"));
                script src="https://unpkg.com/htmx.org@1.9.12" {}
                script src="https://unpkg.com/htmx.org/dist/ext/ws.js" {}
                script src="https://unpkg.com/lucide@latest" {}
                script src="https://cdn.jsdelivr.net/npm/@supabase/supabase-js@2" {}
                script { (PreEscaped(format!("window.__AKRION_CONFIG__ = {config};"))) }
            }
            body {
                header class="topbar" {
                    a class="brand" href=(state.path("/")) aria-label="Akrion Sim home" {
                        img src=(state.path("/assets/akrion-emblem.png")) alt="";
                        span { "Akrion Sim" }
                    }
                    div class="topbar-actions" {
                        nav aria-label="Main navigation" {
                            a class=(nav_class(active, "home")) href=(state.path("/")) { "Home" }
                            a class=(nav_class(active, "portal")) href=(state.path("/portal")) { "Portal" }
                            a href=(state.path("/config")) { "Config" }
                        }
                        div class="theme-switcher" role="radiogroup" aria-label="Theme" {
                            button class="theme-option" type="button" role="radio" title="Dark theme" aria-label="Dark theme" data-theme-option="dark" {
                                i data-lucide="moon" {}
                            }
                            button class="theme-option" type="button" role="radio" title="Medium theme" aria-label="Medium theme" data-theme-option="medium" {
                                i data-lucide="circle-dot" {}
                            }
                            button class="theme-option" type="button" role="radio" title="Light theme" aria-label="Light theme" data-theme-option="light" {
                                i data-lucide="sun" {}
                            }
                        }
                    }
                }
                (body)
                script src=(state.path("/assets/app.js")) {}
            }
        }
    }
}

const THEME_BOOT_JS: &str = r#"
(() => {
  const allowed = new Set(["dark", "medium", "light"]);
  const stored = localStorage.getItem("akrion-theme");
  const theme = allowed.has(stored) ? stored : "dark";
  document.documentElement.dataset.theme = theme;
})();
"#;

pub(crate) fn live_ticker(stats: &DashboardStats) -> Markup {
    html! {
        div class="ticker" {
            (status_pill("backend", stats.backend_status))
            span { (stats.games_running) " games running" }
            span { (stats.active_sessions) " active sessions" }
            span { (stats.sim_ticks_per_second) " ticks/sec" }
        }
    }
}

pub(crate) fn metric_value(value: u64, label: &str) -> Markup {
    html! {
        strong { (value) }
        span { (label) }
    }
}

fn status_pill(label: &str, status: &str) -> Markup {
    let class = if status.contains("online") || status.contains("ready") {
        "status-pill online"
    } else if status.contains("missing") || status.contains("offline") {
        "status-pill offline"
    } else {
        "status-pill"
    };
    html! {
        span class=(class) {
            span class="dot" {}
            span class="status-label" { (label) }
            strong { (status) }
        }
    }
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

fn nav_class(active: &str, target: &str) -> &'static str {
    if active == target {
        "is-active"
    } else {
        ""
    }
}
