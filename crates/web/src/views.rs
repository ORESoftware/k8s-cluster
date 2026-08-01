//! maud templates. Server-rendered HTML with htmx for interactivity and a
//! websocket (`hx-ext="ws"`) for the live stats ticker.

use maud::{html, Markup, DOCTYPE};
use t2v_entity::{translation, vapi_call};

/// Live counters rendered both on first paint and pushed over the websocket.
#[derive(Debug, Clone, Copy, Default)]
pub struct DashboardStats {
    pub transcriptions: u64,
    pub translations: u64,
    pub syntheses: u64,
    pub vapi_calls: u64,
}

// htmx is vendored and served from our own origin (see assets.rs) so the page
// needs no external CDN and can run under a strict `script-src 'self'` CSP.
const HTMX: &str = "/assets/htmx.min.js";
const HTMX_WS: &str = "/assets/htmx-ws.js";
const APP_CSS: &str = "/assets/app.css";

pub fn layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href=(APP_CSS);
                script src=(HTMX) {}
                script src=(HTMX_WS) {}
            }
            body {
                header .topbar {
                    span .brand { "t2v" span .brand-dim { "·v2t" } }
                    nav {
                        a href="/" { "Dashboard" }
                        a href="/translate" { "Translate" }
                        a href="/speak" { "Text to Speech" }
                        a href="/history" { "History" }
                    }
                }
                main .shell { (body) }
                footer .foot {
                    "voice-to-text / text-to-voice · custom FFT DSP · Vapi.ai · SeaORM"
                }
            }
        }
    }
}

/// A live metric card. `oob` puts `hx-swap-oob="true"` on the card itself so
/// the websocket can replace it by id — the card element carries the marker
/// directly rather than being wrapped in a second element with the same id
/// (which would leave two `#id` nodes in the DOM after each swap).
pub fn metric_card(id: &str, value: u64, label: &str, oob: bool) -> Markup {
    html! {
        div .card id=(id) hx-swap-oob=[oob.then_some("true")] {
            div .metric { (value) }
            div .metric-label { (label) }
        }
    }
}

/// The dashboard body. The stat row is wrapped in a ws-connected container;
/// the server pushes out-of-band swaps to each card by id.
pub fn dashboard(stats: &DashboardStats) -> Markup {
    layout(
        "t2v · Dashboard",
        html! {
            section .hero {
                h1 { "Voice ⇄ Text, translated." }
                p .lede {
                    "Speech-to-text, text-to-speech, and AI translation across OpenAI, "
                    "Gemini, and Anthropic — with a hand-rolled FFT DSP core and Vapi.ai "
                    "telephony. This dashboard reads the live "
                    code { "t2v" }
                    " Postgres namespace."
                }
            }
            div hx-ext="ws" ws-connect="/ws/stats" {
                section .cards {
                    (metric_card("stat-transcriptions", stats.transcriptions, "transcriptions", false))
                    (metric_card("stat-translations", stats.translations, "translations", false))
                    (metric_card("stat-syntheses", stats.syntheses, "syntheses", false))
                    (metric_card("stat-vapi", stats.vapi_calls, "vapi calls", false))
                }
                p .ticker id="live-ticker" { "live · streaming over websocket" }
            }
            section .grid-two {
                (translate_widget())
                (tts_widget())
            }
        },
    )
}

/// The translate form. htmx posts to /translate and swaps the result panel.
pub fn translate_widget() -> Markup {
    html! {
        div .panel {
            h2 { "Quick translate" }
            form hx-post="/translate" hx-target="#translate-result" hx-swap="innerHTML" hx-disabled-elt="button" {
                textarea name="text" rows="3" placeholder="Text to translate…" required {}
                div .row {
                    input type="text" name="target_lang" placeholder="Target language (e.g. Spanish)" required;
                    select name="provider" {
                        option value="openai" { "OpenAI" }
                        option value="gemini" { "Gemini" }
                        option value="anthropic" { "Anthropic" }
                    }
                }
                button type="submit" { "Translate" }
            }
            div .result id="translate-result" {}
        }
    }
}

pub fn tts_widget() -> Markup {
    html! {
        div .panel {
            h2 { "Text to speech" }
            form hx-post="/speak" hx-target="#tts-result" hx-swap="innerHTML" hx-disabled-elt="button" {
                textarea name="text" rows="3" placeholder="Text to synthesize…" required {}
                div .row {
                    input type="text" name="voice" placeholder="Voice (optional, e.g. alloy)";
                }
                button type="submit" { "Synthesize" }
            }
            div .result id="tts-result" {}
        }
    }
}

/// Fragment returned by POST /translate.
pub fn translate_result(translated: &str, provider: &str, model: &str, latency_ms: i64) -> Markup {
    html! {
        div .ok-result {
            div .result-text { (translated) }
            div .result-meta { (provider) " · " (model) " · " (latency_ms) " ms" }
        }
    }
}

/// Fragment returned by POST /speak. The audio is a data: URL so no extra
/// round-trip or storage is needed.
pub fn tts_result(data_url: &str, voice: &str, bytes: usize) -> Markup {
    html! {
        div .ok-result {
            audio controls src=(data_url) {}
            div .result-meta { "voice " (voice) " · " (bytes) " bytes" }
        }
    }
}

pub fn error_fragment(message: &str) -> Markup {
    html! {
        div .err-result { (message) }
    }
}

