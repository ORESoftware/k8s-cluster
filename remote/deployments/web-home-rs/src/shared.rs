use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::metrics::record_request;

pub(crate) fn shared_header(active_page: &'static str) -> Markup {
    html! {
        nav class="dd-site-header" aria-label="Remote runtime navigation" {
            a class="dd-site-brand" href="/home" {
                span class="dd-site-mark" { "dd" }
                span { "remote" }
            }
            div class="dd-site-controls" {
                label class="dd-site-select" {
                    span { "Runtime" }
                    select data-dd-nav-select="runtime" aria-label="Runtime pages" {
                        option value="" { "Runtime" }
                        (nav_option(active_page, "home", "/home", "Service directory"))
                        (nav_option(active_page, "threads", "/agents/threads", "Agent threads"))
                        (nav_option(active_page, "tasks", "/agents/tasks", "Agent tasks"))
                        (nav_option(active_page, "lambdas", "/lambdas/functions", "Lambda functions"))
                        (nav_option(active_page, "container-pool-config", "/container-pool/config", "Container pool config"))
                    }
                }
                label class="dd-site-select" {
                    span { "Labs" }
                    select data-dd-nav-select="labs" aria-label="Browser test labs" {
                        option value="" { "Labs" }
                        (nav_option(active_page, "jello", "/jello", "Athlet-O"))
                        (nav_option(active_page, "wss", "/wss-test", "WebSocket lab"))
                        (nav_option(active_page, "presence", "/presence-test", "Presence lab"))
                    }
                }
                label class="dd-site-select" {
                    span { "Ops" }
                    select data-dd-nav-select="ops" aria-label="Operator paths" {
                        option value="" { "Ops" }
                        option value="/auth?return=/home" { "Auth" }
                        option value="/bastion/runtime/deployments" { "Bastion inventory" }
                        option value="/headlamp/" { "Headlamp" }
                        option value="/api-docs" { "API docs" }
                        option value="/telemetry/" { "Grafana" }
                        option value="/prometheus/" { "Prometheus" }
                    }
                }
                div class="dd-mode-toggle" role="group" aria-label="Color mode" {
                    button class="dd-mode-button" type="button" data-dd-mode-option="dark" aria-pressed="true" { "Dark" }
                    button class="dd-mode-button" type="button" data-dd-mode-option="medium" aria-pressed="false" { "Medium" }
                    button class="dd-mode-button" type="button" data-dd-mode-option="light" aria-pressed="false" { "Light" }
                }
            }
        }
    }
}

fn nav_option(
    active_page: &'static str,
    page: &'static str,
    href: &'static str,
    label: &'static str,
) -> Markup {
    if active_page == page {
        html! {
            option value=(href) selected="selected" { (label) }
        }
    } else {
        html! {
            option value=(href) { (label) }
        }
    }
}

pub(crate) const SHARED_HEADER_BOOT_JS: &str = r##"
(() => {
  try {
    const mode = window.localStorage.getItem("dd-web-home-mode");
    if (mode === "dark" || mode === "medium" || mode === "light") {
      document.documentElement.dataset.ddMode = mode;
    }
  } catch (_error) {}
})();
"##;

