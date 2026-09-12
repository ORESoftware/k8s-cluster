//! Users: list + create form.

use axum::extract::State;
use axum::Form;
use dd_pg_defs::{
    validate_usacc_users_insert, UsaccUsersInsert, UsaccUsersRow, USACC_USERS_SELECT_SQL,
    USACC_USERS_TABLE,
};
use maud::{html, Markup};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::state::AppState;

use super::layout::{
    self, caption, empty_row, section_header, short_id, status_badge, NavSection, Ui,
};

const USER_KINDS: &[&str] = &["natural_person", "legal_entity", "service_account", "sim_agent"];
const USER_STATUSES: &[&str] = &["active", "pending", "suspended", "banned", "alumni", "archived"];
const KYC_LEVELS: &[&str] = &["none", "light", "medium", "high"];

pub async fn list_page(State(state): State<AppState>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return layout::page(ui, "Users", NavSection::Users, super::no_db_body());
    };

    let body = html! {
        h1 { "Users" }
        (caption("Register participants and entities. New users appear in the list immediately."))

        div class="split" {
            div {
                (section_header("New user", None))
                form class="card stacked"
                    hx-post=(ui.url("/app/users"))
                    hx-target="#user-list-wrap"
                    hx-swap="outerHTML"
                {
                    label class="field" { "Display name"
                        input type="text" name="display_name" placeholder="Jane Citizen" required;
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Kind"
                            select name="user_kind" { @for k in USER_KINDS { option value=(k) { (k) } } }
                        }
                        label class="field" { "Status"
                            select name="status" { @for s in USER_STATUSES { option value=(s) { (s) } } }
                        }
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "KYC level"
                            select name="kyc_level" { @for l in KYC_LEVELS { option value=(l) { (l) } } }
                        }
                        label class="field" { "Legal region"
                            input type="text" name="legal_region" placeholder="US-CA (optional)";
                        }
                    }
                    div { button type="submit" class="btn-primary" { "Create user" } }
                }
            }
            div {
                (section_header("Directory", None))
                (list_fragment(ui, pool, None).await)
            }
        }
    };
    layout::page(ui, "Users", NavSection::Users, body)
}

#[derive(Deserialize)]
pub struct UserForm {
    display_name: String,
    user_kind: String,
    status: String,
    kyc_level: String,
    legal_region: Option<String>,
}

pub async fn create(State(state): State<AppState>, Form(form): Form<UserForm>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let Some(pool) = state.pool.as_ref() else {
        return html! { div #user-list-wrap { (layout::flash_error("No database is configured.")) } };
    };

    let legal_region = form
        .legal_region
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    let is_legal_entity = form.user_kind == "legal_entity";

    let insert = UsaccUsersInsert {
        display_name: Some(form.display_name.clone()),
        user_kind: Some(form.user_kind.clone()),
        status: Some(form.status.clone()),
        kyc_level: Some(form.kyc_level.clone()),
        roles: Some(json!({})),
        is_legal_entity: Some(is_legal_entity),
        legal_region: legal_region.clone(),
        meta_data: Some(json!({})),
        ..Default::default()
    };
    if let Err(e) = validate_usacc_users_insert(&insert) {
        return list_fragment(ui, pool, Some(layout::flash_error(e))).await;
    }

    let sql = format!(
        "insert into {USACC_USERS_TABLE} \
         (display_name, user_kind, status, kyc_level, roles, is_legal_entity, legal_region, meta_data) \
         values ($1, $2, $3, $4, '{{}}'::jsonb, $5, $6, '{{}}'::jsonb)"
    );
    let result = sqlx::query(&sql)
        .bind(&form.display_name)
        .bind(&form.user_kind)
        .bind(&form.status)
        .bind(&form.kyc_level)
        .bind(is_legal_entity)
        .bind(&legal_region)
        .execute(pool)
        .await;

    let flash = match result {
        Ok(_) => layout::flash_ok(format!("Created user {}.", form.display_name)),
        Err(e) => super::report_db_error("create the user", e),
    };
    list_fragment(ui, pool, Some(flash)).await
}

async fn list_fragment(_ui: Ui<'_>, pool: &PgPool, flash: Option<Markup>) -> Markup {
    let sql = format!("{USACC_USERS_SELECT_SQL} order by created_at desc limit 100");
    let rows = sqlx::query_as::<_, UsaccUsersRow>(&sql)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    html! {
        div #user-list-wrap {
            @if let Some(f) = flash { (f) }
            div class="table-wrap" {
                table {
                    thead { tr { th { "Id" } th { "Name" } th { "Kind" } th { "Status" } th { "KYC" } } }
                    tbody {
                        @if rows.is_empty() { (empty_row(5, "No users yet.")) }
                        @for u in &rows {
                            tr {
                                td { (short_id(&u.id)) }
                                td { (u.display_name) }
                                td { (u.user_kind) }
                                td { (status_badge(&u.status)) }
                                td { (u.kyc_level) }
                            }
                        }
                    }
                }
            }
        }
    }
}
