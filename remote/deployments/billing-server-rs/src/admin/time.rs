//! Small time-formatting helpers for the admin UI.

use chrono::{DateTime, Utc};
use maud::{Markup, html};

/// Format a UTC timestamp as a humane relative span (e.g. `"3m ago"`,
/// `"in 2h"`), with the exact RFC3339 timestamp surfaced in the tooltip.
pub fn rel(t: DateTime<Utc>) -> Markup {
    let now = Utc::now();
    let delta = t.signed_duration_since(now);
    let abs = delta.num_seconds().abs();
    let core = humane(abs);
    // For "just now" (sub-5s delta in either direction), drop the
    // "in"/"ago" affix — clock skew makes the sign meaningless that close
    // to the present, and "in just now" reads awkwardly.
    let text = if abs <= 4 {
        core
    } else if delta.num_seconds() > 0 {
        format!("in {core}")
    } else {
        format!("{core} ago")
    };
    let exact = t.to_rfc3339();
    html! {
        span class="nowrap" title=(exact) { (text) }
    }
}

/// Render an `Option<DateTime<Utc>>` as either a relative span or an em-dash.
pub fn rel_opt(t: Option<DateTime<Utc>>) -> Markup {
    match t {
        Some(t) => rel(t),
        None => html! { span class="muted" { "—" } },
    }
}

fn humane(secs: i64) -> String {
    const M: i64 = 60;
    const H: i64 = 3600;
    const D: i64 = 86_400;
    const W: i64 = 7 * D;
    const Y: i64 = 365 * D;
    match secs {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{secs}s"),
        s if s < H => format!("{}m", s / M),
        s if s < D => format!("{}h", s / H),
        s if s < W => format!("{}d", s / D),
        s if s < Y => format!("{}w", s / W),
        s => format!("{}y", s / Y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humane_buckets() {
        assert_eq!(humane(0), "just now");
        assert_eq!(humane(3), "just now");
        assert_eq!(humane(5), "5s");
        assert_eq!(humane(59), "59s");
        assert_eq!(humane(60), "1m");
        assert_eq!(humane(3599), "59m");
        assert_eq!(humane(3600), "1h");
        assert_eq!(humane(86399), "23h");
        assert_eq!(humane(86400), "1d");
        assert_eq!(humane(7 * 86400 - 1), "6d");
        assert_eq!(humane(7 * 86400), "1w");
        assert_eq!(humane(365 * 86400), "1y");
    }

    #[test]
    fn rel_renders_past_and_future() {
        let past = Utc::now() - chrono::Duration::seconds(120);
        let s = rel(past).into_string();
        assert!(s.contains("ago"), "expected 'ago' suffix in {s}");
        assert!(s.contains("title="), "expected exact timestamp in title attr");

        let future = Utc::now() + chrono::Duration::seconds(120);
        let s = rel(future).into_string();
        assert!(s.contains("in "), "expected 'in ' prefix in {s}");
    }
}
