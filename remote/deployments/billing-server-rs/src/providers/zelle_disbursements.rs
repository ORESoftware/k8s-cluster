//! Bank-sponsored Zelle disbursement APIs.
//!
//! Zelle itself is not a public merchant API. Programmatic access is offered
//! through participating treasury banks and enterprise disbursement products.
//! These clients model the bank-sponsored surfaces we can support safely:
//!
//! * J.P. Morgan Global Payments Zelle Disbursements, whose public docs publish
//!   `/tsapi/v1/payments` and `/tsapi/v1/payments/status`.
//! * U.S. Bank Disbursements via Zelle, whose public portal describes
//!   enrollment checks, payment submission, search, retry, and delete; endpoint
//!   paths remain configurable for bank-provided tenant docs.
//! * Bank of America CashPro Global Digital Disbursements, whose public access
//!   form names "Global Digital Disbursements (Paypal, Zelle)" but keeps
//!   endpoint details behind CashPro onboarding.
//!
//! These provider kinds remain `LimitedFit` in this billing server. The clients
//! are typed and mock-tested, but no automatic sync or money movement is wired
//! into public routes.

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// --- Shared Zelle DTOs ------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZelleAliasKind {
    Email,
    Mobile,
}

impl ZelleAliasKind {
    pub fn jpmorgan_scheme(&self) -> &'static str {
        match self {
            Self::Email => "EMAL",
            Self::Mobile => "TELI",
        }
    }

    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::Mobile => "MOBILE",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZelleAlias {
    pub kind: ZelleAliasKind,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZelleDisbursementInput {
    pub end_to_end_id: String,
    pub amount: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub recipient_name: String,
    pub recipient_alias: ZelleAlias,
    pub memo: Option<String>,
    pub requested_execution_date: Option<NaiveDate>,
}

fn default_currency() -> String {
    "USD".into()
}

// --- J.P. Morgan ------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct JpmorganZelleCredential {
    pub access_token: String,
    pub debtor_account_id: String,
    pub debtor_name: String,
    #[serde(default = "default_jpm_bic")]
    pub debtor_bic: String,
    #[serde(default = "default_env")]
    pub environment: String,
    pub api_base_url: Option<String>,
}

impl fmt::Debug for JpmorganZelleCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JpmorganZelleCredential")
            .field("access_token", &"<redacted>")
            .field("debtor_account_id", &self.debtor_account_id)
            .field("debtor_name", &self.debtor_name)
            .field("debtor_bic", &self.debtor_bic)
            .field("environment", &self.environment)
            .field("api_base_url", &self.api_base_url)
            .finish()
    }
}

fn default_jpm_bic() -> String {
    "CHASUS33".into()
}

impl JpmorganZelleCredential {
    pub fn base_url(&self) -> AppResult<String> {
        if let Some(url) = self.api_base_url.as_deref() {
            return normalize_base_url("jpmorgan_zelle.api_base_url", url, false);
        }
        if self.environment.eq_ignore_ascii_case("sandbox") {
            Ok("https://api-mock.payments.jpmorgan.com/tsapi/v1".into())
        } else {
            Ok("https://apigateway.jpmorgan.com/tsapi/v1".into())
        }
    }
}

