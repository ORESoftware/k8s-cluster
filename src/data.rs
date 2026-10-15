use std::time::Duration;

use serde::Serialize;

use crate::app::AppState;

#[derive(Clone, Serialize)]
pub(crate) struct DashboardStats {
    pub(crate) backend_status: &'static str,
    pub(crate) backend_url: String,
    pub(crate) games_running: u64,
    pub(crate) games_finished_today: u64,
    pub(crate) active_sessions: u64,
    pub(crate) goals_per_match: f64,
    pub(crate) sim_ticks_per_second: u64,
    pub(crate) uptime_seconds: u64,
}

impl DashboardStats {
    pub(crate) async fn load(state: &AppState) -> Self {
        let backend_status = match state
            .client
            .get(format!(
                "{}/healthz",
                state.backend_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_millis(800))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => "online",
            _ => "offline",
        };

        let uptime_seconds = state.started.elapsed().as_secs();
        let pulse = uptime_seconds / 3;
        Self {
            backend_status,
            backend_url: state.backend_url.clone(),
            games_running: 8 + (pulse % 6),
            games_finished_today: 144 + (pulse % 31),
            active_sessions: 18 + (pulse % 11),
            goals_per_match: 2.6 + ((pulse % 7) as f64 * 0.04),
            sim_ticks_per_second: 900 + (pulse % 5) * 64,
            uptime_seconds,
        }
    }
}

pub(crate) struct GameRow {
    pub(crate) name: &'static str,
    pub(crate) state: &'static str,
    pub(crate) state_class: &'static str,
    pub(crate) score: String,
    pub(crate) tick: u64,
    pub(crate) model: &'static str,
}

pub(crate) fn game_rows(stats: &DashboardStats) -> Vec<GameRow> {
    let n = stats.uptime_seconds / 2;
    vec![
        GameRow {
            name: "sun-kick-001",
            state: "live",
            state_class: "live",
            score: format!("{}-{}", 2 + (n % 2), 1 + (n % 3)),
            tick: 18_400 + n * 24,
            model: "LP shape + player MPC",
        },
        GameRow {
            name: "blue-press-014",
            state: "training",
            state_class: "training",
            score: format!("{}-{}", n % 3, n % 2),
            tick: 9_120 + n * 18,
            model: "POMDP policy sample",
        },
        GameRow {
            name: "redline-final-092",
            state: "complete",
            state_class: "complete",
            score: "3-2".to_string(),
            tick: 54_000,
            model: "tournament replay",
        },
    ]
}

pub(crate) struct SessionRow {
    pub(crate) name: &'static str,
    pub(crate) detail: String,
    pub(crate) state: &'static str,
    pub(crate) state_class: &'static str,
}

pub(crate) fn session_rows(stats: &DashboardStats) -> Vec<SessionRow> {
    vec![
        SessionRow {
            name: "Portal observer",
            detail: format!("{} active browser sessions", stats.active_sessions),
            state: "active",
            state_class: "live",
        },
        SessionRow {
            name: "Backend bridge",
            detail: format!("{} at {}", stats.backend_status, stats.backend_url),
            state: stats.backend_status,
            state_class: if stats.backend_status == "online" {
                "live"
            } else {
                "offline"
            },
        },
        SessionRow {
            name: "Supabase auth",
            detail: "browser session managed by Supabase Auth".to_string(),
            state: "client",
            state_class: "training",
        },
    ]
}
