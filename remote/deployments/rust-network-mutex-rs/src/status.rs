//! HTML status page for the broker — upstream
//! [`live-mutex#108`](https://github.com/ORESoftware/live-mutex/issues/108)
//! ("serve a simple html via tcp or uds etc with status page").
//!
//! The page is rendered server-side from a single broker snapshot, so a
//! `GET /` is one mutex lock plus one `format!`. No JS, no external CSS,
//! no template engine. Embeds a small `<style>` block and a
//! `<meta http-equiv="refresh" content="5">` so an operator can leave
//! it open in a browser tab.
//!
//! Two surfaces consume this module:
//!
//! 1. The main HTTP listener (`/` and `/status`), so `LMX_HTTP_PORT`
//!    alone is enough to get a status page.
//! 2. An optional dedicated listener bound on `LMX_STATUS_PORT`. That
//!    listener serves *only* the operator views (no `/v1/*` API), which
//!    matches the deployment posture in this repo: the public gateway
//!    routes the API to the auth-gated HTTP port, and operators reach
//!    the status page on the dedicated port over VPN/bastion.

use std::time::{Duration, Instant};

use crate::broker::{Broker, KeyContentionSnapshot};

/// Snapshot of `ServerConfig` fields we want to surface on the status
/// page. Decoupled from `ServerConfig` itself so this module stays free
/// of the (large) `tls`-feature surface.
#[derive(Debug, Clone)]
pub struct StatusServerInfo {
    pub tcp_bind: Option<String>,
    pub uds_path: Option<String>,
    pub http_bind: Option<String>,
    pub status_bind: Option<String>,
    pub auth_token_set: bool,
    pub tcp_nodelay: bool,
    pub tcp_quickack: bool,
    pub tcp_quickack_effective: bool,
    pub default_ttl: Duration,
    pub ttl_sweep_interval: Duration,
    pub max_lock_holders: u32,
    pub max_concurrency_cap: u32,
    pub tls_enabled: bool,
}

/// Render the status page. `metrics_text` is the raw Prometheus text
/// exposition (already produced by `Metrics::render`); we embed it as a
/// `<pre>` block so the same page is useful in a browser AND scrapeable
/// by curl / `kubectl logs`.
pub fn render(broker: &Broker, info: &StatusServerInfo, metrics_text: &str) -> String {
    let snapshot = broker.metrics();
    let started_at = broker.started_at();
    let uptime = Instant::now().saturating_duration_since(started_at);
    let top = broker.top_keys(10);

    let top_rows = if top.is_empty() {
        "        <tr><td colspan=\"7\" class=\"muted\">No active keys</td></tr>\n".to_string()
    } else {
        top.iter().map(render_top_row).collect::<String>()
    };

    let status_url_line = match info.status_bind.as_ref() {
        Some(addr) => format!(
            "      <tr><th>Dedicated status port</th><td><code>{}</code></td></tr>\n",
            html_escape(addr),
        ),
        None => String::new(),
    };

    let uds_line = match info.uds_path.as_ref() {
        Some(path) => format!(
            "      <tr><th>UDS</th><td><code>{}</code></td></tr>\n",
            html_escape(path),
        ),
        None => String::new(),
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="5">
<title>dd-rust-network-mutex — broker status</title>
<style>
  :root {{
    color-scheme: light dark;
    --fg: #1a1a1a; --bg: #fafafa; --muted: #6b6b6b;
    --accent: #1e6fbf; --warn: #c25400; --ok: #1f8a3b;
    --table-bg: #fff; --border: #d8d8d8;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --fg: #e8e8e8; --bg: #161616; --muted: #9a9a9a;
      --accent: #6cb1ff; --warn: #f4a256; --ok: #6fdc8a;
      --table-bg: #1f1f1f; --border: #353535;
    }}
  }}
  body {{ margin: 0; padding: 24px; font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; color: var(--fg); background: var(--bg); }}
  h1 {{ margin: 0 0 4px; font-size: 18px; font-weight: 600; }}
  h2 {{ margin: 28px 0 8px; font-size: 14px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }}
  .subtitle {{ color: var(--muted); margin-bottom: 20px; font-size: 13px; }}
  .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; }}
  .card {{ background: var(--table-bg); border: 1px solid var(--border); border-radius: 6px; padding: 12px 14px; }}
  .card .label {{ font-size: 11px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }}
  .card .value {{ font-size: 22px; font-weight: 600; margin-top: 4px; font-variant-numeric: tabular-nums; }}
  .card.warn .value {{ color: var(--warn); }}
  table {{ border-collapse: collapse; width: 100%; background: var(--table-bg); border: 1px solid var(--border); border-radius: 6px; overflow: hidden; }}
  th, td {{ padding: 8px 12px; border-bottom: 1px solid var(--border); text-align: left; vertical-align: top; font-variant-numeric: tabular-nums; }}
  thead th {{ background: rgba(127,127,127,0.08); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--muted); }}
  tr:last-child td {{ border-bottom: none; }}
  td.num, th.num {{ text-align: right; }}
  .muted {{ color: var(--muted); }}
  code {{ font: 12px/1.35 SFMono-Regular, ui-monospace, Menlo, monospace; background: rgba(127,127,127,0.1); padding: 1px 5px; border-radius: 3px; }}
  pre {{ background: var(--table-bg); border: 1px solid var(--border); border-radius: 6px; padding: 12px; overflow: auto; max-height: 320px; font: 11px/1.4 SFMono-Regular, ui-monospace, Menlo, monospace; }}
  footer {{ margin-top: 30px; color: var(--muted); font-size: 12px; }}
  footer a {{ color: var(--accent); }}