#[derive(Clone)]
pub struct JpmorganZelleApi {
    cred: JpmorganZelleCredential,
    http: reqwest::Client,
    base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JpmorganZelleInitiationResponse {
    #[serde(rename = "paymentInitiationResponse")]
    pub payment_initiation_response: Option<JpmorganPaymentInitiation>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JpmorganPaymentInitiation {
    #[serde(rename = "firmRootId")]
    pub firm_root_id: Option<String>,
    #[serde(rename = "endToEndId")]
    pub end_to_end_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JpmorganZelleStatusResponse {
    #[serde(rename = "paymentStatus")]
    pub payment_status: Option<JpmorganPaymentStatus>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JpmorganPaymentStatus {
    pub status: Option<String>,
    #[serde(rename = "createDateTime")]
    pub create_date_time: Option<String>,
    #[serde(rename = "firmRootId")]
    pub firm_root_id: Option<String>,
    #[serde(rename = "endToEndId")]
    pub end_to_end_id: Option<String>,
}

impl JpmorganZelleApi {
    pub fn new(cred: JpmorganZelleCredential) -> AppResult<Self> {
        let base_url = cred.base_url()?;
        Ok(Self {
            cred,
            http: http_client("jpmorgan_zelle")?,
            base_url,
        })
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: JpmorganZelleCredential, base_url: String) -> Self {
        Self {
            cred,
            http: http_client("jpmorgan_zelle").expect("build JPMorgan Zelle test HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn initiate_zelle_payment(
        &self,
        input: ZelleDisbursementInput,
    ) -> AppResult<JpmorganZelleInitiationResponse> {
        validate_disbursement_input("jpmorgan_zelle", &input)?;
        let requested_execution_date = input
            .requested_execution_date
            .unwrap_or_else(|| Utc::now().date_naive())
            .to_string();
        let payload = serde_json::json!({
            "payments": {
                "requestedExecutionDate": requested_execution_date,
                "paymentIdentifiers": {
                    "endToEndId": input.end_to_end_id
                },
                "paymentCurrency": input.currency,
                "paymentAmount": input.amount,
                "transferType": "CREDIT",
                "debtor": {
                    "debtorName": &self.cred.debtor_name,
                    "debtorAccount": {
                        "accountId": &self.cred.debtor_account_id
                    }
                },
                "debtorAgent": {
                    "financialInstitutionId": {
                        "bic": &self.cred.debtor_bic
                    }
                },
                "creditor": {
                    "creditorName": input.recipient_name,
                    "creditorAccount": {
                        "accountType": "ZELLE",
                        "alternateAccountIdentifier": input.recipient_alias.value,
                        "schemeName": {
                            "proprietary": input.recipient_alias.kind.jpmorgan_scheme()
                        }
                    }
                },
                "remittanceInformation": input.memo.map(|memo| serde_json::json!({
                    "unstructuredInformation": [{ "text": memo }]
                }))
            }
        });
        let url = format!("{}/payments", self.base_url);
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Request-Id", Uuid::new_v4().to_string())
            .json(&payload)
            .send()
            .await
            .map_err(|e| provider_err("jpmorgan_zelle", format!("initiate HTTP: {e}")))?;
        decode_json_response("jpmorgan_zelle", resp, "initiate").await
    }

    pub async fn get_status_by_end_to_end_id(
        &self,
        end_to_end_id: &str,
    ) -> AppResult<JpmorganZelleStatusResponse> {
        let end_to_end_id = required("jpmorgan_zelle.end_to_end_id", end_to_end_id)?;
        let qs = serde_urlencoded::to_string(&[("endToEndId", end_to_end_id)])
            .map_err(|e| provider_err("jpmorgan_zelle", format!("status query encode: {e}")))?;
        let url = format!("{}/payments/status?{qs}", self.base_url);
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/json")
            .header("Request-Id", Uuid::new_v4().to_string())
            .send()
            .await
            .map_err(|e| provider_err("jpmorgan_zelle", format!("status HTTP: {e}")))?;
        decode_json_response("jpmorgan_zelle", resp, "status").await
    }
}

// --- U.S. Bank --------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct UsBankZelleCredential {
    pub access_token: String,
    pub client_id: String,
    pub program_id: String,
    pub api_base_url: String,
    #[serde(default = "default_payment_path")]
    pub payments_path: String,
    #[serde(default = "default_enrollment_path")]
    pub enrollment_path: String,
    #[serde(default = "default_env")]
    pub environment: String,
}

impl fmt::Debug for UsBankZelleCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UsBankZelleCredential")
            .field("access_token", &"<redacted>")
            .field("client_id", &self.client_id)
            .field("program_id", &self.program_id)
            .field("api_base_url", &self.api_base_url)
            .field("payments_path", &self.payments_path)
            .field("enrollment_path", &self.enrollment_path)
            .field("environment", &self.environment)
            .finish()
    }
}

#[derive(Clone)]
pub struct UsBankZelleApi {
    cred: UsBankZelleCredential,
    http: reqwest::Client,
    base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UsBankZellePaymentResponse {
    #[serde(default, rename = "paymentId", alias = "payment_id")]
    pub payment_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "endToEndId", alias = "end_to_end_id")]
    pub end_to_end_id: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UsBankZelleEnrollmentResponse {
    #[serde(default)]
    pub aliases: Vec<UsBankZelleAliasStatus>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UsBankZelleAliasStatus {
    pub value: String,
    #[serde(default)]
    pub enrolled: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
}

impl UsBankZelleApi {
    pub fn new(cred: UsBankZelleCredential) -> AppResult<Self> {
        let base_url = normalize_base_url("us_bank_zelle.api_base_url", &cred.api_base_url, false)?;
        Ok(Self {
            cred,
            http: http_client("us_bank_zelle")?,
            base_url,
        })
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: UsBankZelleCredential, base_url: String) -> Self {
        Self {
            cred,
            http: http_client("us_bank_zelle").expect("build U.S. Bank Zelle test HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn check_enrollment(
        &self,
        aliases: Vec<ZelleAlias>,
    ) -> AppResult<UsBankZelleEnrollmentResponse> {
        if aliases.is_empty() || aliases.len() > 50 {
            return Err(AppError::BadRequest(
                "us_bank_zelle aliases must contain 1 to 50 entries".into(),
            ));
        }
        for alias in &aliases {
            validate_zelle_alias("us_bank_zelle", alias)?;
        }
        let payload = serde_json::json!({
            "programId": &self.cred.program_id,
            "aliases": aliases.into_iter().map(|alias| serde_json::json!({
                "type": alias.kind.wire_value(),
                "value": alias.value
            })).collect::<Vec<_>>()
        });
        let url = url_with_path(&self.base_url, &self.cred.enrollment_path, "us_bank_zelle")?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Client-Id", &self.cred.client_id)
            .json(&payload)
            .send()
            .await
            .map_err(|e| provider_err("us_bank_zelle", format!("enrollment HTTP: {e}")))?;
        decode_json_response("us_bank_zelle", resp, "enrollment").await
    }

    pub async fn submit_payment(
        &self,
        input: ZelleDisbursementInput,
    ) -> AppResult<UsBankZellePaymentResponse> {
        validate_disbursement_input("us_bank_zelle", &input)?;
        let payload = serde_json::json!({
            "programId": &self.cred.program_id,
            "endToEndId": input.end_to_end_id,
            "amount": {
                "value": input.amount,
                "currency": input.currency
            },
            "recipient": {
                "name": input.recipient_name,
                "alias": {
                    "type": input.recipient_alias.kind.wire_value(),
                    "value": input.recipient_alias.value
                }
            },
            "memo": input.memo,
            "requestedExecutionDate": input.requested_execution_date.map(|d| d.to_string())
        });
        let url = url_with_path(&self.base_url, &self.cred.payments_path, "us_bank_zelle")?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Client-Id", &self.cred.client_id)
            .json(&payload)
            .send()
            .await
            .map_err(|e| provider_err("us_bank_zelle", format!("payment HTTP: {e}")))?;
        decode_json_response("us_bank_zelle", resp, "payment").await
    }
}

// --- Bank of America CashPro GDD -------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct BofaCashProGddCredential {
    pub client_id: String,
    pub client_secret: String,
    pub cashpro_company_id: String,
    pub access_token: Option<String>,
    pub api_base_url: String,
    pub disbursements_path: String,
    #[serde(default = "default_env")]
    pub environment: String,
}

impl fmt::Debug for BofaCashProGddCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BofaCashProGddCredential")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("cashpro_company_id", &self.cashpro_company_id)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field("api_base_url", &self.api_base_url)
            .field("disbursements_path", &self.disbursements_path)
            .field("environment", &self.environment)
            .finish()
    }
}

#[derive(Clone)]
pub struct BofaCashProGddApi {
    cred: BofaCashProGddCredential,
    http: reqwest::Client,
    base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BofaCashProGddResponse {
    #[serde(default, rename = "disbursementId", alias = "disbursement_id")]
    pub disbursement_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "endToEndId", alias = "end_to_end_id")]
    pub end_to_end_id: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

impl BofaCashProGddApi {
    pub fn new(cred: BofaCashProGddCredential) -> AppResult<Self> {
        let base_url =
            normalize_base_url("bofa_cashpro_gdd.api_base_url", &cred.api_base_url, false)?;
        Ok(Self {
            cred,
            http: http_client("bofa_cashpro_gdd")?,
            base_url,
        })
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: BofaCashProGddCredential, base_url: String) -> Self {
        Self {
            cred,
            http: http_client("bofa_cashpro_gdd").expect("build BofA GDD test HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn submit_disbursement(
        &self,
        input: ZelleDisbursementInput,
    ) -> AppResult<BofaCashProGddResponse> {
        validate_disbursement_input("bofa_cashpro_gdd", &input)?;
        let access_token = self.cred.access_token.as_deref().ok_or_else(|| {
            AppError::BadRequest("bofa_cashpro_gdd.access_token is required for API calls".into())
        })?;
        let payload = serde_json::json!({
            "cashProCompanyId": &self.cred.cashpro_company_id,
            "rail": "ZELLE",
            "endToEndId": input.end_to_end_id,
            "amount": input.amount,
            "currency": input.currency,
            "recipient": {
                "name": input.recipient_name,
                "aliasType": input.recipient_alias.kind.wire_value(),
                "alias": input.recipient_alias.value
            },
            "memo": input.memo,
            "requestedExecutionDate": input.requested_execution_date.map(|d| d.to_string())
        });
        let url = url_with_path(
            &self.base_url,
            &self.cred.disbursements_path,
            "bofa_cashpro_gdd",
        )?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Client-Id", &self.cred.client_id)
            .json(&payload)
            .send()
            .await
            .map_err(|e| provider_err("bofa_cashpro_gdd", format!("disbursement HTTP: {e}")))?;
        decode_json_response("bofa_cashpro_gdd", resp, "disbursement").await
    }
}

// --- Helpers ----------------------------------------------------------------

fn default_env() -> String {
    "production".into()
}

fn default_payment_path() -> String {
    "/payments".into()
}

fn default_enrollment_path() -> String {
    "/enrollments".into()
}

pub fn validate_zelle_api_base_url(field: &str, value: &str) -> AppResult<String> {
    normalize_base_url(field, value, false)
}

pub fn validate_zelle_path(field: &str, value: &str) -> AppResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return Err(AppError::BadRequest(format!(
            "{field} must be a root-relative API path"
        )));
    }
    if trimmed.starts_with("//")
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(format!(
            "{field} must not contain URL authority, query, fragment, or control characters"
        )));
    }
    Ok(())
}

fn http_client(provider: &str) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| provider_err(provider, format!("HTTP client build: {e}")))
}

