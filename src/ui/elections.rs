//! Elections: list + create form, a detail view with the vote table, and
//! a "run tally" action that certifies the election (mirrors the JSON
//! `POST /api/usacc/elections/:id/tally`).

use axum::extract::{Path, State};
use axum::Form;
use dd_pg_defs::{
    validate_usacc_elections_insert, UsaccElectionsInsert, UsaccElectionsRow, UsaccVotesRow,
    USACC_ELECTIONS_SELECT_SQL, USACC_ELECTIONS_TABLE, USACC_VOTES_SELECT_SQL, USACC_VOTES_TABLE,
};
use maud::{html, Markup};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row};

use crate::state::AppState;

use super::layout::{
    self, caption, empty_row, section_header, short_id, status_badge, NavSection, Ui,
};

const ELECTION_KINDS: &[&str] = &[
    "priority",
    "admission",
    "panel_verdict",
    "appeal",
    "oversight",
    "policy",
    "assignment_acceptance",
];
const ELECTION_STATUSES: &[&str] = &[
    "draft", "open", "sealed", "tallying", "certified", "void", "archived",
];

pub async fn list_page(State(state): State<AppState>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return layout::page(ui, "Elections", NavSection::Elections, super::no_db_body());
    };

    let body = html! {
        h1 { "Elections" }
        (caption("Open ballots for priority, admission, verdict, and oversight votes."))

        div class="split" {
            div {
                (section_header("New election", None))
                form class="card stacked"
                    hx-post=(ui.url("/app/elections"))
                    hx-target="#election-list-wrap"
                    hx-swap="outerHTML"
                {
                    label class="field" { "Title"
                        input type="text" name="title" placeholder="Ballot title" required;
                    }
                    label class="field" { "Case id"
                        input type="text" name="case_id" placeholder="uuid (optional)";
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Kind"
                            select name="election_kind" { @for k in ELECTION_KINDS { option value=(k) { (k) } } }
                        }
                        label class="field" { "Status"
                            select name="status" { @for s in ELECTION_STATUSES { option value=(s) { (s) } } }
                        }
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Quorum count"
                            input type="text" name="quorum_count" value="1";
                        }
                        label class="field" { "Threshold (micros)"
                            input type="text" name="threshold_micros" value="500000";
                        }
                    }
                    div { button type="submit" class="btn-primary" { "Open election" } }
                }
            }
            div {
                (section_header("Ballots", None))
                (list_fragment(ui, pool, None).await)
            }
        }
    };
    layout::page(ui, "Elections", NavSection::Elections, body)
}

#[derive(Deserialize)]
pub struct ElectionForm {
    title: String,
    case_id: Option<String>,
    election_kind: String,
    status: String,
    quorum_count: Option<String>,
    threshold_micros: Option<String>,
}

pub async fn create(State(state): State<AppState>, Form(form): Form<ElectionForm>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return html! { div #election-list-wrap { (layout::flash_error("No database is configured.")) } };
    };

    let case_id = blank_to_none(form.case_id.as_deref());
    let quorum = parse_or(form.quorum_count.as_deref(), 1);
    let threshold = parse_or(form.threshold_micros.as_deref(), 500_000);

    let insert = UsaccElectionsInsert {
        case_id: case_id.clone(),
        election_kind: Some(form.election_kind.clone()),
        title: Some(form.title.clone()),
        status: Some(form.status.clone()),
        quorum_count: Some(quorum),
        threshold_micros: Some(threshold),
        tally: Some(json!({})),
        meta_data: Some(json!({})),
        ..Default::default()
    };
    if let Err(e) = validate_usacc_elections_insert(&insert) {
        return list_fragment(ui, pool, Some(layout::flash_error(e))).await;
    }

    let sql = format!(
        "insert into {USACC_ELECTIONS_TABLE} \
         (case_id, election_kind, title, status, quorum_count, threshold_micros, meta_data) \
         values ($1::uuid, $2, $3, $4, $5, $6, '{{}}'::jsonb)"
    );
    let result = sqlx::query(&sql)
        .bind(&case_id)
        .bind(&form.election_kind)
        .bind(&form.title)
        .bind(&form.status)
        .bind(quorum)
        .bind(threshold)
        .execute(pool)
        .await;

    let flash = match result {
        Ok(_) => layout::flash_ok(format!("Opened election {}.", form.title)),
        Err(e) => super::report_db_error("open the election", e),
    };
    list_fragment(ui, pool, Some(flash)).await
}