pub(crate) const SHARED_HEADER_CSS: &str = r##"
:root {
  --dd-site-header-height: 72px;
  --dd-site-header-bg: var(--panel, #111923);
  --dd-site-header-field: var(--field, var(--panel-2, #0f1720));
  --dd-site-header-line: var(--line, rgba(148, 163, 184, 0.24));
  --dd-site-header-text: var(--text, #eef2f6);
  --dd-site-header-muted: var(--muted, #a8b3c1);
  --dd-site-header-accent: var(--accent, #5eead4);
  --dd-site-header-active-bg: rgba(94, 234, 212, 0.12);
  --dd-site-header-shadow: 0 10px 28px rgba(0, 0, 0, 0.22);
}
:root[data-dd-mode="medium"] {
  color-scheme: dark;
  --bg: #343b45;
  --panel: #424c58;
  --panel-2: #38424e;
  --panel-3: #303946;
  --field: #27313c;
  --line: rgba(245, 248, 251, 0.4);
  --text: #ffffff;
  --muted: #e3ebf3;
  --accent: #9dfff0;
  --accent-2: #fff092;
  --danger: #ffb8c4;
  --ok: #b9ffd2;
  --warn: #ffe49a;
  --code-bg: #202832;
  --code-text: #f7fffc;
  --stream-bg: #1d2530;
  --accent-soft: rgba(157, 255, 240, 0.18);
  --accent-border: rgba(157, 255, 240, 0.72);
  --warn-soft: rgba(255, 228, 154, 0.16);
  --warn-border: rgba(255, 228, 154, 0.7);
  --danger-soft: rgba(255, 184, 196, 0.16);
  --danger-border: rgba(255, 184, 196, 0.7);
  --ok-soft: rgba(185, 255, 210, 0.16);
  --ok-border: rgba(185, 255, 210, 0.7);
  --dd-site-header-bg: #252d36;
  --dd-site-header-field: #1f2832;
  --dd-site-header-active-bg: rgba(157, 255, 240, 0.2);
}
:root[data-dd-mode="light"] {
  color-scheme: light;
  --bg: #f7f9fc;
  --panel: #ffffff;
  --panel-2: #edf2f7;
  --panel-3: #f8fafc;
  --field: #ffffff;
  --line: #8a9aae;
  --text: #111827;
  --muted: #334155;
  --accent: #005f56;
  --accent-2: #744300;
  --danger: #9f1239;
  --ok: #166534;
  --warn: #744300;
  --code-bg: #e6f1f0;
  --code-text: #002e29;
  --stream-bg: #f8fafc;
  --accent-soft: #dff8f4;
  --accent-border: #00796d;
  --warn-soft: #fff1c2;
  --warn-border: #8a5600;
  --danger-soft: #ffe4e6;
  --danger-border: #be123c;
  --ok-soft: #dcfce7;
  --ok-border: #15803d;
  --dd-site-header-bg: #ffffff;
  --dd-site-header-field: #f8fafc;
  --dd-site-header-active-bg: #dff8f4;
  --dd-site-header-shadow: 0 10px 30px rgba(15, 23, 42, 0.12);
}
:root[data-dd-mode="medium"] input,
:root[data-dd-mode="medium"] select,
:root[data-dd-mode="medium"] textarea,
:root[data-dd-mode="medium"] button,
:root[data-dd-mode="light"] input,
:root[data-dd-mode="light"] select,
:root[data-dd-mode="light"] textarea,
:root[data-dd-mode="light"] button {
  background: var(--field);
  color: var(--text);
  border-color: var(--line);
}
:root[data-dd-mode="medium"] button.primary,
:root[data-dd-mode="light"] button.primary {
  background: var(--accent-soft);
  border-color: var(--accent-border);
  color: var(--accent);
}
:root[data-dd-mode="medium"] button.warn,
:root[data-dd-mode="light"] button.warn {
  background: var(--warn-soft);
  border-color: var(--warn-border);
  color: var(--warn);
}
:root[data-dd-mode="medium"] button.danger,
:root[data-dd-mode="light"] button.danger {
  background: var(--danger-soft);
  border-color: var(--danger-border);
  color: var(--danger);
}
:root[data-dd-mode="medium"] button.ok,
:root[data-dd-mode="light"] button.ok {
  background: var(--ok-soft);
  border-color: var(--ok-border);
  color: var(--ok);
}
:root[data-dd-mode="medium"] code,
:root[data-dd-mode="light"] code {
  background: var(--code-bg);
  color: var(--code-text);
  border-color: var(--line);
}
:root[data-dd-mode="medium"] .sidebar,
:root[data-dd-mode="medium"] .tasks-sidebar,
:root[data-dd-mode="light"] .sidebar,
:root[data-dd-mode="light"] .tasks-sidebar {
  background: var(--panel-2);
}
:root[data-dd-mode="medium"] .event,
:root[data-dd-mode="medium"] .task-item,
:root[data-dd-mode="medium"] .context-row,
:root[data-dd-mode="light"] .event,
:root[data-dd-mode="light"] .task-item,
:root[data-dd-mode="light"] .context-row {
  background: var(--panel-3);
  border-color: var(--line);
}
:root[data-dd-mode="medium"] .event.agent,
:root[data-dd-mode="light"] .event.agent {
  background: var(--accent-soft);
  border-color: var(--accent-border);
}
:root[data-dd-mode="medium"] .pill,
:root[data-dd-mode="light"] .pill {
  background: var(--accent-soft);
  border-color: var(--accent-border);
  color: var(--accent);
}
:root[data-dd-mode="medium"] .pill.warn,
:root[data-dd-mode="light"] .pill.warn {
  background: var(--warn-soft);
  border-color: var(--warn-border);
  color: var(--warn);
}
:root[data-dd-mode="medium"] .pill.bad,
:root[data-dd-mode="light"] .pill.bad {
  background: var(--danger-soft);
  border-color: var(--danger-border);
  color: var(--danger);
}
:root[data-dd-mode="medium"] .pill.ok,
:root[data-dd-mode="light"] .pill.ok {
  background: var(--ok-soft);
  border-color: var(--ok-border);
  color: var(--ok);
}
:root[data-dd-mode="medium"] .stream-box,
:root[data-dd-mode="medium"] .terminal-frame,
:root[data-dd-mode="medium"] .terminal-inline iframe,
:root[data-dd-mode="light"] .stream-box,
:root[data-dd-mode="light"] .terminal-frame,
:root[data-dd-mode="light"] .terminal-inline iframe {
  background: var(--stream-bg);
  color: var(--code-text);
}
.dd-site-header {
  position: sticky;
  top: 0;
  z-index: 1000;
  min-height: var(--dd-site-header-height);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 10px 16px;
  background: var(--dd-site-header-bg);
  border-bottom: 1px solid var(--dd-site-header-line);
  box-shadow: var(--dd-site-header-shadow);
  color: var(--dd-site-header-text);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
}
.dd-site-brand {
  display: inline-flex;
  align-items: center;
  gap: 9px;
  color: var(--dd-site-header-text);
  text-decoration: none;
  font-weight: 700;
  white-space: nowrap;
}
.dd-site-brand:hover { text-decoration: none; }
.dd-site-mark {
  display: inline-grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border: 1px solid var(--dd-site-header-accent);
  border-radius: 7px;
  background: var(--dd-site-header-active-bg);
  color: var(--dd-site-header-accent);
}
.dd-site-controls {
  display: flex;
  align-items: end;
  justify-content: flex-end;
  gap: 10px;
  flex-wrap: wrap;
  min-width: 0;
}
.dd-site-select {
  display: grid;
  gap: 4px;
  color: var(--dd-site-header-muted);
  font-size: 11px;
  line-height: 1.1;
}
.dd-site-select span {
  margin: 0;
  color: var(--dd-site-header-muted);
  font-size: 11px;
}
.dd-site-select select {
  min-width: 150px;
  min-height: 34px;
  border: 1px solid var(--dd-site-header-line);
  border-radius: 7px;
  background: var(--dd-site-header-field);
  color: var(--dd-site-header-text);
  padding: 6px 9px;
  font: inherit;
  font-size: 13px;
}
.dd-mode-toggle {
  display: inline-flex;
  align-items: center;
  gap: 0;
  overflow: hidden;
  min-height: 34px;
  border: 1px solid var(--dd-site-header-line);
  border-radius: 7px;
  background: var(--dd-site-header-field);
}
.dd-mode-button {
  min-height: 32px;
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--dd-site-header-text);
  padding: 6px 10px;
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}
.dd-mode-button + .dd-mode-button {
  border-left: 1px solid var(--dd-site-header-line);
}
.dd-mode-button[aria-pressed="true"] {
  background: var(--dd-site-header-active-bg);
  color: var(--dd-site-header-accent);
}
.dd-mode-button:focus-visible,
.dd-site-select select:focus-visible {
  outline: 2px solid var(--dd-site-header-accent);
  outline-offset: 2px;
}
body > .dd-site-header + header {
  top: var(--dd-site-header-height);
}
body > .dd-site-header + .app {
  min-height: calc(100vh - var(--dd-site-header-height));
  min-height: calc(100dvh - var(--dd-site-header-height));
}
body > .dd-site-header + .app[data-spa-root="agents-threads"] {
  height: calc(100vh - var(--dd-site-header-height));
  height: calc(100dvh - var(--dd-site-header-height));
}
@media (max-width: 760px) {
  :root { --dd-site-header-height: 166px; }
  .dd-site-header {
    align-items: stretch;
    flex-direction: column;
  }
  .dd-site-controls {
    width: 100%;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .dd-site-select select,
  .dd-mode-toggle {
    width: 100%;
  }
  .dd-mode-button {
    flex: 1 1 0;
  }
}
@media (max-width: 480px) {
  :root { --dd-site-header-height: 252px; }
  .dd-site-controls {
    grid-template-columns: minmax(0, 1fr);
  }
}
"##;

pub(crate) const SHARED_HEADER_JS: &str = r##"
(() => {
  const root = document.documentElement;
  const storageKey = "dd-web-home-mode";
  const modes = new Set(["dark", "medium", "light"]);
  const modeButtons = Array.from(document.querySelectorAll("[data-dd-mode-option]"));
  const navSelects = Array.from(document.querySelectorAll("[data-dd-nav-select]"));

  const normalizeMode = (value) => modes.has(value) ? value : "dark";

  const storedMode = () => {
    try {
      return normalizeMode(window.localStorage.getItem(storageKey));
    } catch (_error) {
      return normalizeMode(root.dataset.ddMode);
    }
  };

  const applyMode = (mode, persist = true) => {
    const next = normalizeMode(mode);
    root.dataset.ddMode = next;
    for (const button of modeButtons) {
      button.setAttribute("aria-pressed", String(button.dataset.ddModeOption === next));
    }
    if (persist) {
      try {
        window.localStorage.setItem(storageKey, next);
      } catch (_error) {}
    }
  };

  for (const button of modeButtons) {
    button.addEventListener("click", () => applyMode(button.dataset.ddModeOption));
  }

  for (const select of navSelects) {
    select.addEventListener("change", () => {
      if (select.value) window.location.href = select.value;
    });
  }

  applyMode(storedMode(), false);
})();
"##;


pub(crate) fn ui_document(
    title: &str,
    active_page: &'static str,
    theme_color: &str,
    stylesheet_path: &str,
    script_path: &str,
    body: Markup,
) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" data-dd-mode="dark" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                    meta name="theme-color" content=(theme_color);
                    title { (title) }
                    script { (PreEscaped(SHARED_HEADER_BOOT_JS)) }
                    link rel="stylesheet" href=(stylesheet_path);
                    link rel="stylesheet" href="/assets/web-home/shared-header.css";
                    script defer="defer" src="https://cdn.jsdelivr.net/npm/rxjs@7.8.1/dist/bundles/rxjs.umd.min.js" crossorigin="anonymous" {}
                    script defer="defer" src="/assets/web-home/shared-header.js" {}
                    script defer="defer" src=(script_path) {}
                }
                body {
                    (shared_header(active_page))
                    (body)
                }
            }
        }
        .into_string(),
    )
}

pub(crate) fn inline_ui_document(
    title: &str,
    active_page: &'static str,
    stylesheet: &'static str,
    body: &'static str,
    script: &'static str,
) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" data-dd-mode="dark" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                    title { (title) }
                    script { (PreEscaped(SHARED_HEADER_BOOT_JS)) }
                    style { (PreEscaped(stylesheet)) }
                    link rel="stylesheet" href="/assets/web-home/shared-header.css";
                    script defer="defer" src="/assets/web-home/shared-header.js" {}
                }
                body {
                    (shared_header(active_page))
                    (PreEscaped(body))
                    script { (PreEscaped(script)) }
                }
            }
        }
        .into_string(),
    )
}

pub(crate) fn text_asset(path: &'static str, content_type: &'static str, body: &'static str) -> Response {
    record_request("GET", path, StatusCode::OK);
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

pub(crate) fn html_asset(path: &'static str, body: Markup) -> Response {
    record_request("GET", path, StatusCode::OK);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body.into_string(),
    )
        .into_response()
}
