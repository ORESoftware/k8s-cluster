use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::ProviderAuthKind;
use crate::providers::connection::{CreateConnection, ProviderConnection, UpsertCredential};
use crate::providers::{
    ProviderKind, adyen::AdyenCredential, coinbase::CoinbaseCredential,
    coinflow::CoinflowCredential, dwolla::DwollaCredential, ethereum::EthereumWalletCredential,
    modern_treasury::ModernTreasuryCredential, square::SquareCredential, wise::WiseCredential,
};
use crate::scheduler::{CreateScheduledJob, ScheduleKind, ScheduledJob};
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<ProviderConnection>>> {
    let rows = state.connections.list_for_tenant(tenant_id).await?;
    Ok(Json(rows))
}

pub async fn create(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateConnection>,
) -> AppResult<Json<ProviderConnection>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let conn = state.connections.create(tenant_id, region, input).await?;
    Ok(Json(conn))
}

#[derive(Debug, Default, Deserialize)]
pub struct SyncNowRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    /// "user", "webhook", "api", etc. Recorded on the lease + run for audit.
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Serialize)]
pub struct SyncNowResponse {
    /// The one-shot scheduled job that will execute the sync.
    pub job: ScheduledJob,
    /// Convenience hint for clients: poll `runs_url` to see the result.
    pub runs_url: String,
}