async fn list_fragment(ui: Ui<'_>, pool: &PgPool, flash: Option<Markup>) -> Markup {
    let sql = format!("{USACC_ELECTIONS_SELECT_SQL} order by created_at desc limit 100");
    let rows = sqlx::query_as::<_, UsaccElectionsRow>(&sql)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    html! {
        div #election-list-wrap {
            @if let Some(f) = flash { (f) }
            div class="table-wrap" {
                table {
                    thead { tr { th { "Id" } th { "Title" } th { "Kind" } th { "Status" } } }
                    tbody {
                        @if rows.is_empty() { (empty_row(4, "No elections yet.")) }
                        @for e in &rows {
                            tr {
                                td { a href=(ui.url(&format!("/app/elections/{}", e.id))) { (short_id(&e.id)) } }
                                td { (e.title) }
                                td { (e.election_kind) }
                                td { (status_badge(&e.status)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub async fn detail_page(State(state): State<AppState>, Path(id): Path<String>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return layout::page(ui, "Election", NavSection::Elections, super::no_db_body());
    };

    let election = sqlx::query_as::<_, UsaccElectionsRow>(&format!(
        "{USACC_ELECTIONS_SELECT_SQL} where id = $1::uuid"
    ))
    .bind(&id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let Some(election) = election else {
        return layout::page(
            ui,
            "Election",
            NavSection::Elections,
            html! {
                h1 { "Election" }
                (layout::flash_error("Election not found."))
                p { a href=(ui.url("/app/elections")) { "← Back to ballots" } }
            },
        );
    };

    let votes = sqlx::query_as::<_, UsaccVotesRow>(&format!(
        "{USACC_VOTES_SELECT_SQL} where election_id = $1::uuid order by created_at desc"
    ))
    .bind(&id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let body = html! {
        p { a href=(ui.url("/app/elections")) { "← Back to ballots" } }
        h1 { (election.title) }
        (caption(&format!("{} · quorum {} · threshold {} micros", election.election_kind, election.quorum_count, election.threshold_micros)))

        dl class="kv card" style="margin-top:12px;" {
            dt { "Status" } dd { (status_badge(&election.status)) }
            dt { "Case" } dd {
                @match &election.case_id {
                    Some(cid) => a href=(ui.url(&format!("/app/cases/{cid}"))) { (short_id(cid)) },
                    None => "—",
                }
            }
            dt { "Created" } dd { (election.created_at) }
        }

        div style="margin:16px 0;" {
            button class="btn-primary"
                hx-post=(ui.url(&format!("/app/elections/{id}/tally")))
                hx-target="#tally-panel"
                hx-swap="innerHTML"
            { "Run tally & certify" }
        }
        div #tally-panel {}

        (section_header("Votes", None))
        div class="table-wrap" {
            table {
                thead { tr { th { "Voter" } th { "Value" } th { "Kind" } th class="num" { "Weight" } } }
                tbody {
                    @if votes.is_empty() { (empty_row(4, "No votes cast yet.")) }
                    @for v in &votes {
                        tr {
                            td { (short_id(&v.voter_user_id)) }
                            td { (v.vote_value) }
                            td { (v.vote_kind) }
                            td class="num" { (v.weight_micros) }
                        }
                    }
                }
            }
        }
    };
    layout::page(ui, &election.title, NavSection::Elections, body)
}

/// HTMX action: compute the weighted tally, persist `certified` + the
/// tally JSON, and return an HTML summary panel. Mirrors the JSON route's
/// arithmetic so the console and API agree.
pub async fn tally(State(state): State<AppState>, Path(id): Path<String>) -> Markup {
    let Some(pool) = state.pool.as_ref() else {
        return layout::flash_error("No database is configured.");
    };

    let election = match sqlx::query_as::<_, UsaccElectionsRow>(&format!(
        "{USACC_ELECTIONS_SELECT_SQL} where id = $1::uuid"
    ))
    .bind(&id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(e)) => e,
        Ok(None) => return layout::flash_error("Election not found."),
        Err(e) => return super::report_db_error("tally the election", e),
    };

    let tally_sql = format!(
        "select vote_value, count(*)::bigint as vote_count, coalesce(sum(weight_micros), 0)::bigint as weight_micros \
         from {USACC_VOTES_TABLE} where election_id = $1::uuid group by vote_value \
         order by weight_micros desc, vote_count desc, vote_value asc"
    );
    let rows = match sqlx::query(&tally_sql).bind(&id).fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => return super::report_db_error("tally the election", e),
    };

    struct Choice {
        value: String,
        count: i64,
        weight: i64,
    }
    let choices: Vec<Choice> = rows
        .into_iter()
        .map(|row| Choice {
            value: row.get("vote_value"),
            count: row.get("vote_count"),
            weight: row.get("weight_micros"),
        })
        .collect();
    let total_votes: i64 = choices.iter().map(|c| c.count).sum();
    let total_weight: i64 = choices.iter().map(|c| c.weight).sum();
    let winner = choices.first();
    let passed = winner
        .map(|c| {
            c.weight.saturating_mul(1_000_000)
                >= total_weight.saturating_mul(election.threshold_micros as i64)
        })
        .unwrap_or(false);
    let winning_value = winner.map(|c| c.value.clone());

    let tally_json = json!({
        "ok": true,
        "electionId": id,
        "totalVotes": total_votes,
        "totalWeightMicros": total_weight,
        "thresholdMicros": election.threshold_micros,
        "winningValue": winning_value,
        "passed": passed,
        "choices": choices.iter().map(|c| json!({
            "voteValue": c.value, "voteCount": c.count, "weightMicros": c.weight,
        })).collect::<Vec<_>>(),
    });

    let update = format!(
        "update {USACC_ELECTIONS_TABLE} set status = 'certified', tally = $2::jsonb, updated_at = now() where id = $1::uuid"
    );
    if let Err(e) = sqlx::query(&update)
        .bind(&id)
        .bind(&tally_json)
        .execute(pool)
        .await
    {
        return super::report_db_error("persist the certified tally", e);
    }

    html! {
        (layout::flash_ok(if passed { "Certified — threshold met." } else { "Certified — threshold not met." }))
        div class="card" {
            dl class="kv" {
                dt { "Winning value" } dd { (winning_value.clone().unwrap_or_else(|| "—".into())) }
                dt { "Passed" } dd { (status_badge(if passed { "succeeded" } else { "failed" })) }
                dt { "Total votes" } dd { (total_votes) }
                dt { "Total weight" } dd { (total_weight) " micros" }
            }
        }
        div class="table-wrap" style="margin-top:12px;" {
            table {
                thead { tr { th { "Value" } th class="num" { "Votes" } th class="num" { "Weight" } } }
                tbody {
                    @for c in &choices {
                        tr { td { (c.value) } td class="num" { (c.count) } td class="num" { (c.weight) } }
                    }
                }
            }
        }
    }
}

fn blank_to_none(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|s| !s.is_empty()).map(ToString::to_string)
}

fn parse_or(v: Option<&str>, default: i32) -> i32 {
    v.and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(default)
}