fn normalize_base_url(field: &str, value: &str, allow_http_for_tests: bool) -> AppResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| AppError::BadRequest(format!("{field} must be a valid URL: {e}")))?;
    if parsed.scheme() != "https" && !(allow_http_for_tests && parsed.scheme() == "http") {
        return Err(AppError::BadRequest(format!("{field} must use https")));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not include URL credentials"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not include query or fragment components"
        )));
    }
    if !allow_http_for_tests {
        validate_runtime_host(field, &parsed)?;
    }
    Ok(trimmed.to_string())
}

fn validate_runtime_host(field: &str, parsed: &url::Url) -> AppResult<()> {
    let Some(host) = parsed.host() else {
        return Err(AppError::BadRequest(format!("{field} must include a host")));
    };
    match host {
        url::Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
                || !host.contains('.')
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must use a public provider hostname"
                )));
            }
        }
        url::Host::Ipv4(addr) => {
            if addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_unspecified()
                || addr.is_broadcast()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
        url::Host::Ipv6(addr) => {
            if addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
    }
    Ok(())
}

fn url_with_path(base_url: &str, path: &str, provider: &str) -> AppResult<String> {
    validate_zelle_path(&format!("{provider}.path"), path)?;
    let mut url = url::Url::parse(base_url)
        .map_err(|e| provider_err(provider, format!("base URL parse: {e}")))?;
    url.set_path(path.trim_start_matches('/'));
    Ok(url.to_string())
}

