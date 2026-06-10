//! `/app` overview: at-a-glance counts, a live status pill, recent cases.

use axum::extract::State;
use dd_pg_defs::{
    UsaccCasesRow, USACC_CASES_SELECT_SQL, USACC_CASES_TABLE, USACC_ELECTIONS_TABLE,
    USACC_SIMULATION_RUNS_TABLE, USACC_USERS_TABLE,
};
use maud::{html, Markup};
use sqlx::PgPool;

use crate::state::AppState;

use super::layout::{
    self, caption, empty_row, section_header, short_id, stat_card, status_badge, NavSection, Ui,
};

pub async fn page(State(state): State<AppState>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);

    let Some(pool) = state.pool.as_ref() else {
        return layout::page(
            ui,
            "Dashboard",
            NavSection::Dashboard,
            html! {
                h1 { "Dashboard" }
                (layout::flash_error(
                    "No database is configured. Set USACC_DATABASE_URL (or DATABASE_URL) \
                     to enable the console's data views."
                ))
            },
        );
    };

    let (users, cases, elections, sims) = tokio::join!(
        count(pool, USACC_USERS_TABLE),
        count(pool, USACC_CASES_TABLE),
        count(pool, USACC_ELECTIONS_TABLE),
        count(pool, USACC_SIMULATION_RUNS_TABLE),
    );

    let recent = recent_cases(pool).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "console dashboard: recent cases failed");
        Vec::new()
    });

    let body = html! {
        h1 { "Dashboard" }
        (caption("Operator console for the US Anti-Corruption Court backend. The status pill auto-refreshes every 5s."))

        div class="grid grid-stats" style="margin-top:16px;" {
            (stat_card("Users", fmt(users), "Registered participants and entities."))
            (stat_card("Cases", fmt(cases), "Filed across all tiers."))
            (stat_card("Elections", fmt(elections), "Votes and admission ballots."))
            (stat_card("Simulations", fmt(sims), "Persisted DES runs."))
        }

        (section_header("Recent cases", Some("Most-recent 10 filings. Click an id to drill in.")))
        div class="table-wrap" {
            table {
                thead {
                    tr {
                        th { "Id" }
                        th { "Case #" }
                        th { "Title" }
                        th { "Status" }
                        th { "Tier" }
                        th class="num" { "Priority" }
                    }
                }
                tbody {
                    @if recent.is_empty() {
                        (empty_row(6, "No cases yet."))
                    }
                    @for c in &recent {
                        tr {
                            td { a href=(ui.url(&format!("/app/cases/{}", c.id))) { (short_id(&c.id)) } }
                            td class="nowrap" { (c.case_number) }
                            td { (c.title) }
                            td { (status_badge(&c.status)) }
                            td { (c.filing_tier) }
                            td class="num" { (c.priority_score_micros) }
                        }
                    }
                }
            }
        }
    };

    layout::page(ui, "Dashboard", NavSection::Dashboard, body)
}

/// Navbar status pill, refreshed via `hx-trigger="every 5s"`.
pub async fn status_fragment(State(state): State<AppState>) -> Markup {
    let Some(pool) = state.pool.as_ref() else {
        return html! {
            span class="dot dot-pending" {}
            span { "no db" }
        };
    };
    let ok = sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(pool)
        .await
        .is_ok();
    if ok {
        html! { span class="dot dot-ok" {} span { "ready" } }
    } else {
        html! { span class="dot dot-fail" {} span { "db down" } }
    }
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let sql = format!("select count(*)::bigint from {table}");
    sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

async fn recent_cases(pool: &PgPool) -> Result<Vec<UsaccCasesRow>, sqlx::Error> {
    let sql = format!("{USACC_CASES_SELECT_SQL} order by created_at desc limit 10");
    sqlx::query_as::<_, UsaccCasesRow>(&sql).fetch_all(pool).await
}

fn fmt(n: i64) -> String {
    n.to_string()
}