/// On-demand sync trigger. This is the *primary* sync mechanism — the
/// backstop poller (default 5x/day) only catches what this missed. Returns
/// quickly with a job handle the client can poll for results.
pub async fn sync_now(
    State(state): State<AppState>,
    Path((tenant_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SyncNowRequest>,
) -> AppResult<(StatusCode, Json<SyncNowResponse>)> {
    // Validate the connection exists for this tenant up front so we don't
    // queue garbage. The job handler also validates.
    let _conn = state.connections.get(tenant_id, connection_id).await?;
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;

    let payload = serde_json::json!({
        "connection_id": connection_id,
        "cursor": req.cursor,
        "trigger": req.trigger.unwrap_or_else(|| "on_demand".into()),
    });

    let job = state
        .scheduler
        .enqueue_one_shot(
            tenant_id,
            region,
            "sync.connection",
            format!("on-demand-conn-{}", connection_id),
            payload,
        )
        .await?;

    let runs_url = format!(
        "/v1/tenants/{tenant_id}/scheduled-jobs/{}/runs?limit=1",
        job.id
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(SyncNowResponse { job, runs_url }),
    ))
}

// --- API-key attach (Coinflow / Coinbase / Wise / any non-OAuth provider) --

#[derive(Debug, Deserialize)]
pub struct AttachApiKeyRequest {
    /// Provider-specific credential payload, as JSON. The shape depends on
    /// the provider (see each provider's `*Credential` struct, e.g.
    /// `CoinflowCredential { api_key, merchant_id, environment,
    /// webhook_validation_key }`). We seal these bytes as-is.
    pub credential: serde_json::Value,
    /// Optional: lets the caller stamp the connection with the merchant id
    /// they just pasted, before sync ever runs.
    pub external_account_id: Option<String>,
    /// "production" | "sandbox" — recorded as connection metadata for ops
    /// visibility; the provider's own credential payload is the actual
    /// source of truth.
    pub environment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttachApiKeyResponse {
    pub connection_id: Uuid,
    pub status: &'static str,
    pub backstop_job_id: Uuid,
}

/// `POST /v1/tenants/{tenant_id}/connections/{connection_id}/attach-api-key`
///
/// For non-OAuth providers (Coinflow, Coinbase, Wise, etc.), the tenant
/// pastes their API key + merchant id into our dashboard. We seal the
/// provider-specific credential JSON, flip the connection to `active`,
/// and auto-register the backstop sync job. Mirror of what the OAuth
/// callback does for OAuth providers — same end state.
pub async fn attach_api_key(
    State(state): State<AppState>,
    Path((tenant_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<AttachApiKeyRequest>,
) -> AppResult<(StatusCode, Json<AttachApiKeyResponse>)> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;

    let conn = state.connections.get(tenant_id, connection_id).await?;

    if conn.auth_kind != ProviderAuthKind::ApiKey {
        return Err(AppError::BadRequest(format!(
            "connection {connection_id} ({}) does not use api_key auth; \
             use the OAuth flow instead",
            conn.provider.tag()
        )));
    }

    let derived_external_account_id = validate_api_key_credential(conn.provider, &req.credential)?;

    let plaintext = serde_json::to_vec(&req.credential)
        .map_err(|e| AppError::BadRequest(format!("credential must be a JSON object: {e}")))?;

    let _ = state
        .connections
        .attach_credential(
            tenant_id,
            connection_id,
            UpsertCredential {
                plaintext,
                scopes: vec![],
                expires_at: None,
            },
        )
        .await?;

    let external_account_id = req
        .external_account_id
        .as_deref()
        .map(str::to_string)
        .or(derived_external_account_id);

    if let Some(ext) = external_account_id.as_deref() {
        let _ = state
            .connections
            .set_external_account(tenant_id, connection_id, ext)
            .await;
    }

    if let Some(env) = req.environment.as_deref() {
        let _ = state
            .connections
            .merge_metadata(
                tenant_id,
                connection_id,
                serde_json::json!({ "environment": env }),
            )
            .await;
    }

    let backstop = state
        .scheduler
        .create(
            Some(tenant_id),
            Some(region),
            CreateScheduledJob {
                kind: "sync.connection".into(),
                name: format!("backstop-conn-{}", connection_id),
                schedule_kind: ScheduleKind::Interval,
                cron_expr: None,
                interval_seconds: Some(18_000),
                one_shot_at: None,
                timezone: "UTC".into(),
                payload: serde_json::json!({
                    "connection_id": connection_id,
                    "trigger": "backstop"
                }),
                enabled: true,
                max_attempts: 3,
                retry_backoff_secs: 300,
                timeout_seconds: 600,
            },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(AttachApiKeyResponse {
            connection_id,
            status: "active",
            backstop_job_id: backstop.id,
        }),
    ))
}

fn validate_api_key_credential(
    provider: ProviderKind,
    credential: &serde_json::Value,
) -> AppResult<Option<String>> {
    match provider {
        ProviderKind::Coinflow => {
            let cred: CoinflowCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid coinflow credential: {e}")))?;
            require_non_empty("coinflow.api_key", &cred.api_key)?;
            require_non_empty("coinflow.merchant_id", &cred.merchant_id)?;
            validate_environment("coinflow.environment", &cred.environment)?;
            Ok(Some(cred.merchant_id))
        }
        ProviderKind::CoinbaseCommerce => {
            let cred: CoinbaseCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid coinbase credential: {e}")))?;
            require_non_empty("coinbase.api_key", &cred.api_key)?;
            require_non_empty("coinbase.webhook_secret", &cred.webhook_secret)?;
            Ok(None)
        }
        ProviderKind::CoinbasePrime => {
            let cred: CoinbaseCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid coinbase credential: {e}")))?;
            require_non_empty("coinbase.api_key", &cred.api_key)?;
            require_non_empty("coinbase.webhook_secret", &cred.webhook_secret)?;
            // Prime additionally requires the HMAC secret + passphrase
            // + portfolio_id to issue signed REST requests against
            // /v1/portfolios/{id}/transactions.
            require_non_empty_opt("coinbase.api_secret", cred.api_secret.as_deref())?;
            require_non_empty_opt("coinbase.passphrase", cred.passphrase.as_deref())?;
            require_non_empty_opt("coinbase.portfolio_id", cred.portfolio_id.as_deref())?;
            Ok(cred.portfolio_id.clone())
        }
        ProviderKind::Wise => {
            let cred: WiseCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid wise credential: {e}")))?;
            require_non_empty("wise.api_token", &cred.api_token)?;
            require_non_empty("wise.profile_id", &cred.profile_id)?;
            validate_environment("wise.environment", &cred.environment)?;
            Ok(Some(cred.profile_id))
        }
        ProviderKind::Revolut => {
            let cred: crate::providers::revolut::RevolutCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid revolut credential: {e}"))
                })?;
            require_non_empty("revolut.access_token", &cred.access_token)?;
            validate_environment("revolut.environment", &cred.environment)?;
            Ok(None)
        }
        ProviderKind::Mercury => {
            let cred: crate::providers::mercury::MercuryCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid mercury credential: {e}"))
                })?;
            require_non_empty("mercury.api_key", &cred.api_key)?;
            Ok(None)
        }
        ProviderKind::Bridge => {
            let cred: crate::providers::bridge::BridgeCredential =
                serde_json::from_value(credential.clone())
                    .map_err(|e| AppError::BadRequest(format!("invalid bridge credential: {e}")))?;
            require_non_empty("bridge.api_key", &cred.api_key)?;
            validate_environment("bridge.environment", &cred.environment)?;
            Ok(None)
        }
        ProviderKind::GoCardless => {
            let cred: crate::providers::gocardless::GoCardlessCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid gocardless credential: {e}"))
                })?;
            require_non_empty("gocardless.access_token", &cred.access_token)?;
            // gocardless uses "live"/"sandbox", not "production"/"sandbox" —
            // accept whatever the tenant sends and validate against its own list
            let env = cred.environment.trim().to_lowercase();
            if !matches!(env.as_str(), "live" | "sandbox") {
                return Err(AppError::BadRequest(format!(
                    "gocardless.environment must be 'live' or 'sandbox' (got {env})"
                )));
            }
            Ok(None)
        }
        ProviderKind::Remitly => {
            let cred: crate::providers::remitly::RemitlyCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid remitly credential: {e}"))
                })?;
            // Remitly is limited_fit and can remain intent-only. If the
            // tenant has partner/export credentials, validate the pieces that
            // would make typed API calls usable.
            let api_key = optional_trimmed(cred.api_key.as_deref());
            let api_base_url = optional_trimmed(cred.api_base_url.as_deref());
            if let Some(partner_id) = cred.partner_id.as_deref() {
                require_non_empty("remitly.partner_id", partner_id)?;
            }
            match (api_key, api_base_url) {
                (Some(api_key), Some(api_base_url)) => {
                    require_non_empty("remitly.api_key", api_key)?;
                    crate::providers::remitly::validate_partner_base_url(api_base_url)?;
                }
                (None, None) => {}
                _ => {
                    return Err(AppError::BadRequest(
                        "remitly.api_key and remitly.api_base_url must be provided together".into(),
                    ));
                }
            }
            validate_environment("remitly.environment", &cred.environment)?;
            Ok(cred.partner_id.clone())
        }
        ProviderKind::MoneyGram => {
            let cred: crate::providers::moneygram::MoneyGramCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid moneygram credential: {e}"))
                })?;
            require_non_empty("moneygram.client_id", &cred.client_id)?;
            require_non_empty("moneygram.client_secret", &cred.client_secret)?;
            require_non_empty("moneygram.agent_partner_id", &cred.agent_partner_id)?;
            require_non_empty("moneygram.user_language", &cred.user_language)?;
            validate_language_tag("moneygram.user_language", &cred.user_language)?;
            validate_environment("moneygram.environment", &cred.environment)?;
            Ok(Some(cred.agent_partner_id))
        }
        ProviderKind::WesternUnion => {
            let cred: crate::providers::western_union::WesternUnionCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid western_union credential: {e}"))
                })?;
            require_non_empty("western_union.client_id", &cred.client_id)?;
            validate_environment("western_union.environment", &cred.environment)?;
            match (
                cred.client_certificate_pem.as_deref(),
                cred.client_private_key_pem.as_deref(),
            ) {
                (Some(cert), Some(key)) => {
                    require_non_empty("western_union.client_certificate_pem", cert)?;
                    require_non_empty("western_union.client_private_key_pem", key)?;
                    crate::providers::western_union::validate_client_identity_pem(cert, key)?;
                }
                (None, None) => {}
                _ => {
                    return Err(AppError::BadRequest(
                        "western_union requires both client_certificate_pem and client_private_key_pem"
                            .into(),
                    ));
                }
            }
            Ok(Some(cred.client_id))
        }
        ProviderKind::UsBankZelle => {
            let cred: crate::providers::zelle_disbursements::UsBankZelleCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid us_bank_zelle credential: {e}"))
                })?;
            require_non_empty("us_bank_zelle.access_token", &cred.access_token)?;
            require_non_empty("us_bank_zelle.client_id", &cred.client_id)?;
            require_non_empty("us_bank_zelle.program_id", &cred.program_id)?;
            validate_environment("us_bank_zelle.environment", &cred.environment)?;
            crate::providers::zelle_disbursements::validate_zelle_api_base_url(
                "us_bank_zelle.api_base_url",
                &cred.api_base_url,
            )?;
            crate::providers::zelle_disbursements::validate_zelle_path(
                "us_bank_zelle.payments_path",
                &cred.payments_path,
            )?;
            crate::providers::zelle_disbursements::validate_zelle_path(
                "us_bank_zelle.enrollment_path",
                &cred.enrollment_path,
            )?;
            Ok(Some(cred.program_id))
        }
        ProviderKind::JpmorganZelle => {
            let cred: crate::providers::zelle_disbursements::JpmorganZelleCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid jpmorgan_zelle credential: {e}"))
                })?;
            require_non_empty("jpmorgan_zelle.access_token", &cred.access_token)?;
            require_non_empty("jpmorgan_zelle.debtor_account_id", &cred.debtor_account_id)?;
            require_non_empty("jpmorgan_zelle.debtor_name", &cred.debtor_name)?;
            require_non_empty("jpmorgan_zelle.debtor_bic", &cred.debtor_bic)?;
            validate_environment("jpmorgan_zelle.environment", &cred.environment)?;
            if let Some(api_base_url) = cred.api_base_url.as_deref() {
                crate::providers::zelle_disbursements::validate_zelle_api_base_url(
                    "jpmorgan_zelle.api_base_url",
                    api_base_url,
                )?;
            }
            Ok(Some(cred.debtor_account_id))
        }
        ProviderKind::BofaCashProGdd => {
            let cred: crate::providers::zelle_disbursements::BofaCashProGddCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid bofa_cashpro_gdd credential: {e}"))
                })?;
            require_non_empty("bofa_cashpro_gdd.client_id", &cred.client_id)?;
            require_non_empty("bofa_cashpro_gdd.client_secret", &cred.client_secret)?;
            require_non_empty(
                "bofa_cashpro_gdd.cashpro_company_id",
                &cred.cashpro_company_id,
            )?;
            if let Some(access_token) = cred.access_token.as_deref() {
                require_non_empty("bofa_cashpro_gdd.access_token", access_token)?;
            }
            validate_environment("bofa_cashpro_gdd.environment", &cred.environment)?;
            crate::providers::zelle_disbursements::validate_zelle_api_base_url(
                "bofa_cashpro_gdd.api_base_url",
                &cred.api_base_url,
            )?;
            crate::providers::zelle_disbursements::validate_zelle_path(
                "bofa_cashpro_gdd.disbursements_path",
                &cred.disbursements_path,
            )?;
            Ok(Some(cred.cashpro_company_id))
        }
        ProviderKind::ModernTreasury => {
            let cred: ModernTreasuryCredential = serde_json::from_value(credential.clone())
                .map_err(|e| {
                    AppError::BadRequest(format!("invalid modern_treasury credential: {e}"))
                })?;
            require_non_empty("modern_treasury.organization_id", &cred.organization_id)?;
            require_non_empty("modern_treasury.api_key", &cred.api_key)?;
            validate_environment("modern_treasury.environment", &cred.environment)?;
            if let Some(api_base_url) = cred.api_base_url.as_deref() {
                crate::providers::modern_treasury::validate_modern_treasury_api_base_url(
                    api_base_url,
                )?;
            }
            if let Some(originating_account_id) = cred.default_originating_account_id.as_deref() {
                require_non_empty(
                    "modern_treasury.default_originating_account_id",
                    originating_account_id,
                )?;
            }
            if let Some(webhook_secret) = cred.webhook_secret.as_deref() {
                require_non_empty("modern_treasury.webhook_secret", webhook_secret)?;
            }
            Ok(Some(cred.organization_id))
        }
        ProviderKind::Dwolla => {
            let cred: DwollaCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid dwolla credential: {e}")))?;
            require_non_empty("dwolla.access_token", &cred.access_token)?;
            validate_environment("dwolla.environment", &cred.environment)?;
            if let Some(api_base_url) = cred.api_base_url.as_deref() {
                crate::providers::dwolla::validate_dwolla_api_base_url(api_base_url)?;
            }
            if let Some(account_id) = cred.account_id.as_deref() {
                require_non_empty("dwolla.account_id", account_id)?;
            }
            if let Some(webhook_secret) = cred.webhook_secret.as_deref() {
                require_non_empty("dwolla.webhook_secret", webhook_secret)?;
            }
            Ok(cred.account_id.clone())
        }
        ProviderKind::EthereumWallet => {
            let cred: EthereumWalletCredential = serde_json::from_value(credential.clone())
                .map_err(|e| {
                    AppError::BadRequest(format!("invalid ethereum_wallet credential: {e}"))
                })?;
            crate::providers::ethereum::validate_ethereum_address(&cred.address)?;
            crate::providers::ethereum::validate_ethereum_rpc_url(&cred.rpc_url)?;
            if cred.chain_id == 0 {
                return Err(AppError::BadRequest(
                    "ethereum_wallet.chain_id must be non-zero".into(),
                ));
            }
            if let Some(token) = cred.rpc_bearer_token.as_deref() {
                require_non_empty("ethereum_wallet.rpc_bearer_token", token)?;
            }
            for asset in &cred.tracked_assets {
                require_non_empty("ethereum_wallet.tracked_assets.symbol", &asset.symbol)?;
                if let Some(contract_address) = asset.contract_address.as_deref() {
                    crate::providers::ethereum::validate_ethereum_address(contract_address)?;
                }
            }
            Ok(Some(cred.address.to_ascii_lowercase()))
        }
        ProviderKind::Robinhood => {
            let _cred: crate::providers::robinhood::RobinhoodCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid robinhood credential: {e}"))
                })?;
            Ok(None)
        }
        ProviderKind::Fireblocks => {
            let cred: crate::providers::fireblocks::FireblocksCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid fireblocks credential: {e}"))
                })?;
            require_non_empty("fireblocks.api_key", &cred.api_key)?;
            require_non_empty("fireblocks.api_secret_pem", &cred.api_secret_pem)?;
            // Sanity-check that the PEM is a real RSA private key
            // before we seal it — catches paste errors at attach time
            // rather than at first signed-request time.
            jsonwebtoken::EncodingKey::from_rsa_pem(cred.api_secret_pem.as_bytes()).map_err(
                |e| {
                    AppError::BadRequest(format!(
                        "fireblocks.api_secret_pem is not a valid RSA PEM: {e}"
                    ))
                },
            )?;
            validate_environment("fireblocks.environment", &cred.environment)?;
            Ok(Some(cred.api_key))
        }
        ProviderKind::Circle => {
            let cred: crate::providers::circle::CircleCredential =
                serde_json::from_value(credential.clone())
                    .map_err(|e| AppError::BadRequest(format!("invalid circle credential: {e}")))?;
            require_non_empty("circle.api_key", &cred.api_key)?;
            validate_environment("circle.environment", &cred.environment)?;
            Ok(None)
        }
        ProviderKind::Adyen => {
            let cred: AdyenCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid adyen credential: {e}")))?;
            require_non_empty("adyen.api_key", &cred.api_key)?;
            require_non_empty("adyen.merchant_account", &cred.merchant_account)?;
            validate_environment("adyen.environment", &cred.environment)?;
            // Live traffic needs the per-merchant endpoint prefix; sandbox
            // uses the shared host so the base is optional there.
            if cred.is_production() {
                require_non_empty_opt("adyen.api_base_url", cred.api_base_url.as_deref())?;
            }
            if let Some(hmac_key_hex) = cred.hmac_key_hex.as_deref() {
                require_non_empty("adyen.hmac_key_hex", hmac_key_hex)?;
                if hex::decode(hmac_key_hex.trim()).is_err() {
                    return Err(AppError::BadRequest(
                        "adyen.hmac_key_hex must be hex-encoded".into(),
                    ));
                }
            }
            // The merchant account is the natural webhook-routing key.
            Ok(Some(cred.merchant_account))
        }
        ProviderKind::Square => {
            let cred: SquareCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid square credential: {e}")))?;
            require_non_empty("square.access_token", &cred.access_token)?;
            validate_environment("square.environment", &cred.environment)?;
            if let Some(merchant_id) = cred.merchant_id.as_deref() {
                require_non_empty("square.merchant_id", merchant_id)?;
            }
            // Square signs `url + body`; if a signature key is configured the
            // notification URL must be too, or verification can never succeed.
            if let Some(key) = cred.webhook_signature_key.as_deref() {
                require_non_empty("square.webhook_signature_key", key)?;
                require_non_empty_opt(
                    "square.webhook_notification_url",
                    cred.webhook_notification_url.as_deref(),
                )?;
            }
            Ok(cred.merchant_id.clone())
        }
        ProviderKind::Stripe
        | ProviderKind::Paypal
        | ProviderKind::Braintree
        | ProviderKind::PlaidBank
        | ProviderKind::SwiftWire
        | ProviderKind::AchDirect
        | ProviderKind::SolanaWallet => Ok(None),
    }
}

