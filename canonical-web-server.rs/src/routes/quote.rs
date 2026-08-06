use crate::{
    auth::{require_csrf, require_origin, QuoteSessionAuthenticated},
    error::AppError,
    quote_api::{self, QuoteRequest},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use maud::html;
use serde::Deserialize;
use uuid::Uuid;

const QUOTE_RETURN_PATH: &str = "/u/quote";
const SHARED_AUTH_BROWSER_SIGN_IN_PATH: &str = "/shared-auth/auth/browser/sign-in";

pub async fn page(
    State(state): State<AppState>,
    auth: Result<QuoteSessionAuthenticated, AppError>,
) -> Response {
    let actor = match auth {
        Ok(QuoteSessionAuthenticated(actor)) => actor,
        Err(AppError::Unauthorized) => return shared_auth_redirect(&state).into_response(),
        Err(error) => return error.into_response(),
    };
    let Some(client) = state.quote_api.as_ref() else {
        return AppError::ServiceUpstream.into_response();
    };
    match client.list(&actor).await {
        Ok(records) => quote_api::quote_page(&actor, &records).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Result<QuoteSessionAuthenticated, AppError>,
    Path(raw_id): Path<String>,
) -> Response {
    let actor = match auth {
        Ok(QuoteSessionAuthenticated(actor)) => actor,
        Err(AppError::Unauthorized) => return shared_auth_redirect(&state).into_response(),
        Err(error) => return error.into_response(),
    };
    let quote_id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return AppError::NotFound.into_response(),
    };
    let Some(client) = state.quote_api.as_ref() else {
        return AppError::ServiceUpstream.into_response();
    };
    match client.get(&actor, quote_id).await {
        Ok(record) if headers.contains_key("hx-request") => {
            quote_api::quote_status_fragment(&record).into_response()
        }
        Ok(record) => quote_api::quote_detail_page(&actor, &record).into_response(),
        Err(error) => error.into_response(),
    }
}

pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Result<QuoteSessionAuthenticated, AppError>,
    Form(form): Form<QuoteForm>,
) -> Response {
    let actor = match auth {
        Ok(QuoteSessionAuthenticated(actor)) => actor,
        Err(AppError::Unauthorized) => return htmx_or_browser_auth_redirect(&headers, &state),
        Err(error) => return error.into_response(),
    };
    if let Err(error) = require_origin(&headers, &state) {
        return error.into_response();
    }
    if let Err(error) = require_csrf(&actor, &headers, Some(&form.csrf)) {
        return error.into_response();
    }
    let request = match form.into_request() {
        Ok(request) => request,
        Err(AppError::BadRequest(message)) => return form_error(&headers, &message),
        Err(error) => return error.into_response(),
    };
    let Some(client) = state.quote_api.as_ref() else {
        return AppError::ServiceUpstream.into_response();
    };
    match client.create(&actor, &request).await {
        Ok(record) if headers.contains_key("hx-request") => {
            quote_api::quote_status_fragment(&record).into_response()
        }
        Ok(record) => Redirect::to(&format!("/u/quote/{}", record.id)).into_response(),
        Err(AppError::BadRequest(message)) => form_error(&headers, &message),
        Err(error) => error.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct QuoteForm {
    csrf: String,
    company_name: String,
    industry: String,
    employee_count: u32,
    #[serde(default)]
    annual_revenue_usd: String,
    security_program_maturity: String,
    target_timeline: String,
    #[serde(default)]
    cloud_providers: String,
    #[serde(default)]
    existing_certifications: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    soc2: Option<String>,
    #[serde(default)]
    nist_csf: Option<String>,
    #[serde(default)]
    nist_800_53: Option<String>,
    #[serde(default)]
    hipaa: Option<String>,
    #[serde(default)]
    iso_27001: Option<String>,
    #[serde(default)]
    fedramp: Option<String>,
    #[serde(default)]
    pci_dss: Option<String>,
    #[serde(default)]
    handles_phi: Option<String>,
    #[serde(default)]
    handles_payment_cards: Option<String>,
}

impl QuoteForm {
    fn into_request(self) -> Result<QuoteRequest, AppError> {
        let company_name = self.company_name.trim().to_owned();
        if company_name.is_empty() || company_name.chars().count() > 200 {
            return Err(AppError::BadRequest(
                "company name is required and must be at most 200 characters".into(),
            ));
        }
        let industry = self.industry.trim().to_owned();
        if industry.is_empty() || industry.chars().count() > 120 {
            return Err(AppError::BadRequest(
                "industry is required and must be at most 120 characters".into(),
            ));
        }
        if !(1..=1_000_000).contains(&self.employee_count) {
            return Err(AppError::BadRequest(
                "employee count must be between 1 and 1000000".into(),
            ));
        }
        let annual_revenue_usd = match self.annual_revenue_usd.trim() {
            "" => None,
            value => Some(value.parse::<u64>().map_err(|_| {
                AppError::BadRequest("annual revenue must be a whole USD amount".into())
            })?),
        };
        if annual_revenue_usd.is_some_and(|value| value > 10_000_000_000_000) {
            return Err(AppError::BadRequest(
                "annual revenue is outside the supported range".into(),
            ));
        }
        let frameworks = [
            ("soc2", self.soc2),
            ("nist_csf", self.nist_csf),
            ("nist_800_53", self.nist_800_53),
            ("hipaa", self.hipaa),
            ("iso_27001", self.iso_27001),
            ("fedramp", self.fedramp),
            ("pci_dss", self.pci_dss),
        ]
        .into_iter()
        .filter_map(|(name, selected)| selected.map(|_| name.to_owned()))
        .collect::<Vec<_>>();
        if frameworks.is_empty() {
            return Err(AppError::BadRequest(
                "choose at least one supported framework".into(),
            ));
        }
        if !matches!(
            self.security_program_maturity.as_str(),
            "none" | "informal" | "documented" | "managed" | "audited"
        ) {
            return Err(AppError::BadRequest(
                "choose a supported security program maturity".into(),
            ));
        }
        if !matches!(
            self.target_timeline.as_str(),
            "under_3_months" | "3_to_6_months" | "6_to_12_months" | "over_12_months" | "unsure"
        ) {
            return Err(AppError::BadRequest(
                "choose a supported target timeline".into(),
            ));
        }
        let notes = optional(self.notes);
        if notes.as_deref().is_some_and(|value| value.len() > 4_000) {
            return Err(AppError::BadRequest(
                "notes must be at most 4000 characters".into(),
            ));
        }

        Ok(QuoteRequest {
            company_name,
            industry,
            employee_count: self.employee_count,
            annual_revenue_usd,
            frameworks,
            cloud_providers: split_list(&self.cloud_providers, 8, 80),
            handles_phi: self.handles_phi.is_some(),
            handles_payment_cards: self.handles_payment_cards.is_some(),
            security_program_maturity: self.security_program_maturity,
            target_timeline: self.target_timeline,
            existing_certifications: split_list(&self.existing_certifications, 16, 120),
            notes,
        })
    }
}

fn split_list(value: &str, maximum_entries: usize, maximum_length: usize) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(maximum_entries)
        .map(|value| value.chars().take(maximum_length).collect())
        .collect()
}

fn optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn shared_auth_sign_in_url(app_base_url: &str) -> reqwest::Url {
    let mut destination = reqwest::Url::parse(app_base_url)
        .expect("APP_BASE_URL was validated before application state construction");
    destination.set_path(SHARED_AUTH_BROWSER_SIGN_IN_PATH);
    destination.set_query(None);
    destination
        .query_pairs_mut()
        .append_pair("client_id", "canonical-web")
        .append_pair("return", QUOTE_RETURN_PATH);
    destination
}

fn shared_auth_redirect(state: &AppState) -> Redirect {
    let destination = shared_auth_sign_in_url(&state.config.app_base_url);
    Redirect::temporary(destination.as_str())
}

fn htmx_or_browser_auth_redirect(headers: &HeaderMap, state: &AppState) -> Response {
    let destination = shared_auth_sign_in_url(&state.config.app_base_url);
    if headers.contains_key("hx-request") {
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        if let Ok(value) = HeaderValue::from_str(destination.as_str()) {
            response.headers_mut().insert("hx-redirect", value);
        }
        response
    } else {
        Redirect::temporary(destination.as_str()).into_response()
    }
}

fn form_error(headers: &HeaderMap, message: &str) -> Response {
    let fragment = html! { p class="error" role="alert" { (message) } };
    if headers.contains_key("hx-request") {
        let mut response = (StatusCode::UNPROCESSABLE_ENTITY, fragment).into_response();
        response
            .headers_mut()
            .insert("hx-retarget", HeaderValue::from_static("#quote-results"));
        response
    } else {
        (StatusCode::UNPROCESSABLE_ENTITY, fragment).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_maps_selected_frameworks_and_scope() {
        let form = QuoteForm {
            csrf: "csrf".into(),
            company_name: "Example".into(),
            industry: "Software".into(),
            employee_count: 15,
            annual_revenue_usd: "1000000".into(),
            security_program_maturity: "documented".into(),
            target_timeline: "3_to_6_months".into(),
            cloud_providers: "AWS, Cloudflare".into(),
            existing_certifications: String::new(),
            notes: String::new(),
            soc2: Some("on".into()),
            nist_csf: None,
            nist_800_53: None,
            hipaa: Some("on".into()),
            iso_27001: None,
            fedramp: None,
            pci_dss: None,
            handles_phi: Some("on".into()),
            handles_payment_cards: None,
        };
        let request = form.into_request().unwrap();
        assert_eq!(request.frameworks, ["soc2", "hipaa"]);
        assert_eq!(request.cloud_providers, ["AWS", "Cloudflare"]);
        assert!(request.handles_phi);
    }

    #[test]
    fn auth_return_target_is_same_origin_and_relative() {
        let destination = shared_auth_sign_in_url("https://app.canonical.plus");
        assert_eq!(destination.host_str(), Some("app.canonical.plus"));
        assert_eq!(destination.path(), SHARED_AUTH_BROWSER_SIGN_IN_PATH);
        assert_eq!(
            destination
                .query_pairs()
                .find(|(name, _)| name == "return")
                .map(|(_, value)| value.into_owned()),
            Some(QUOTE_RETURN_PATH.into())
        );
        assert_eq!(
            destination
                .query_pairs()
                .find(|(name, _)| name == "client_id")
                .map(|(_, value)| value.into_owned()),
            Some("canonical-web".into())
        );
    }
}
