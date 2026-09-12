//! Cases: list + create form, and a per-case detail view (stages,
//! elections, ledger summary).

use axum::extract::{Path, State};
use axum::Form;
use dd_pg_defs::{
    validate_usacc_cases_insert, UsaccCaseStagesRow, UsaccCasesInsert, UsaccCasesRow,
    UsaccElectionsRow, UsaccLedgerEntriesRow, USACC_CASES_SELECT_SQL, USACC_CASES_TABLE,
    USACC_CASE_STAGES_SELECT_SQL, USACC_ELECTIONS_SELECT_SQL, USACC_LEDGER_ENTRIES_SELECT_SQL,
};
use maud::{html, Markup};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::state::AppState;

use super::layout::{
    self, caption, empty_row, section_header, short_id, status_badge, NavSection, Ui,
};

const CASE_STATUSES: &[&str] = &[
    "draft",
    "signature_collection",
    "screening",
    "inquiry",
    "admission_review",
    "trial",
    "appeal",
    "resolved",
    "canceled",
    "archived",
];
const FILING_TIERS: &[&str] = &[
    "screen", "inquiry", "trial_1", "trial_2", "trial_3", "trial_5", "trial_10",
];

pub async fn list_page(State(state): State<AppState>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return layout::page(ui, "Cases", NavSection::Cases, super::no_db_body());
    };

    let body = html! {
        h1 { "Cases" }
        (caption("File a case and review the docket. New filings appear in the list immediately."))

        div class="split" {
            div {
                (section_header("New case", None))
                form class="card stacked"
                    hx-post=(ui.url("/app/cases"))
                    hx-target="#case-list-wrap"
                    hx-swap="outerHTML"
                {
                    label class="field" { "Case number"
                        input type="text" name="case_number" placeholder="USACC-2026-0001" required;
                    }
                    label class="field" { "Title"
                        input type="text" name="title" placeholder="Short case title" required;
                    }
                    label class="field" { "Defendant summary"
                        textarea name="defendant_summary" rows="2" required {}
                    }
                    label class="field" { "Conduct summary"
                        textarea name="conduct_summary" rows="3" required {}
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Status"
                            select name="status" { @for s in CASE_STATUSES { option value=(s) { (s) } } }
                        }
                        label class="field" { "Filing tier"
                            select name="filing_tier" { @for t in FILING_TIERS { option value=(t) { (t) } } }
                        }
                    }
                    div { button type="submit" class="btn-primary" { "File case" } }
                }
            }
            div {
                (section_header("Docket", None))
                (list_fragment(ui, pool, None).await)
            }
        }
    };
    layout::page(ui, "Cases", NavSection::Cases, body)
}

#[derive(Deserialize)]
pub struct CaseForm {
    case_number: String,
    title: String,
    defendant_summary: String,
    conduct_summary: String,
    status: String,
    filing_tier: String,
}

pub async fn create(State(state): State<AppState>, Form(form): Form<CaseForm>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return html! { div #case-list-wrap { (layout::flash_error("No database is configured.")) } };
    };

    let insert = UsaccCasesInsert {
        case_number: Some(form.case_number.clone()),
        title: Some(form.title.clone()),
        status: Some(form.status.clone()),
        filing_tier: Some(form.filing_tier.clone()),
        defendant_summary: Some(form.defendant_summary.clone()),
        conduct_summary: Some(form.conduct_summary.clone()),
        priority_score_micros: Some(0),
        meta_data: Some(json!({})),
        ..Default::default()
    };
    if let Err(e) = validate_usacc_cases_insert(&insert) {
        return list_fragment(ui, pool, Some(layout::flash_error(e))).await;
    }

    let sql = format!(
        "insert into {USACC_CASES_TABLE} \
         (case_number, title, status, filing_tier, defendant_summary, conduct_summary, priority_score_micros, meta_data) \
         values ($1, $2, $3, $4, $5, $6, 0, '{{}}'::jsonb)"
    );
    let result = sqlx::query(&sql)
        .bind(&form.case_number)
        .bind(&form.title)
        .bind(&form.status)
        .bind(&form.filing_tier)
        .bind(&form.defendant_summary)
        .bind(&form.conduct_summary)
        .execute(pool)
        .await;

    let flash = match result {
        Ok(_) => layout::flash_ok(format!("Filed case {}.", form.case_number)),
        Err(e) => super::report_db_error("file the case", e),
    };
    list_fragment(ui, pool, Some(flash)).await
}