fn optional_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn require_non_empty(field: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(())
}

fn require_non_empty_opt(field: &str, value: Option<&str>) -> AppResult<()> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(()),
        _ => Err(AppError::BadRequest(format!("{field} is required"))),
    }
}

fn validate_environment(field: &str, value: &str) -> AppResult<()> {
    match value.to_ascii_lowercase().as_str() {
        "production" | "sandbox" => Ok(()),
        other => Err(AppError::BadRequest(format!(
            "{field} must be production or sandbox, got {other}"
        ))),
    }
}

fn validate_language_tag(field: &str, value: &str) -> AppResult<()> {
    let value = value.trim();
    if value.len() < 2
        || value.len() > 20
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(AppError::BadRequest(format!(
            "{field} must be a compact BCP-47-style language tag"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_coinflow_and_derives_merchant_id() {
        let credential = serde_json::json!({
            "api_key": "cf_test",
            "merchant_id": "merchant_123",
            "environment": "sandbox",
            "webhook_validation_key": "hook_secret"
        });

        let derived = validate_api_key_credential(ProviderKind::Coinflow, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("merchant_123"));
    }

    #[test]
    fn validates_wise_and_derives_profile_id() {
        let credential = serde_json::json!({
            "api_token": "wise_test",
            "profile_id": "profile_456",
            "environment": "production"
        });

        let derived = validate_api_key_credential(ProviderKind::Wise, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("profile_456"));
    }

    #[test]
    fn rejects_empty_coinbase_webhook_secret() {
        let credential = serde_json::json!({
            "api_key": "coinbase_test",
            "webhook_secret": "",
            "variant": "commerce"
        });

        let err =
            validate_api_key_credential(ProviderKind::CoinbaseCommerce, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_unknown_environment() {
        let credential = serde_json::json!({
            "api_token": "wise_test",
            "profile_id": "profile_456",
            "environment": "staging"
        });

        let err = validate_api_key_credential(ProviderKind::Wise, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn validates_moneygram_and_derives_partner_id() {
        let credential = serde_json::json!({
            "client_id": "mg_client",
            "client_secret": "mg_secret",
            "agent_partner_id": "agent_123",
            "environment": "sandbox"
        });

        let derived = validate_api_key_credential(ProviderKind::MoneyGram, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("agent_123"));
    }

    #[test]
    fn rejects_bad_moneygram_language_tag() {
        let credential = serde_json::json!({
            "client_id": "mg_client",
            "client_secret": "mg_secret",
            "agent_partner_id": "agent_123",
            "user_language": "en_US with spaces",
            "environment": "sandbox"
        });

        let err = validate_api_key_credential(ProviderKind::MoneyGram, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_partial_remitly_partner_api_config() {
        let key_only = serde_json::json!({
            "api_key": "remitly_key",
            "environment": "sandbox"
        });
        let url_only = serde_json::json!({
            "api_base_url": "https://partner.remitly.example",
            "environment": "sandbox"
        });

        assert!(matches!(
            validate_api_key_credential(ProviderKind::Remitly, &key_only),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            validate_api_key_credential(ProviderKind::Remitly, &url_only),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_remitly_partner_api_non_https_or_local_base_url() {
        for api_base_url in [
            "http://partner.remitly.example",
            "https://localhost:8080",
            "https://10.0.0.1",
            "https://billing-service",
        ] {
            let credential = serde_json::json!({
                "api_key": "remitly_key",
                "api_base_url": api_base_url,
                "environment": "sandbox"
            });

            let err = validate_api_key_credential(ProviderKind::Remitly, &credential).unwrap_err();

            assert!(matches!(err, AppError::BadRequest(_)));
        }
    }

    #[test]
    fn validates_western_union_and_derives_client_id() {
        let credential = serde_json::json!({
            "client_id": "wu_client",
            "environment": "production"
        });

        let derived = validate_api_key_credential(ProviderKind::WesternUnion, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("wu_client"));
    }

    #[test]
    fn rejects_incomplete_western_union_mtls_pair() {
        let credential = serde_json::json!({
            "client_id": "wu_client",
            "environment": "sandbox",
            "client_certificate_pem": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"
        });

        let err = validate_api_key_credential(ProviderKind::WesternUnion, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_invalid_western_union_mtls_pair() {
        let credential = serde_json::json!({
            "client_id": "wu_client",
            "environment": "sandbox",
            "client_certificate_pem": "not a cert",
            "client_private_key_pem": "not a key"
        });

        let err = validate_api_key_credential(ProviderKind::WesternUnion, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn validates_us_bank_zelle_and_derives_program_id() {
        let credential = serde_json::json!({
            "access_token": "usb_access",
            "client_id": "usb_client",
            "program_id": "program_123",
            "api_base_url": "https://api.usbank.example",
            "payments_path": "/payments",
            "enrollment_path": "/enrollments",
            "environment": "sandbox"
        });

        let derived = validate_api_key_credential(ProviderKind::UsBankZelle, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("program_123"));
    }

    #[test]
    fn rejects_zelle_http_or_local_runtime_base_url() {
        for api_base_url in ["http://api.usbank.example", "https://localhost:8443"] {
            let credential = serde_json::json!({
                "access_token": "usb_access",
                "client_id": "usb_client",
                "program_id": "program_123",
                "api_base_url": api_base_url,
                "environment": "sandbox"
            });

            let err =
                validate_api_key_credential(ProviderKind::UsBankZelle, &credential).unwrap_err();

            assert!(matches!(err, AppError::BadRequest(_)));
        }
    }

    #[test]
    fn validates_jpmorgan_zelle_and_derives_debtor_account() {
        let credential = serde_json::json!({
            "access_token": "jpm_access",
            "debtor_account_id": "acct_123",
            "debtor_name": "Example Corp",
            "debtor_bic": "CHASUS33",
            "api_base_url": "https://payments.jpmorgan.example/tsapi/v1",
            "environment": "production"
        });

        let derived =
            validate_api_key_credential(ProviderKind::JpmorganZelle, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("acct_123"));
    }

    #[test]
    fn validates_bofa_cashpro_gdd_and_derives_company_id() {
        let credential = serde_json::json!({
            "client_id": "bofa_client",
            "client_secret": "bofa_secret",
            "cashpro_company_id": "company_123",
            "access_token": "bofa_access",
            "api_base_url": "https://cashpro-api.bankofamerica.example",
            "disbursements_path": "/gdd/disbursements",
            "environment": "production"
        });

        let derived =
            validate_api_key_credential(ProviderKind::BofaCashProGdd, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("company_123"));
    }

    #[test]
    fn rejects_bofa_cashpro_gdd_non_root_relative_path() {
        let credential = serde_json::json!({
            "client_id": "bofa_client",
            "client_secret": "bofa_secret",
            "cashpro_company_id": "company_123",
            "api_base_url": "https://cashpro-api.bankofamerica.example",
            "disbursements_path": "https://evil.example/gdd",
            "environment": "production"
        });

        let err =
            validate_api_key_credential(ProviderKind::BofaCashProGdd, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn validates_modern_treasury_and_derives_org_id() {
        let credential = serde_json::json!({
            "organization_id": "org_123",
            "api_key": "mt_key",
            "default_originating_account_id": "ia_123",
            "api_base_url": "https://app.moderntreasury.example",
            "environment": "production"
        });

        let derived =
            validate_api_key_credential(ProviderKind::ModernTreasury, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("org_123"));
    }

    #[test]
    fn validates_dwolla_and_derives_account_id() {
        let credential = serde_json::json!({
            "access_token": "dwolla_access",
            "account_id": "acct_123",
            "api_base_url": "https://api.dwolla.example",
            "environment": "sandbox"
        });

        let derived = validate_api_key_credential(ProviderKind::Dwolla, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("acct_123"));
    }

    #[test]
    fn rejects_dwolla_local_base_url() {
        let credential = serde_json::json!({
            "access_token": "dwolla_access",
            "api_base_url": "https://127.0.0.1:8080",
            "environment": "sandbox"
        });

        let err = validate_api_key_credential(ProviderKind::Dwolla, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn validates_ethereum_wallet_and_derives_address() {
        let credential = serde_json::json!({
            "address": "0x1111111111111111111111111111111111111111",
            "rpc_url": "https://eth-mainnet.example",
            "chain_id": 1,
            "rpc_bearer_token": "rpc_token",
            "tracked_assets": [{
                "symbol": "USDC",
                "contract_address": "0x2222222222222222222222222222222222222222",
                "decimals": 6
            }]
        });

        let derived =
            validate_api_key_credential(ProviderKind::EthereumWallet, &credential).unwrap();

        assert_eq!(
            derived.as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn rejects_ethereum_wallet_local_rpc_url() {
        let credential = serde_json::json!({
            "address": "0x1111111111111111111111111111111111111111",
            "rpc_url": "https://192.168.1.10",
            "chain_id": 1
        });

        let err =
            validate_api_key_credential(ProviderKind::EthereumWallet, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }
}
