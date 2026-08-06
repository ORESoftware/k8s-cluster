//! Browser-facing quote client and Maud views for the dedicated Canonical API.

use std::{env, sync::Arc, time::Duration};

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use maud::{html, Markup, DOCTYPE};
use reqwest::{Client, Response, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::{auth::AuthContext, error::AppError};

const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub struct QuoteApiClient {
    base_url: String,
    http: Client,
    service_token: Arc<str>,
}

impl QuoteApiClient {
    pub fn from_env() -> Result<Self, AppError> {
        let raw_url = env::var("CANONICAL_API_URL")
            .map_err(|_| AppError::BadRequest("CANONICAL_API_URL is required".into()))?;
        let parsed = Url::parse(&raw_url)
            .map_err(|_| AppError::BadRequest("CANONICAL_API_URL must be absolute".into()))?;
        let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
            || parsed.host_str().is_some_and(|host| host.ends_with(".svc"))
            || parsed.host_str().is_some_and(|host| host.contains(".svc."));
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
            || (parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback))
        {
            return Err(AppError::BadRequest(
                "CANONICAL_API_URL must be an HTTPS origin, except for loopback or Kubernetes service DNS"
                    .into(),
            ));
        }
        let service_token = env::var("CANONICAL_WEB_SERVICE_TOKEN")
            .map_err(|_| AppError::BadRequest("CANONICAL_WEB_SERVICE_TOKEN is required".into()))?;
        if service_token.len() < 32 || service_token.trim() != service_token {
            return Err(AppError::BadRequest(
                "CANONICAL_WEB_SERVICE_TOKEN must contain at least 32 bytes".into(),
            ));
        }
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("canonical-web-server/0.1")
            .build()?;
        Ok(Self {
            base_url: parsed.origin().ascii_serialization(),
            http,
            service_token: Arc::from(service_token),
        })
    }

    pub async fn create(
        &self,
        actor: &AuthContext,
        request: &QuoteRequest,
    ) -> Result<QuoteResponse, AppError> {
        let response = self
            .http
            .post(format!("{}/v1/quotes", self.base_url))
            .headers(self.headers(actor)?)
            .json(request)
            .send()
            .await?;
        decode(response, Some(StatusCode::ACCEPTED)).await
    }

    pub async fn get(
        &self,
        actor: &AuthContext,
        quote_id: Uuid,
    ) -> Result<QuoteResponse, AppError> {
        let response = self
            .http
            .get(format!("{}/v1/quotes/{quote_id}", self.base_url))
            .headers(self.headers(actor)?)
            .send()
            .await?;
        decode(response, Some(StatusCode::OK)).await
    }

    pub async fn list(&self, actor: &AuthContext) -> Result<Vec<QuoteResponse>, AppError> {
        let response = self
            .http
            .get(format!("{}/v1/quotes", self.base_url))
            .headers(self.headers(actor)?)
            .send()
            .await?;
        decode(response, Some(StatusCode::OK)).await
    }

    fn headers(&self, actor: &AuthContext) -> Result<HeaderMap, AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-canonical-service-token",
            HeaderValue::from_str(&self.service_token).map_err(|_| AppError::Crypto)?,
        );
        headers.insert(
            "x-canonical-user-id",
            HeaderValue::from_str(&actor.user_id.to_string()).map_err(|_| AppError::Crypto)?,
        );
        if !actor.email.is_empty() {
            headers.insert(
                "x-canonical-user-email",
                HeaderValue::from_str(&actor.email).map_err(|_| AppError::Crypto)?,
            );
        }
        Ok(headers)
    }
}

async fn decode<T: DeserializeOwned>(
    response: Response,
    expected: Option<StatusCode>,
) -> Result<T, AppError> {
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::NotFound);
    }
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        return Err(AppError::BadRequest(
            "review the quote fields and try again".into(),
        ));
    }
    if expected.is_some_and(|expected| status != expected) || !status.is_success() {
        tracing::warn!(%status, "dedicated quote API rejected the request");
        return Err(AppError::ServiceUpstream);
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(AppError::ServiceUpstream);
    }
    serde_json::from_slice(&bytes).map_err(AppError::from)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub company_name: String,
    pub industry: String,
    pub employee_count: u32,
    pub annual_revenue_usd: Option<u64>,
    pub frameworks: Vec<String>,
    pub cloud_providers: Vec<String>,
    pub handles_phi: bool,
    pub handles_payment_cards: bool,
    pub security_program_maturity: String,
    pub target_timeline: String,
    pub existing_certifications: Vec<String>,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub id: Uuid,
    pub status: String,
    pub company_name: String,
    pub frameworks: Vec<String>,
    pub estimate: Option<QuoteEstimate>,
    pub analysis_markdown: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteEstimate {
    pub low: u64,
    pub high: u64,
    pub currency: String,
}