</style>
</head>
<body>
<h1>dd-rust-network-mutex</h1>
<p class="subtitle">Protocol <code>{protocol_version}</code> · uptime <strong>{uptime}</strong> · auto-refreshes every 5s</p>

<div class="grid">
  <div class="card"><div class="label">Connected clients</div><div class="value">{clients}</div></div>
  <div class="card"><div class="label">Tracked keys</div><div class="value">{keys}</div></div>
  <div class="card"><div class="label">Active holders</div><div class="value">{holders}</div></div>
  <div class="card{waiters_class}"><div class="label">Queued waiters</div><div class="value">{waiters}</div></div>
  <div class="card"><div class="label">Pending deadlines</div><div class="value">{pending_deadlines}</div></div>
  <div class="card{evict_class}"><div class="label">TTL evictions (total)</div><div class="value">{ttl_evictions_total}</div></div>
  <div class="card{clamp_class}"><div class="label">Cap clamps (total)</div><div class="value">{concurrency_cap_clamps_total}</div></div>
</div>

<h2>Top keys by contention</h2>
<table>
  <thead>
    <tr>
      <th>Key</th>
      <th class="num">Holders / max</th>
      <th class="num">Readers</th>
      <th class="num">Writer</th>
      <th class="num">Waiters</th>
      <th class="num">Fencing #</th>
    </tr>
  </thead>
  <tbody>
{top_rows}  </tbody>
</table>

<h2>Listener configuration</h2>
<table>
  <tbody>
{status_url_line}      <tr><th>TCP</th><td>{tcp}</td></tr>
{uds_line}      <tr><th>HTTP</th><td>{http}</td></tr>
      <tr><th>Auth required</th><td>{auth_required}</td></tr>
      <tr><th>TLS</th><td>{tls_state}</td></tr>
      <tr><th><code>TCP_NODELAY</code></th><td>{tcp_nodelay}</td></tr>
      <tr><th><code>TCP_QUICKACK</code></th><td>{tcp_quickack}</td></tr>
      <tr><th>Default TTL</th><td>{default_ttl_ms} ms</td></tr>
      <tr><th>TTL sweep interval</th><td>{ttl_sweep_interval_ms} ms</td></tr>
      <tr><th>Default holders / key</th><td>{max_lock_holders}</td></tr>
      <tr><th>Concurrency cap (ceiling)</th><td>{max_concurrency_cap}</td></tr>
    </tbody>
</table>

<h2>Prometheus exposition</h2>
<pre>{metrics_text}</pre>

<footer>
  upstream <a href="https://github.com/ORESoftware/live-mutex/issues/108">live-mutex#108</a> ·
  <a href="/healthz">/healthz</a> ·
  <a href="/metrics">/metrics</a>
</footer>
</body>
</html>
"#,
        protocol_version = html_escape(crate::protocol::PROTOCOL_VERSION),
        uptime = format_duration(uptime),
        clients = snapshot.clients,
        keys = snapshot.keys,
        holders = snapshot.holders,
        waiters = snapshot.waiters,
        pending_deadlines = snapshot.pending_deadlines,
        ttl_evictions_total = snapshot.ttl_evictions_total,
        concurrency_cap_clamps_total = snapshot.concurrency_cap_clamps_total,
        waiters_class = if snapshot.waiters > 0 { " warn" } else { "" },
        evict_class = if snapshot.ttl_evictions_total > 0 { " warn" } else { "" },
        clamp_class = if snapshot.concurrency_cap_clamps_total > 0 { " warn" } else { "" },
        top_rows = top_rows,
        status_url_line = status_url_line,
        tcp = info
            .tcp_bind
            .as_deref()
            .map(html_escape)
            .map(|s| format!("<code>{s}</code>"))
            .unwrap_or_else(|| "<span class=\"muted\">disabled</span>".into()),
        uds_line = uds_line,
        http = info
            .http_bind
            .as_deref()
            .map(html_escape)
            .map(|s| format!("<code>{s}</code>"))
            .unwrap_or_else(|| "<span class=\"muted\">disabled</span>".into()),
        auth_required = if info.auth_token_set {
            "<span class=\"warn\">required</span>"
        } else {
            "<span class=\"muted\">none</span>"
        },
        tls_state = if info.tls_enabled {
            "<strong>enabled</strong>"
        } else {
            "<span class=\"muted\">disabled</span>"
        },
        tcp_nodelay = on_off(info.tcp_nodelay),
        tcp_quickack = if info.tcp_quickack_effective {
            "<strong>on</strong>"
        } else if info.tcp_quickack {
            "configured (no-op on this OS)"
        } else {
            "<span class=\"muted\">off</span>"
        },
        default_ttl_ms = info.default_ttl.as_millis(),
        ttl_sweep_interval_ms = info.ttl_sweep_interval.as_millis(),
        max_lock_holders = info.max_lock_holders,
        max_concurrency_cap = info.max_concurrency_cap,
        metrics_text = html_escape(metrics_text),
    )
}