async fn list_fragment(ui: Ui<'_>, pool: &PgPool, flash: Option<Markup>) -> Markup {
    let sql = format!("{USACC_CASES_SELECT_SQL} order by created_at desc limit 100");
    let rows = sqlx::query_as::<_, UsaccCasesRow>(&sql)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    html! {
        div #case-list-wrap {
            @if let Some(f) = flash { (f) }
            div class="table-wrap" {
                table {
                    thead { tr { th { "Id" } th { "Case #" } th { "Title" } th { "Status" } th { "Tier" } } }
                    tbody {
                        @if rows.is_empty() { (empty_row(5, "No cases yet.")) }
                        @for c in &rows {
                            tr {
                                td { a href=(ui.url(&format!("/app/cases/{}", c.id))) { (short_id(&c.id)) } }
                                td class="nowrap" { (c.case_number) }
                                td { (c.title) }
                                td { (status_badge(&c.status)) }
                                td { (c.filing_tier) }
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
        return layout::page(ui, "Case", NavSection::Cases, super::no_db_body());
    };

    let case = sqlx::query_as::<_, UsaccCasesRow>(&format!(
        "{USACC_CASES_SELECT_SQL} where id = $1::uuid"
    ))
    .bind(&id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let Some(case) = case else {
        return layout::page(
            ui,
            "Case",
            NavSection::Cases,
            html! {
                h1 { "Case" }
                (layout::flash_error("Case not found."))
                p { a href=(ui.url("/app/cases")) { "← Back to docket" } }
            },
        );
    };

    let stages = sqlx::query_as::<_, UsaccCaseStagesRow>(&format!(
        "{USACC_CASE_STAGES_SELECT_SQL} where case_id = $1::uuid order by stage_order asc"
    ))
    .bind(&id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let elections = sqlx::query_as::<_, UsaccElectionsRow>(&format!(
        "{USACC_ELECTIONS_SELECT_SQL} where case_id = $1::uuid order by created_at desc"
    ))
    .bind(&id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let ledger = sqlx::query_as::<_, UsaccLedgerEntriesRow>(&format!(
        "{USACC_LEDGER_ENTRIES_SELECT_SQL} where case_id = $1::uuid"
    ))
    .bind(&id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let net_cents: i64 = ledger
        .iter()
        .map(|e| match e.direction.as_str() {
            "credit" => e.amount_cents,
            "debit" => -e.amount_cents,
            _ => 0,
        })
        .sum();

    let body = html! {
        p { a href=(ui.url("/app/cases")) { "← Back to docket" } }
        h1 { (case.title) }
        (caption(&format!("{} · {}", case.case_number, case.filing_tier)))

        dl class="kv card" style="margin-top:12px;" {
            dt { "Status" } dd { (status_badge(&case.status)) }
            dt { "Defendant" } dd { (case.defendant_summary) }
            dt { "Conduct" } dd { (case.conduct_summary) }
            dt { "Priority" } dd { (case.priority_score_micros) }
            dt { "Opened" } dd { (case.opened_at.clone().unwrap_or_else(|| "—".into())) }
            dt { "Created" } dd { (case.created_at) }
        }

        (section_header("Stages", None))
        div class="table-wrap" {
            table {
                thead { tr { th class="num" { "#" } th { "Key" } th { "Title" } th { "Status" } } }
                tbody {
                    @if stages.is_empty() { (empty_row(4, "No stages.")) }
                    @for s in &stages {
                        tr {
                            td class="num" { (s.stage_order) }
                            td { (s.stage_key) }
                            td { (s.title) }
                            td { (status_badge(&s.status)) }
                        }
                    }
                }
            }
        }

        (section_header("Elections", None))
        div class="table-wrap" {
            table {
                thead { tr { th { "Id" } th { "Title" } th { "Kind" } th { "Status" } } }
                tbody {
                    @if elections.is_empty() { (empty_row(4, "No elections.")) }
                    @for e in &elections {
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

        (section_header("Ledger", Some(&format!("{} entries · net {} cents", ledger.len(), net_cents))))
    };
    layout::page(ui, &case.title, NavSection::Cases, body)
}