pub fn quote_page(actor: &AuthContext, quotes: &[QuoteResponse]) -> Markup {
    let csrf = actor.csrf_token.as_deref().unwrap_or_default();
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Get a quote · canonical.plus" }
                style {
                    "body{font-family:ui-sans-serif,system-ui,sans-serif;max-width:64rem;margin:0 auto;padding:2rem;line-height:1.5}.card{border:1px solid #8886;border-radius:.75rem;padding:1.25rem;margin:1rem 0}label{display:block;margin:.75rem 0}input,textarea,select,button{font:inherit;padding:.65rem}input,textarea,select{box-sizing:border-box;width:100%}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(13rem,1fr));gap:.6rem}.grid label{display:flex;gap:.5rem;align-items:center;margin:0}.grid input{width:auto}.muted{opacity:.72}.error{color:#b42318}.quote-total{font-size:1.5rem;font-weight:700}"
                }
                script type="module" src="/app-assets/app.js" {}
            }
            body {
                nav { a href="/" { "← canonical.plus" } }
                main {
                    h1 { "Get a compliance quote in less than 5 minutes" }
                    p class="muted" {
                        "Signed in as " (actor.email) ". Your answers are private to your account."
                    }
                    form class="card" method="post" action="/u/quote"
                        hx-post="/u/quote" hx-target="#quote-results" hx-swap="innerHTML" {
                        input type="hidden" name="csrf" value=(csrf);
                        h2 { "Company" }
                        label { "Company name" input name="company_name" required maxlength="200"; }
                        label { "Industry" input name="industry" required maxlength="120"; }
                        label { "Number of employees"
                            input type="number" name="employee_count" min="1" max="1000000" required;
                        }
                        label { "Annual revenue in USD (optional)"
                            input type="number" name="annual_revenue_usd" min="0" max="10000000000000";
                        }

                        h2 { "Frameworks" }
                        div class="grid" {
                            label { input type="checkbox" name="soc2"; "SOC 2" }
                            label { input type="checkbox" name="nist_csf"; "NIST CSF" }
                            label { input type="checkbox" name="nist_800_53"; "NIST SP 800-53" }
                            label { input type="checkbox" name="hipaa"; "HIPAA" }
                            label { input type="checkbox" name="iso_27001"; "ISO 27001" }
                            label { input type="checkbox" name="pci_dss"; "PCI DSS" }
                            label { input type="checkbox" name="fedramp"; "FedRAMP" }
                        }

                        h2 { "Scope" }
                        label { "Security program maturity"
                            select name="security_program_maturity" required {
                                option value="" { "Choose a stage" }
                                option value="none" { "Starting from scratch" }
                                option value="informal" { "Informal practices" }
                                option value="documented" { "Controls documented" }
                                option value="managed" { "Managed program" }
                                option value="audited" { "Previously audited" }
                            }
                        }
                        label { "Target timeline"
                            select name="target_timeline" required {
                                option value="" { "Choose a timeline" }
                                option value="under_3_months" { "Under 3 months" }
                                option value="3_to_6_months" { "3–6 months" }
                                option value="6_to_12_months" { "6–12 months" }
                                option value="over_12_months" { "More than 12 months" }
                                option value="unsure" { "Still exploring" }
                            }
                        }
                        div class="grid" {
                            label { input type="checkbox" name="handles_phi"; "Handles protected health information" }
                            label { input type="checkbox" name="handles_payment_cards"; "Handles payment-card data" }
                        }
                        label { "Cloud providers (comma-separated)"
                            input name="cloud_providers" maxlength="640" placeholder="AWS, GCP, Azure, Cloudflare";
                        }
                        label { "Existing certifications (comma-separated)"
                            input name="existing_certifications" maxlength="1920" placeholder="ISO 27001, SOC 2 Type II";
                        }
                        label { "Anything else we should know"
                            textarea name="notes" rows="5" maxlength="4000" {}
                        }
                        button type="submit" { "Analyze my quote" }
                    }
                    section id="quote-results" aria-live="polite" {
                        @for quote in quotes {
                            (quote_status_fragment(quote))
                        }
                    }
                }
            }
        }
    }
}

pub fn quote_detail_page(actor: &AuthContext, quote: &QuoteResponse) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Quote · canonical.plus" }
            }
            body {
                main {
                    p { a href="/u/quote" { "← All quotes" } }
                    p class="muted" { "Signed in as " (actor.email) }
                    (quote_status_fragment(quote))
                }
            }
        }
    }
}

pub fn quote_status_fragment(quote: &QuoteResponse) -> Markup {
    if matches!(quote.status.as_str(), "queued" | "analyzing") {
        return html! {
            article id={ "quote-" (quote.id) } class="card"
                hx-get={ "/u/quote/" (quote.id) }
                hx-trigger="every 2s"
                hx-swap="outerHTML" {
                h2 { (quote.company_name) }
                p { "Canonical's secure analysis is running." }
                p class="muted" { "This status refreshes automatically." }
            }
        };
    }
    if quote.status == "ready" {
        let estimate = quote.estimate.as_ref();
        return html! {
            article id={ "quote-" (quote.id) } class="card" {
                h2 { (quote.company_name) }
                @if let Some(estimate) = estimate {
                    p class="quote-total" {
                        "$" (estimate.low) "–$" (estimate.high) " " (&estimate.currency)
                    }
                }
                pre style="white-space:pre-wrap;font:inherit" {
                    (quote.analysis_markdown.as_deref().unwrap_or("Your preliminary quote is ready."))
                }
                p class="muted" {
                    "This is a preliminary estimate, not an audit opinion or certification."
                }
            }
        };
    }
    html! {
        article id={ "quote-" (quote.id) } class="card" {
            h2 { (quote.company_name) }
            p class="error" role="alert" {
                "We could not finish this quote. Please review your answers and try again."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_contract_uses_camel_case() {
        let value = serde_json::json!({
            "id": Uuid::nil(),
            "status": "queued",
            "companyName": "Example",
            "frameworks": ["soc2"],
            "createdAt": "2026-08-06T00:00:00Z",
            "updatedAt": "2026-08-06T00:00:00Z",
            "completedAt": null,
            "estimate": null,
            "analysisMarkdown": null,
            "failureMessage": null
        });
        let quote: QuoteResponse = serde_json::from_value(value).unwrap();
        assert_eq!(quote.company_name, "Example");
        assert_eq!(quote.frameworks, ["soc2"]);
    }
}