/// Standalone translate page (same widget, full-width).
pub fn translate_page() -> Markup {
    layout(
        "t2v · Translate",
        html! {
            section .hero { h1 { "Translate" } }
            div .single { (translate_widget()) }
        },
    )
}

pub fn speak_page() -> Markup {
    layout(
        "t2v · Text to Speech",
        html! {
            section .hero { h1 { "Text to Speech" } }
            div .single { (tts_widget()) }
        },
    )
}

/// History page: recent translations and Vapi calls read straight from the DB.
pub fn history_page(translations: &[translation::Model], calls: &[vapi_call::Model]) -> Markup {
    layout(
        "t2v · History",
        html! {
            section .hero { h1 { "History" } }
            section .panel {
                h2 { "Recent translations" }
                @if translations.is_empty() {
                    p .muted { "No translations yet." }
                } @else {
                    table {
                        thead { tr { th { "When" } th { "Target" } th { "Provider" } th { "Translation" } } }
                        tbody {
                            @for t in translations {
                                tr {
                                    td .nowrap { (t.created_at.format("%Y-%m-%d %H:%M").to_string()) }
                                    td { (t.target_lang) }
                                    td { (t.provider) }
                                    td .clip { (t.translated_text) }
                                }
                            }
                        }
                    }
                }
            }
            section .panel {
                h2 { "Recent Vapi calls" }
                @if calls.is_empty() {
                    p .muted { "No calls yet." }
                } @else {
                    table {
                        thead { tr { th { "When" } th { "Call" } th { "Status" } th { "Ended reason" } } }
                        tbody {
                            @for c in calls {
                                tr {
                                    td .nowrap { (c.updated_at.format("%Y-%m-%d %H:%M").to_string()) }
                                    td .clip { (c.vapi_call_id) }
                                    td { (c.status) }
                                    td { (c.ended_reason.clone().unwrap_or_default()) }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub const STYLE: &str = r#"
:root { color-scheme: light dark; --bg:#0f1220; --panel:#181c2e; --card:#1e2338; --ink:#e7e9f3; --dim:#9aa0bf; --accent:#7c5cff; --ok:#3ad29f; --err:#ff6b6b; --line:#2a3050; }
* { box-sizing: border-box; }
body { margin:0; font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif; background:var(--bg); color:var(--ink); }
.topbar { display:flex; align-items:center; justify-content:space-between; padding:14px 24px; border-bottom:1px solid var(--line); position:sticky; top:0; background:rgba(15,18,32,.9); backdrop-filter: blur(8px); }
.brand { font-weight:800; font-size:20px; letter-spacing:.5px; }
.brand-dim { color:var(--dim); font-weight:600; }
nav a { color:var(--dim); text-decoration:none; margin-left:18px; font-weight:600; }
nav a:hover { color:var(--ink); }
.shell { max-width:1040px; margin:0 auto; padding:28px 24px 64px; }
.hero h1 { font-size:34px; margin:12px 0 6px; }
.lede { color:var(--dim); max-width:70ch; line-height:1.5; }
.cards { display:grid; grid-template-columns: repeat(4,1fr); gap:16px; margin:24px 0 8px; }
.card { background:var(--card); border:1px solid var(--line); border-radius:14px; padding:18px; }
.metric { font-size:34px; font-weight:800; }
.metric-label { color:var(--dim); font-size:13px; text-transform:uppercase; letter-spacing:.6px; }
.ticker { color:var(--ok); font-size:13px; margin:6px 2px 26px; }
.grid-two { display:grid; grid-template-columns: 1fr 1fr; gap:18px; }
.single { max-width:640px; }
.panel { background:var(--panel); border:1px solid var(--line); border-radius:16px; padding:20px; margin-bottom:18px; }
.panel h2 { margin:0 0 14px; font-size:18px; }
textarea, input, select { width:100%; background:var(--bg); color:var(--ink); border:1px solid var(--line); border-radius:10px; padding:10px 12px; font:inherit; margin-bottom:10px; }
.row { display:flex; gap:10px; }
.row > * { margin-bottom:10px; }
button { background:var(--accent); color:white; border:0; border-radius:10px; padding:11px 18px; font-weight:700; cursor:pointer; }
button:hover { filter:brightness(1.08); }
button:disabled { opacity:.6; cursor:progress; }
.result { margin-top:12px; }
.ok-result { border-left:3px solid var(--ok); padding:10px 14px; background:rgba(58,210,159,.06); border-radius:8px; }
.err-result { border-left:3px solid var(--err); padding:10px 14px; background:rgba(255,107,107,.08); border-radius:8px; color:#ffd7d7; }
.result-text { font-size:16px; line-height:1.5; }
.result-meta { color:var(--dim); font-size:12px; margin-top:8px; }
audio { width:100%; }
table { width:100%; border-collapse:collapse; }
th, td { text-align:left; padding:9px 10px; border-bottom:1px solid var(--line); font-size:14px; }
th { color:var(--dim); font-weight:600; font-size:12px; text-transform:uppercase; letter-spacing:.5px; }
.clip { max-width:420px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.nowrap { white-space:nowrap; color:var(--dim); }
.muted { color:var(--dim); }
.foot { color:var(--dim); text-align:center; padding:24px; border-top:1px solid var(--line); font-size:13px; }
@media (max-width:760px){ .cards{grid-template-columns:repeat(2,1fr);} .grid-two{grid-template-columns:1fr;} }
"#;