fn validate_disbursement_input(provider: &str, input: &ZelleDisbursementInput) -> AppResult<()> {
    required(&format!("{provider}.end_to_end_id"), &input.end_to_end_id)?;
    required(&format!("{provider}.recipient_name"), &input.recipient_name)?;
    validate_zelle_alias(provider, &input.recipient_alias)?;
    if input.amount <= 0.0 || !input.amount.is_finite() {
        return Err(AppError::BadRequest(format!(
            "{provider}.amount must be a positive finite number"
        )));
    }
    if input.currency.trim().len() != 3
        || !input
            .currency
            .trim()
            .bytes()
            .all(|b| b.is_ascii_uppercase())
    {
        return Err(AppError::BadRequest(format!(
            "{provider}.currency must be a 3-letter uppercase ISO currency code"
        )));
    }
    Ok(())
}

fn validate_zelle_alias(provider: &str, alias: &ZelleAlias) -> AppResult<()> {
    let value = required(&format!("{provider}.recipient_alias.value"), &alias.value)?;
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AppError::BadRequest(format!(
            "{provider}.recipient_alias.value is invalid"
        )));
    }
    match alias.kind {
        ZelleAliasKind::Email => {
            if !value.contains('@') {
                return Err(AppError::BadRequest(format!(
                    "{provider}.recipient_alias.value must be an email address"
                )));
            }
        }
        ZelleAliasKind::Mobile => {
            if !value
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'(' | b')' | b' '))
            {
                return Err(AppError::BadRequest(format!(
                    "{provider}.recipient_alias.value must be a phone-like value"
                )));
            }
        }
    }
    Ok(())
}

fn required(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}

async fn decode_json_response<T: for<'de> Deserialize<'de>>(
    provider: &str,
    resp: reqwest::Response,
    label: &str,
) -> AppResult<T> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| provider_err(provider, format!("{label} body: {e}")))?;
    if !status.is_success() {
        return Err(provider_err(
            provider,
            format!("{label} {status}: {}", String::from_utf8_lossy(&bytes)),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| provider_err(provider, format!("{label} decode: {e}")))
}

fn provider_err(provider: &str, message: String) -> AppError {
    AppError::Provider {
        provider: provider.into(),
        message,
    }
}
