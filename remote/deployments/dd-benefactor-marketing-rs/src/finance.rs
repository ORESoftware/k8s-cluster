use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_budget_forecasts as budget_forecasts,
    benefactor_marketing_commission_entries as commission_entries,
    benefactor_marketing_invoices as invoices, benefactor_marketing_time_entries as time_entries,
    benefactor_marketing_vendor_costs as vendor_costs,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation};
use crate::shared::{
    computed_commission_amount, ensure_client, ensure_optional_campaign_for_client,
    ensure_optional_opportunity, ensure_optional_project_task, iso_date, limit, minutes_in_day,
    non_negative, object_or_default, optional_iso_date, probability, require_auth,
    require_write_access, ListQuery,
};
use crate::state::{AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTimeEntryRequest {
    campaign_id: Option<Uuid>,
    project_task_id: Option<Uuid>,
    user_id: Uuid,
    entry_date: String,
    minutes: i32,
    billable: Option<bool>,
    rate_cents: Option<i32>,
    cost_cents: Option<i32>,
    notes: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateVendorCostRequest {
    campaign_id: Option<Uuid>,
    vendor_name: String,
    category: String,
    status: Option<String>,
    amount_cents: i32,
    incurred_on: Option<String>,
    invoice_ref: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCommissionEntryRequest {
    opportunity_id: Option<Uuid>,
    user_id: Uuid,
    status: Option<String>,
    commission_kind: Option<String>,
    basis_cents: Option<i32>,
    rate_micros: Option<i32>,
    amount_cents: Option<i32>,
    earned_on: Option<String>,
    paid_at: Option<DateTime<FixedOffset>>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateBudgetForecastRequest {
    campaign_id: Option<Uuid>,
    forecast_kind: Option<String>,
    period_start: String,
    period_end: String,
    status: Option<String>,
    revenue_cents: Option<i32>,
    media_spend_cents: Option<i32>,
    labor_cost_cents: Option<i32>,
    vendor_cost_cents: Option<i32>,
    gross_margin_cents: Option<i32>,
    assumptions: Option<Value>,
}

pub(crate) async fn list_client_time_entries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = time_entries::Entity::find()
        .filter(time_entries::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(time_entries::Column::EntryDate)
        .order_by_desc(time_entries::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "timeEntries": rows })))
}

pub(crate) async fn create_time_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateTimeEntryRequest>,
) -> AppResult<(StatusCode, Json<time_entries::Model>)> {
    require_write_access(&state, &headers, "time-entries.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_campaign_for_client(&state.db, client_id, req.campaign_id).await?;
    ensure_optional_project_task(&state.db, client_id, req.project_task_id).await?;
    let model = time_entries::ActiveModel {
        client_id: Set(Some(client_id)),
        campaign_id: Set(req.campaign_id),
        project_task_id: Set(req.project_task_id),
        user_id: Set(req.user_id),
        entry_date: Set(iso_date(req.entry_date, "entryDate")?),
        minutes: Set(minutes_in_day(req.minutes)?),
        billable: Set(req.billable.unwrap_or(true)),
        rate_cents: Set(non_negative(req.rate_cents.unwrap_or(0), "rateCents")?),
        cost_cents: Set(non_negative(req.cost_cents.unwrap_or(0), "costCents")?),
        notes: Set(req.notes),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .agency_finance_records_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "time_entry_recorded",
        json!({
            "clientId": client_id,
            "timeEntryId": model.id,
            "campaignId": model.campaign_id,
            "projectTaskId": model.project_task_id,
            "userId": model.user_id,
            "minutes": model.minutes,
            "billable": model.billable
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_vendor_costs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = vendor_costs::Entity::find()
        .filter(vendor_costs::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(vendor_costs::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "vendorCosts": rows })))
}

pub(crate) async fn create_vendor_cost(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateVendorCostRequest>,
) -> AppResult<(StatusCode, Json<vendor_costs::Model>)> {
    require_write_access(&state, &headers, "vendor-costs.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_campaign_for_client(&state.db, client_id, req.campaign_id).await?;
    let model = vendor_costs::ActiveModel {
        client_id: Set(Some(client_id)),
        campaign_id: Set(req.campaign_id),
        vendor_name: Set(req.vendor_name),
        category: Set(req.category),
        status: Set(req.status.unwrap_or_else(|| "planned".to_string())),
        amount_cents: Set(non_negative(req.amount_cents, "amountCents")?),
        incurred_on: Set(optional_iso_date(req.incurred_on, "incurredOn")?),
        invoice_ref: Set(req.invoice_ref),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .agency_finance_records_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "vendor_cost_recorded",
        json!({
            "clientId": client_id,
            "vendorCostId": model.id,
            "campaignId": model.campaign_id,
            "vendorName": &model.vendor_name,
            "category": &model.category,
            "amountCents": model.amount_cents,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_commissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = commission_entries::Entity::find()
        .filter(commission_entries::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(commission_entries::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "commissions": rows })))
}

pub(crate) async fn create_commission_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateCommissionEntryRequest>,
) -> AppResult<(StatusCode, Json<commission_entries::Model>)> {
    require_write_access(&state, &headers, "commissions.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_opportunity(&state.db, client_id, req.opportunity_id).await?;
    let basis_cents = non_negative(req.basis_cents.unwrap_or(0), "basisCents")?;
    let rate_micros = probability(req.rate_micros.unwrap_or(0))?;
    let amount_cents = match req.amount_cents {
        Some(value) => non_negative(value, "amountCents")?,
        None => computed_commission_amount(basis_cents, rate_micros)?,
    };
    let model = commission_entries::ActiveModel {
        client_id: Set(Some(client_id)),
        opportunity_id: Set(req.opportunity_id),
        user_id: Set(req.user_id),
        status: Set(req.status.unwrap_or_else(|| "pending".to_string())),
        commission_kind: Set(req.commission_kind.unwrap_or_else(|| "deal".to_string())),
        basis_cents: Set(basis_cents),
        rate_micros: Set(rate_micros),
        amount_cents: Set(amount_cents),
        earned_on: Set(optional_iso_date(req.earned_on, "earnedOn")?),
        paid_at: Set(req.paid_at),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .agency_finance_records_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "commission_entry_recorded",
        json!({
            "clientId": client_id,
            "commissionEntryId": model.id,
            "opportunityId": model.opportunity_id,
            "userId": model.user_id,
            "commissionKind": &model.commission_kind,
            "amountCents": model.amount_cents,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_budget_forecasts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = budget_forecasts::Entity::find()
        .filter(budget_forecasts::Column::ClientId.eq(client_id))
        .order_by_desc(budget_forecasts::Column::PeriodStart)
        .order_by_desc(budget_forecasts::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "budgetForecasts": rows })))
}

pub(crate) async fn create_budget_forecast(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateBudgetForecastRequest>,
) -> AppResult<(StatusCode, Json<budget_forecasts::Model>)> {
    require_write_access(&state, &headers, "budget-forecasts.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_campaign_for_client(&state.db, client_id, req.campaign_id).await?;
    let revenue_cents = non_negative(req.revenue_cents.unwrap_or(0), "revenueCents")?;
    let media_spend_cents = non_negative(req.media_spend_cents.unwrap_or(0), "mediaSpendCents")?;
    let labor_cost_cents = non_negative(req.labor_cost_cents.unwrap_or(0), "laborCostCents")?;
    let vendor_cost_cents = non_negative(req.vendor_cost_cents.unwrap_or(0), "vendorCostCents")?;
    let gross_margin_cents = req
        .gross_margin_cents
        .unwrap_or(revenue_cents - media_spend_cents - labor_cost_cents - vendor_cost_cents);
    let model = budget_forecasts::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        forecast_kind: Set(req.forecast_kind.unwrap_or_else(|| "monthly".to_string())),
        period_start: Set(iso_date(req.period_start, "periodStart")?),
        period_end: Set(iso_date(req.period_end, "periodEnd")?),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        revenue_cents: Set(revenue_cents),
        media_spend_cents: Set(media_spend_cents),
        labor_cost_cents: Set(labor_cost_cents),
        vendor_cost_cents: Set(vendor_cost_cents),
        gross_margin_cents: Set(gross_margin_cents),
        assumptions: Set(object_or_default(req.assumptions, "assumptions")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .agency_finance_records_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "budget_forecast_recorded",
        json!({
            "clientId": client_id,
            "budgetForecastId": model.id,
            "campaignId": model.campaign_id,
            "forecastKind": &model.forecast_kind,
            "periodStart": &model.period_start,
            "periodEnd": &model.period_end,
            "grossMarginCents": model.gross_margin_cents,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn client_profitability_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let row_limit = limit(query.limit);
    let recent_invoices = invoices::Entity::find()
        .filter(invoices::Column::ClientId.eq(client_id))
        .order_by_desc(invoices::Column::UpdatedAt)
        .limit(row_limit)
        .all(&state.db)
        .await?;
    let recent_time_entries = time_entries::Entity::find()
        .filter(time_entries::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(time_entries::Column::EntryDate)
        .order_by_desc(time_entries::Column::UpdatedAt)
        .limit(row_limit)
        .all(&state.db)
        .await?;
    let recent_vendor_costs = vendor_costs::Entity::find()
        .filter(vendor_costs::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(vendor_costs::Column::UpdatedAt)
        .limit(row_limit)
        .all(&state.db)
        .await?;
    let recent_commissions = commission_entries::Entity::find()
        .filter(commission_entries::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(commission_entries::Column::UpdatedAt)
        .limit(row_limit)
        .all(&state.db)
        .await?;
    let recent_budget_forecasts = budget_forecasts::Entity::find()
        .filter(budget_forecasts::Column::ClientId.eq(client_id))
        .order_by_desc(budget_forecasts::Column::PeriodStart)
        .limit(row_limit)
        .all(&state.db)
        .await?;

    let invoice_revenue_cents: i64 = recent_invoices
        .iter()
        .map(|invoice| i64::from(invoice.amount_cents))
        .sum();
    let labor_cost_cents: i64 = recent_time_entries
        .iter()
        .map(|entry| i64::from(entry.cost_cents))
        .sum();
    let billable_value_cents: i64 = recent_time_entries
        .iter()
        .filter(|entry| entry.billable)
        .map(|entry| i64::from(entry.rate_cents) * i64::from(entry.minutes) / 60)
        .sum();
    let vendor_cost_cents: i64 = recent_vendor_costs
        .iter()
        .map(|cost| i64::from(cost.amount_cents))
        .sum();
    let commission_cents: i64 = recent_commissions
        .iter()
        .map(|commission| i64::from(commission.amount_cents))
        .sum();
    let forecast_revenue_cents: i64 = recent_budget_forecasts
        .iter()
        .map(|forecast| i64::from(forecast.revenue_cents))
        .sum();
    let forecast_gross_margin_cents: i64 = recent_budget_forecasts
        .iter()
        .map(|forecast| i64::from(forecast.gross_margin_cents))
        .sum();
    let estimated_gross_margin_cents =
        invoice_revenue_cents - labor_cost_cents - vendor_cost_cents - commission_cents;

    Ok(Json(json!({
        "clientId": client_id,
        "recentTotals": {
            "invoiceRevenueCents": invoice_revenue_cents,
            "billableValueCents": billable_value_cents,
            "laborCostCents": labor_cost_cents,
            "vendorCostCents": vendor_cost_cents,
            "commissionCents": commission_cents,
            "estimatedGrossMarginCents": estimated_gross_margin_cents,
            "forecastRevenueCents": forecast_revenue_cents,
            "forecastGrossMarginCents": forecast_gross_margin_cents
        },
        "recent": {
            "invoices": recent_invoices,
            "timeEntries": recent_time_entries,
            "vendorCosts": recent_vendor_costs,
            "commissions": recent_commissions,
            "budgetForecasts": recent_budget_forecasts
        }
    })))
}
