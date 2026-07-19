//! Maud views for the shared HTMX/WebSocket surface.

use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::realtime::{EventEnvelope, ServiceSurface};

const HTMX_URL: &str = "https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js";
const HTMX_INTEGRITY: &str =
    "sha384-H5SrcfygHmAuTDZphMHqBJLc3FhssKjG7w/CeCpFReSfwBWDTKpkzPP8c+cLsK+V";
const HTMX_WS_URL: &str = "https://cdn.jsdelivr.net/npm/htmx-ext-ws@2.0.4";
const HTMX_WS_INTEGRITY: &str =
    "sha384-1RwI/nvUSrMRuNj7hX1+27J8XDdCoSLf0EjEyF69nacuWyiJYoQ/j39RT1mSnd2G";

pub(crate) fn page(surface: ServiceSurface, event: &EventEnvelope) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (surface.title()) " · Daedalus" }
                script src=(HTMX_URL) integrity=(HTMX_INTEGRITY) crossorigin="anonymous" {}
                script src=(HTMX_WS_URL) integrity=(HTMX_WS_INTEGRITY) crossorigin="anonymous" {}
                style { (PreEscaped(STYLES)) }
            }
            body {
                main {
                    header {
                        p class="eyebrow" { "DAEDALUS · MASH" }
                        h1 { (surface.title()) }
                        p class="lede" {
                            "Maud-rendered hypermedia over Axum, with SeaORM-backed Postgres, "
                            "HTMX HTML streams, JSON WebSockets, raw TCP, and NATS fan-out."
                        }
                    }
                    section class="transport-grid" aria-label="Transport contracts" {
                        (transport_card("HTTP", "GET /api/realtime", "Snapshot JSON and server-rendered HTML"))
                        (transport_card("HTML WebSocket", "GET /ws/html", "HTMX out-of-band Maud fragments"))
                        (transport_card("JSON WebSocket", "GET /ws/json", "Versioned event envelopes"))
                        (transport_card("TCP + NATS", "newline JSON · shared subjects", "Non-browser stream and broker fan-out"))
                    }
                    div hx-ext="ws" ws-connect="/ws/html" {
                        (status_fragment(event))
                        form ws-send="" {
                            input type="hidden" name="action" value="refresh";
                            button type="submit" { "Refresh over WebSocket" }
                        }
                    }
                    section class="json-panel" {
                        h2 { "JSON WebSocket" }
                        p { "The same event envelope, without HTML presentation." }
                        pre id="json-stream" { (serialized(event)) }
                    }
                }
                script { (PreEscaped(JSON_SOCKET_SCRIPT)) }
            }
        }
    }
}

pub(crate) fn status_fragment(event: &EventEnvelope) -> Markup {
    html! {
        section id="realtime-status" class="status-panel" hx-swap-oob="outerHTML" {
            div class="status-heading" {
                h2 { "Realtime event" }
                span class="schema" { (event.schema_version) }
            }
            dl {
                dt { "Kind" }
                dd { (event.kind) }
                dt { "Source" }
                dd { (event.source) }
                dt { "Event ID" }
                dd { (event.event_id) }
            }
            pre { (serialized(event)) }
        }
    }
}

fn transport_card(title: &str, endpoint: &str, description: &str) -> Markup {
    html! {
        article {
            h2 { (title) }
            code { (endpoint) }
            p { (description) }
        }
    }
}

fn serialized(event: &EventEnvelope) -> String {
    serde_json::to_string_pretty(event).unwrap_or_else(|_| "{}".to_string())
}

const JSON_SOCKET_SCRIPT: &str = r#"
(() => {
  const output = document.getElementById('json-stream');
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  let retry = 250;
  const connect = () => {
    const socket = new WebSocket(`${scheme}://${location.host}/ws/json`);
    socket.addEventListener('open', () => { retry = 250; });
    socket.addEventListener('message', ({ data }) => {
      try { output.textContent = JSON.stringify(JSON.parse(data), null, 2); }
      catch (_) { output.textContent = data; }
    });
    socket.addEventListener('close', () => {
      setTimeout(connect, retry);
      retry = Math.min(retry * 2, 10000);
    });
  };
  connect();
})();
"#;

const STYLES: &str = r#"
:root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, sans-serif; }
* { box-sizing: border-box; }
body { margin: 0; background: #07110f; color: #e9f7ef; }
main { width: min(1120px, calc(100% - 2rem)); margin: 0 auto; padding: 4rem 0; }
header { max-width: 760px; margin-bottom: 2.5rem; }
.eyebrow, .schema { color: #75e6b5; font-weight: 700; letter-spacing: .14em; text-transform: uppercase; }
h1 { margin: .35rem 0 1rem; font-size: clamp(2.4rem, 7vw, 5.5rem); line-height: .95; }
.lede { color: #b8ccc4; font-size: 1.15rem; line-height: 1.65; }
.transport-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 1rem; }
article, .status-panel, .json-panel { border: 1px solid #21443a; border-radius: 14px; background: #0c1d18; padding: 1.25rem; }
article h2, .status-panel h2, .json-panel h2 { margin-top: 0; }
code { color: #ffd57a; }
.status-panel, .json-panel { margin-top: 1rem; }
.status-heading { display: flex; align-items: baseline; justify-content: space-between; gap: 1rem; }
dl { display: grid; grid-template-columns: 7rem 1fr; gap: .5rem; }
dt { color: #86a79a; }
dd { margin: 0; overflow-wrap: anywhere; }
pre { overflow: auto; border-radius: 8px; background: #04100c; color: #b9fbd8; padding: 1rem; }
button { margin-top: 1rem; border: 0; border-radius: 999px; background: #75e6b5; color: #07110f; padding: .75rem 1.1rem; font-weight: 750; cursor: pointer; }
"#;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn page_contains_mash_and_both_websocket_contracts() {
        let event = EventEnvelope::new("test", "test.ready", json!({"ok": true}));
        let rendered = page(ServiceSurface::Web, &event).into_string();

        for contract in [
            "htmx.org@2.0.10",
            "htmx-ext-ws@2.0.4",
            "hx-ext=\"ws\"",
            "ws-connect=\"/ws/html\"",
            "ws-send",
            "/ws/json",
            "SeaORM-backed Postgres",
        ] {
            assert!(rendered.contains(contract), "missing {contract}");
        }
    }

    #[test]
    fn maud_escapes_untrusted_realtime_payloads() {
        let event = EventEnvelope::new(
            "supabase",
            "postgres_changes",
            json!({"value": "<script>alert('unsafe')</script>"}),
        );
        let rendered = status_fragment(&event).into_string();

        assert!(!rendered.contains("<script>alert"));
        assert!(rendered.contains("&lt;script&gt;alert"));
        assert!(rendered.contains("hx-swap-oob=\"outerHTML\""));
    }
}