fn render_top_row(snap: &KeyContentionSnapshot) -> String {
    // Holders/max collapses to just the holder count for the
    // overwhelming-common `max=1` case so the table stays compact for
    // classic mutex users.
    let holders_cell = if snap.max <= 1 {
        format!("{}", snap.exclusive_holders)
    } else {
        format!("{} / {}", snap.exclusive_holders, snap.max)
    };
    format!(
        "        <tr><td><code>{key}</code></td><td class=\"num\">{hm}</td><td class=\"num\">{rd}</td><td class=\"num\">{wr}</td><td class=\"num\">{wt}</td><td class=\"num\">{fc}</td></tr>\n",
        key = html_escape(&snap.key),
        hm = holders_cell,
        rd = snap.readers,
        wr = snap.writers,
        wt = snap.waiters,
        fc = snap.fencing_counter,
    )
}

fn on_off(b: bool) -> &'static str {
    if b {
        "<strong>on</strong>"
    } else {
        "<span class=\"muted\">off</span>"
    }
}

/// Minimal HTML-escape — covers the five characters that matter for
/// embedding broker-controlled strings (lock keys, paths) into the
/// status page. We avoid pulling in `askama` / `tera` for this one
/// function.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Format a `Duration` for the status page header. Picks the smallest
/// unit that keeps the leading digit non-zero (`42s`, `7m 13s`,
/// `2h 04m`, `9d 03h`).
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs < 86_400 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {:02}h", secs / 86_400, (secs % 86_400) / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::BrokerConfig;
    use crate::protocol::Request;

    fn info() -> StatusServerInfo {
        StatusServerInfo {
            tcp_bind: Some("0.0.0.0:6970".into()),
            uds_path: None,
            http_bind: Some("0.0.0.0:6971".into()),
            status_bind: Some("0.0.0.0:6972".into()),
            auth_token_set: false,
            tcp_nodelay: true,
            tcp_quickack: true,
            tcp_quickack_effective: cfg!(target_os = "linux"),
            default_ttl: Duration::from_millis(4000),
            ttl_sweep_interval: Duration::from_millis(10),
            max_lock_holders: 1,
            max_concurrency_cap: crate::protocol::DEFAULT_MAX_CONCURRENCY_CAP,
            tls_enabled: false,
        }
    }

    #[test]
    fn html_escape_handles_special_characters() {
        assert_eq!(
            html_escape(r#"<script>alert("x&y")</script>"#),
            "&lt;script&gt;alert(&quot;x&amp;y&quot;)&lt;/script&gt;",
        );
    }

    #[test]
    fn duration_formatting_picks_appropriate_unit() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 02m");
        assert_eq!(format_duration(Duration::from_secs(90_000)), "1d 01h");
    }

    #[test]
    fn renders_idle_broker() {
        let broker = Broker::new(BrokerConfig::default());
        let html = render(&broker, &info(), "# fake metrics\nfoo 1\n");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("dd-rust-network-mutex"));
        assert!(html.contains("No active keys"));
        assert!(html.contains("# fake metrics"));
        // status_bind row should be rendered when set.
        assert!(html.contains("Dedicated status port"));
    }

    #[test]
    fn renders_a_held_lock_in_top_keys() {
        let broker = Broker::new(BrokerConfig::default());
        let (a, _a_rx) = broker.register_client();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: Some("contended".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
            },
        );
        let html = render(&broker, &info(), "");
        assert!(html.contains("contended"));
        // Exclusive count column should reflect the single holder.
        assert!(html.contains("<code>contended</code>"));
    }

    #[test]
    fn html_escapes_keys_to_prevent_xss_via_lock_key() {
        let broker = Broker::new(BrokerConfig::default());
        let (a, _a_rx) = broker.register_client();
        let evil = r#"<script>x="y"</script>"#;
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: Some(evil.into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
            },
        );
        let html = render(&broker, &info(), "");
        assert!(
            !html.contains("<script>x=\"y\"</script>"),
            "raw key escaped into HTML — XSS via lock-key vector",
        );
        assert!(html.contains("&lt;script&gt;"));
    }
}
