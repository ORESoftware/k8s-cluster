use std::sync::atomic::Ordering;

use dd_nats_subject_defs::PUBLIC_DATA_PIPELINE_JOBS_SUBJECT;
use serde_json::{json, Value};

use crate::catalog::*;
use crate::dashboard::*;
use crate::forecast::*;
use crate::recommendations::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn is_cluster_internal_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host.ends_with(".svc.cluster.local")
        || host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
}

pub(crate) fn validate_http_base_url(
    base: &str,
    allow_external: bool,
    label: &str,
) -> Result<reqwest::Url, String> {
    let trimmed = base.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "{label} must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
        ));
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| format!("{label} is invalid: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(format!("{label} must use http or https")),
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(format!("{label} must not contain URL credentials"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "{label} must not contain query strings or fragments"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("{label} must include a host"))?;
    if !is_cluster_internal_host(host) && !allow_external {
        return Err(format!(
            "{label} must be cluster-local unless ECONOMICS_ALLOW_EXTERNAL_PIPELINE_URLS=true"
        ));
    }
    Ok(parsed)
}

pub(crate) fn validate_plan_only_http_url(base: &str, label: &str) -> Result<reqwest::Url, String> {
    validate_http_base_url(base, true, label)
}

pub(crate) fn integration_dependency(
    id: &str,
    kind: &str,
    status: &str,
    configured: bool,
    required_for_core_readiness: bool,
    mode: &str,
    details: Value,
    warnings: Vec<String>,
) -> IntegrationDependencyStatus {
    IntegrationDependencyStatus {
        id: id.to_string(),
        kind: kind.to_string(),
        status: status.to_string(),
        configured,
        required_for_core_readiness,
        mode: mode.to_string(),
        details,
        warnings,
    }
}

pub(crate) fn sentiment_credential_count(credentials: &SentimentCredentialStatus) -> usize {
    [
        credentials.x_bearer_token,
        credentials.x_api_key,
        credentials.x_api_secret,
        credentials.x_access_token,
        credentials.x_access_token_secret,
        credentials.reddit_client_id,
        credentials.reddit_client_secret,
        credentials.reddit_user_agent,
        credentials.news_api_key,
        credentials.stocktwits_token,
        credentials.gdelt_api_key,
    ]
    .into_iter()
    .filter(|configured| *configured)
    .count()
}

pub(crate) fn market_data_credential_count(credentials: &MarketDataCredentialStatus) -> usize {
    [
        credentials.fred_api_key,
        credentials.bea_api_key,
        credentials.bls_api_key,
        credentials.treasury_api_key,
        credentials.census_api_key,
        credentials.eia_api_key,
        credentials.coingecko_api_key,
        credentials.sec_api_key,
        credentials.crunchbase_api_key,
        credentials.pitchbook_api_key,
        credentials.cb_insights_api_key,
        credentials.dealroom_api_key,
        credentials.preqin_api_key,
    ]
    .into_iter()
    .filter(|configured| *configured)
    .count()
}

pub(crate) fn integration_dependencies(state: &AppState) -> Vec<IntegrationDependencyStatus> {
    let auth_ready =
        state.config.allow_unauthenticated || state.config.server_auth_secret.is_some();
    let mut dependencies = vec![integration_dependency(
        "server-auth",
        "security",
        if auth_ready { "ready" } else { "not-ready" },
        state.config.server_auth_secret.is_some(),
        true,
        if state.config.allow_unauthenticated {
            "local-unauthenticated"
        } else {
            "shared-secret"
        },
        json!({
            "acceptedHeaders": ["x-server-auth", "auth", "authorization"],
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "secretConfigured": state.config.server_auth_secret.is_some()
        }),
        if state.config.allow_unauthenticated {
            vec![
                "ECONOMICS_ALLOW_UNAUTHENTICATED=true should stay limited to local development"
                    .to_string(),
            ]
        } else {
            Vec::new()
        },
    )];

    let source_warnings = [
        (
            state.config.allow_private_source_urls,
            "private/link-local source URLs are enabled",
        ),
        (
            state.config.allowed_source_hosts.is_empty(),
            "ad-hoc source host allowlist is empty",
        ),
    ]
    .into_iter()
    .filter_map(|(active, warning)| active.then(|| warning.to_string()))
    .collect::<Vec<_>>();
    dependencies.push(integration_dependency(
        "source-egress",
        "data-ingest",
        if source_warnings.is_empty() {
            "ready"
        } else {
            "degraded"
        },
        true,
        false,
        "bounded-http-pull",
        json!({
            "privateUrlsAllowed": state.config.allow_private_source_urls,
            "redirectFollowing": false,
            "allowedSourceHosts": state.config.allowed_source_hosts,
            "knownPublicHosts": public_source_hosts(),
            "maxSourceFetchBytes": MAX_SOURCE_FETCH_BYTES
        }),
        source_warnings,
    ));
    dependencies.push(integration_dependency(
        "source-auth-env-allowlist",
        "secret-boundary",
        if state.config.allowed_source_auth_envs.is_empty() {
            "degraded"
        } else {
            "ready"
        },
        !state.config.allowed_source_auth_envs.is_empty(),
        false,
        "explicit-env-allowlist",
        json!({
            "allowedEnvCount": state.config.allowed_source_auth_envs.len(),
            "allowlistEnv": "ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS",
            "valuesReturned": false
        }),
        Vec::new(),
    ));

    let spark_url_status = state.config.spark_pipeline_url.as_deref().map(|url| {
        validate_http_base_url(
            url,
            state.config.allow_external_pipeline_urls,
            "spark pipeline URL",
        )
    });
    let spark_valid = spark_url_status
        .as_ref()
        .map(Result::is_ok)
        .unwrap_or(false);
    let spark_auth_configured = optional_env(&state.config.spark_pipeline_auth_env).is_some();
    let spark_status = if state.config.allow_pipeline_submit {
        if spark_valid && spark_auth_configured {
            "ready"
        } else {
            "degraded"
        }
    } else if spark_url_status.as_ref().is_some_and(Result::is_err) {
        "misconfigured"
    } else {
        "disabled"
    };
    dependencies.push(integration_dependency(
        "spark-pipeline-server",
        "big-data",
        spark_status,
        state.config.spark_pipeline_url.is_some(),
        false,
        if state.config.allow_pipeline_submit {
            "submit-enabled"
        } else {
            "plan-only"
        },
        json!({
            "urlConfigured": state.config.spark_pipeline_url.is_some(),
            "urlValid": spark_valid,
            "authEnv": state.config.spark_pipeline_auth_env,
            "authConfigured": spark_auth_configured,
            "externalUrlsAllowed": state.config.allow_external_pipeline_urls
        }),
        spark_url_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    let airflow_status = state
        .config
        .airflow_api_url
        .as_deref()
        .map(|url| validate_plan_only_http_url(url, "Airflow API URL"));
    dependencies.push(integration_dependency(
        "airflow",
        "orchestrator",
        match airflow_status.as_ref() {
            Some(Ok(_)) => "plan-only",
            Some(Err(_)) => "misconfigured",
            None => "disabled",
        },
        state.config.airflow_api_url.is_some(),
        false,
        "plan-only",
        json!({
            "apiUrlConfigured": state.config.airflow_api_url.is_some(),
            "dagBlueprint": "economics_market_refresh",
            "liveSubmissionImplemented": false
        }),
        airflow_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    let databricks_status = state
        .config
        .databricks_host
        .as_deref()
        .map(|url| validate_plan_only_http_url(url, "Databricks host"));
    let databricks_token_configured = optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some();
    dependencies.push(integration_dependency(
        "databricks",
        "managed-big-data",
        match databricks_status.as_ref() {
            Some(Ok(_)) if databricks_token_configured => "plan-only",
            Some(Ok(_)) => "degraded",
            Some(Err(_)) => "misconfigured",
            None => "disabled",
        },
        state.config.databricks_host.is_some() || databricks_token_configured,
        false,
        "plan-only",
        json!({
            "hostConfigured": state.config.databricks_host.is_some(),
            "tokenConfigured": databricks_token_configured,
            "credentialValuesReturned": false,
            "liveSubmissionImplemented": false
        }),
        databricks_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_else(|| {
                if state.config.databricks_host.is_some() && !databricks_token_configured {
                    vec!["ECONOMICS_DATABRICKS_TOKEN is not configured".to_string()]
                } else {
                    Vec::new()
                }
            }),
    ));

    let data_lake_valid = validate_data_lake_uri(&state.config.data_lake_uri);
    dependencies.push(integration_dependency(
        "data-lake",
        "storage",
        if data_lake_valid.is_ok() {
            "ready"
        } else {
            "misconfigured"
        },
        true,
        false,
        "pipeline-target",
        json!({
            "uriSchemeAllowed": data_lake_valid.is_ok(),
            "allowedSchemes": ["s3", "s3a", "abfss", "gs", "file:///tmp/"]
        }),
        data_lake_valid
            .err()
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    dependencies.push(integration_dependency(
        "nats",
        "messaging",
        if state.nats.is_some() {
            "ready"
        } else {
            "disabled"
        },
        state.nats.is_some(),
        false,
        "forecast-and-pipeline-events",
        json!({
            "forecastRequestSubject": state.config.request_subject,
            "forecastResultSubject": state.config.result_subject,
            "marketEventSubject": state.config.market_event_subject,
            "runtimeEventSubject": state.config.runtime_event_subject,
            "pipelineIntentSubject": state.config.pipeline_intent_subject
        }),
        if state.nats.is_some() {
            Vec::new()
        } else {
            vec!["NATS_URL is not configured or connection was not established".to_string()]
        },
    ));

    let sentiment_count = sentiment_credential_count(&state.config.sentiment_credentials);
    dependencies.push(integration_dependency(
        "sentiment-providers",
        "social-news",
        if sentiment_count > 0 {
            "ready"
        } else {
            "placeholder"
        },
        sentiment_count > 0,
        false,
        "document-analysis-now-live-fetchers-later",
        json!({
            "configuredCredentialCount": sentiment_count,
            "providerCatalogRoute": "GET /sentiment/sources",
            "analyzeRoute": "POST /sentiment/analyze"
        }),
        if sentiment_count > 0 {
            Vec::new()
        } else {
            vec!["live sentiment provider fetchers are placeholders; POST supplied documents for scoring".to_string()]
        },
    ));

    let market_count = market_data_credential_count(&state.config.market_data_credentials);
    dependencies.push(integration_dependency(
        "market-data-providers",
        "market-macro-private-data",
        if market_count > 0 {
            "ready"
        } else {
            "public-only"
        },
        market_count > 0,
        false,
        "source-templates-and-private-credentials",
        json!({
            "configuredCredentialCount": market_count,
            "publicSourceTemplateCount": public_source_templates().len(),
            "publicSourcesRoute": "GET /sources/public"
        }),
        Vec::new(),
    ));

    dependencies.push(integration_dependency(
        "runtime-config",
        "control-plane",
        if optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some() {
            "ready"
        } else {
            "disabled"
        },
        optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some(),
        false,
        "register-and-receive-updates",
        json!({
            "registerUrlConfigured": optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some(),
            "applyRouteConfigured": optional_env("RUNTIME_CONFIG_APPLY_URL").is_some(),
            "scope": env_value("RUNTIME_CONFIG_SCOPE", "default"),
            "env": env_value("RUNTIME_CONFIG_ENV", "stage")
        }),
        Vec::new(),
    ));

    dependencies.push(integration_dependency(
        "des-engine",
        "math-engine",
        "ready",
        true,
        true,
        "embedded-sdk-surface",
        des_surface_descriptor(),
        Vec::new(),
    ));

    dependencies
}

pub(crate) fn integration_health_payload(state: &AppState) -> Value {
    let dependencies = integration_dependencies(state);
    let required_ready = dependencies
        .iter()
        .filter(|dependency| dependency.required_for_core_readiness)
        .all(|dependency| dependency.status == "ready");
    let degraded_count = dependencies
        .iter()
        .filter(|dependency| {
            matches!(
                dependency.status.as_str(),
                "degraded" | "misconfigured" | "not-ready"
            )
        })
        .count();
    let overall_status = if required_ready && degraded_count == 0 {
        "ready"
    } else if required_ready {
        "degraded"
    } else {
        "not-ready"
    };
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "overallStatus": overall_status,
        "coreReady": required_ready,
        "dependencyCount": dependencies.len(),
        "degradedDependencyCount": degraded_count,
        "dependencies": dependencies,
        "telemetry": {
            "metricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "integrationHealthRequestsMetric": "dd_economics_server_integration_health_requests_total",
            "structuredLogSchema": "dd.log.v1"
        },
        "atMs": now_ms()
    })
}

pub(crate) fn pipeline_integration_status(state: &AppState) -> PipelineIntegrationStatus {
    PipelineIntegrationStatus {
        spark_pipeline_url_configured: state.config.spark_pipeline_url.is_some(),
        spark_pipeline_auth_configured: optional_env(&state.config.spark_pipeline_auth_env)
            .is_some(),
        spark_pipeline_submit_enabled: state.config.allow_pipeline_submit,
        spark_pipeline_url: state.config.spark_pipeline_url.clone(),
        spark_pipeline_auth_env: state.config.spark_pipeline_auth_env.clone(),
        spark_master_url: state.config.spark_master_url.clone(),
        airflow_api_url_configured: state.config.airflow_api_url.is_some(),
        airflow_api_url: state.config.airflow_api_url.clone(),
        databricks_host_configured: state.config.databricks_host.is_some(),
        databricks_token_configured: optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some(),
        data_lake_uri: state.config.data_lake_uri.clone(),
        pipeline_intent_subject: state.config.pipeline_intent_subject.clone(),
        nats_configured: state.nats.is_some(),
    }
}

pub(crate) fn pipeline_catalog_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "status": pipeline_integration_status(state),
        "engines": [
            {
                "id": "spark-pipeline-server",
                "kind": "internal-http",
                "route": "POST /v1/jobs",
                "supportedJobKinds": ["INGEST_VALIDATE_PUBLISH", "SPARK_SUBMIT"],
                "defaultUrl": DEFAULT_SPARK_PIPELINE_URL,
                "submitRoute": "POST /pipelines/submit",
                "submitGate": "ECONOMICS_ENABLE_PIPELINE_SUBMIT must be true and SERVER_AUTH_SECRET must be available"
            },
            {
                "id": "spark-standalone",
                "kind": "spark",
                "master": state.config.spark_master_url,
                "namespace": "big-data",
                "notes": "Development Spark master/worker stack from remote/argocd/big-data."
            },
            {
                "id": "airflow",
                "kind": "orchestrator",
                "apiUrl": state.config.airflow_api_url,
                "dagBlueprint": "economics_market_refresh",
                "notes": "Plan output includes a DAG trigger payload; live Airflow submission is intentionally not implemented until service credentials and API auth are designed."
            },
            {
                "id": "databricks",
                "kind": "managed-external",
                "hostConfigured": state.config.databricks_host.is_some(),
                "tokenConfigured": optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some(),
                "credentialEnv": ["ECONOMICS_DATABRICKS_HOST", "ECONOMICS_DATABRICKS_TOKEN"],
                "notes": "Plan output includes Databricks Jobs API run-now payloads without exposing token values."
            },
            {
                "id": "nats-public-data-pipeline",
                "kind": "nats",
                "subject": state.config.pipeline_intent_subject,
                "defaultSubject": PUBLIC_DATA_PIPELINE_JOBS_SUBJECT,
                "notes": "Pipeline plans can be published as redacted job intents for downstream big-data workers."
            }
        ],
        "integrationHealthRoute": "GET /integrations/health",
        "planRoute": "POST /pipelines/plan",
        "auditRoute": "GET /audit/hardening"
    })
}

pub(crate) fn hardening_audit_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "auth": {
            "required": !state.config.allow_unauthenticated,
            "acceptedHeaders": ["x-server-auth", "auth", "authorization"],
            "constantTimeComparison": true,
            "allowUnauthenticated": state.config.allow_unauthenticated
        },
        "requestLimits": {
            "maxHttpBodyBytes": MAX_HTTP_BODY_BYTES,
            "maxNatsPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
            "maxSeries": MAX_SERIES,
            "maxObservationsPerSeries": MAX_OBSERVATIONS_PER_SERIES,
            "maxSentimentDocuments": MAX_SENTIMENT_DOCUMENTS,
            "maxSentimentTextBytes": MAX_SENTIMENT_TEXT_BYTES,
            "maxSentimentContextScores": MAX_SENTIMENT_CONTEXT_SCORES,
            "maxVentureCapitalDeals": MAX_VC_DEALS,
            "maxVentureSectorFlows": MAX_VC_SECTOR_FLOWS,
            "maxPipelineJobIntents": MAX_PIPELINE_JOB_INTENTS
        },
        "egressPolicy": {
            "sourcePullPrivateUrlsAllowed": state.config.allow_private_source_urls,
            "sourcePullAllowedHosts": state.config.allowed_source_hosts,
            "knownPublicSourceHosts": public_source_hosts(),
            "knownPublicSourceTemplates": public_source_templates().len(),
            "sourcePullRedirectFollowing": false,
            "externalPipelineUrlsAllowed": state.config.allow_external_pipeline_urls,
            "sparkPipelineSubmitEnabled": state.config.allow_pipeline_submit,
            "sparkPipelineSubmitRequiresInternalUrl": !state.config.allow_external_pipeline_urls
        },
        "secretHandling": {
            "credentialValuesReturned": false,
            "credentialStatusOnly": true,
            "sourceAuthHeaderEnvAllowlistEnabled": true,
            "sourceAuthHeaderEnvAllowlistCount": state.config.allowed_source_auth_envs.len(),
            "sourceAuthHeaderEnvAllowlistVar": "ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS",
            "sparkPipelineAuthEnv": state.config.spark_pipeline_auth_env,
            "databricksTokenEnv": "ECONOMICS_DATABRICKS_TOKEN"
        },
        "observability": {
            "prometheusMetricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "integrationHealthRoute": "GET /integrations/health",
            "structuredLogSchema": "dd.log.v1",
            "lokiCollectionBoundary": "container stdout/stderr via Promtail",
            "otelMode": "explicit-only",
            "autoInstrumentation": false,
            "runtimeMonkeyPatching": false
        },
        "bigData": pipeline_integration_status(state),
        "integrationHealth": integration_health_payload(state),
        "deploymentPosture": {
            "expectedNoServiceAccountToken": true,
            "expectedReadOnlyRootFilesystem": true,
            "expectedDroppedCapabilities": true,
            "expectedRuntimeDefaultSeccomp": true,
            "expectedBoundedWritableVolumes": true
        },
        "residualRisks": [
            "live provider connectors are placeholders until per-provider rate limits, retries, and backoff are implemented",
            "recommendation rankings are research signals, not trade execution instructions",
            "Airflow and Databricks submission remain plan-only until their auth and audit flows are explicitly designed",
            "GET /integrations/health reports integration readiness but does not perform active network probes against external providers",
            "Spark pipeline HTTP submission is disabled unless ECONOMICS_ENABLE_PIPELINE_SUBMIT=true"
        ],
        "atMs": now_ms()
    })
}

pub(crate) fn pipeline_plan_from_request(
    state: &AppState,
    mut request: PipelinePlanRequest,
) -> Result<PipelinePlanResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let request_id = request_id(request.request_id.as_ref(), "economics-pipeline-plan");
    let scenario = request
        .scenario
        .take()
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    clean_token(&scenario, "scenario")?;
    let data_lake_uri = request
        .data_lake_uri
        .take()
        .unwrap_or_else(|| state.config.data_lake_uri.clone());
    validate_data_lake_uri(&data_lake_uri)?;
    let job_kinds = normalize_pipeline_job_kinds(request.job_kinds.as_ref())?;
    let include_recommendations = request.include_recommendations.unwrap_or(true);
    let recommendation_summary = if include_recommendations {
        let mut recommendation_request =
            request
                .recommendation_request
                .take()
                .unwrap_or_else(|| RecommendationRequest {
                    request_id: Some(format!("{request_id}-recommendations")),
                    schema_version: Some(SCHEMA_VERSION.to_string()),
                    horizon_months: Some(state.config.projection_months),
                    company_limit: Some(20),
                    commodity_limit: Some(30),
                    scenario: Some(scenario.clone()),
                    series: Some(snapshot_series_or_sample(state)),
                    macro_context: None,
                    macro_fiscal_context: Some(default_macro_fiscal_context()),
                    venture_capital_context: Some(sample_venture_capital_context()),
                    sentiment_context: None,
                });
        if recommendation_request
            .series
            .as_ref()
            .map(Vec::is_empty)
            .unwrap_or(true)
        {
            recommendation_request.series = Some(snapshot_series_or_sample(state));
        }
        let recommendations = generate_recommendations(&state.config, recommendation_request)?;
        json!({
            "requestId": recommendations.request_id,
            "companyBuyCount": recommendations.company_buys.len(),
            "companyDumpCount": recommendations.company_dumps.len(),
            "commodityBuyCount": recommendations.commodity_buys.len(),
            "commoditySellOrDumpCount": recommendations.commodity_sells_or_dumps.len(),
            "topCompanyBuys": recommendations.company_buys.iter().take(5).map(|item| json!({
                "ticker": item.ticker,
                "company": item.company,
                "score": item.score,
                "expectedReturn18m": item.expected_return_18m
            })).collect::<Vec<_>>(),
            "topCommodityBuys": recommendations.commodity_buys.iter().take(5).map(|item| json!({
                "instrumentId": item.instrument_id,
                "commodity": item.commodity,
                "score": item.score,
                "expectedReturn18m": item.expected_return_18m
            })).collect::<Vec<_>>()
        })
    } else {
        json!({ "included": false })
    };

    let mut job_intents = Vec::new();
    if job_kinds.iter().any(|kind| kind == "ingest") {
        job_intents.push(spark_ingest_intent(&request_id, &data_lake_uri));
    }
    if job_kinds.iter().any(|kind| kind == "spark-features") {
        job_intents.push(spark_feature_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            &state.config.spark_master_url,
        ));
    }
    if job_kinds.iter().any(|kind| kind == "airflow") {
        job_intents.push(airflow_dag_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            state.config.airflow_api_url.as_deref(),
        ));
    }
    if job_kinds.iter().any(|kind| kind == "databricks") {
        job_intents.push(databricks_job_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            state.config.databricks_host.as_deref(),
        ));
    }
    if job_kinds.iter().any(|kind| kind == "nats") {
        job_intents.push(nats_pipeline_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            &state.config.pipeline_intent_subject,
        ));
    }
    if job_intents.len() > MAX_PIPELINE_JOB_INTENTS {
        return Err(format!(
            "pipeline plan produced more than {MAX_PIPELINE_JOB_INTENTS} job intents"
        ));
    }

    let mut warnings = vec![
        "pipeline plans are redacted job intents; secret values are never returned".to_string(),
        "Airflow and Databricks are plan-only until their auth flows are explicitly enabled"
            .to_string(),
    ];
    if !state.config.allow_pipeline_submit {
        warnings.push(
            "Spark pipeline submission is disabled by ECONOMICS_ENABLE_PIPELINE_SUBMIT=false"
                .to_string(),
        );
    }

    Ok(PipelinePlanResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        pipeline_status: pipeline_integration_status(state),
        recommendation_summary,
        job_intents,
        warnings,
    })
}

pub(crate) fn normalize_pipeline_job_kinds(input: Option<&Vec<String>>) -> Result<Vec<String>, String> {
    let values = input.cloned().unwrap_or_else(|| {
        vec![
            "ingest".to_string(),
            "spark-features".to_string(),
            "airflow".to_string(),
            "databricks".to_string(),
            "nats".to_string(),
        ]
    });
    if values.is_empty() {
        return Err("jobKinds must contain at least one item".to_string());
    }
    if values.len() > MAX_PIPELINE_JOB_INTENTS {
        return Err(format!(
            "jobKinds must contain at most {MAX_PIPELINE_JOB_INTENTS} items"
        ));
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let clean = clean_token(&value, "jobKinds[]")?.to_ascii_lowercase();
        match clean.as_str() {
            "ingest" | "spark-features" | "airflow" | "databricks" | "nats" => {
                if !normalized.iter().any(|existing| existing == &clean) {
                    normalized.push(clean);
                }
            }
            _ => {
                return Err(format!(
                    "jobKinds[] value `{clean}` is not supported; use ingest, spark-features, airflow, databricks, or nats"
                ));
            }
        }
    }
    Ok(normalized)
}

pub(crate) fn validate_data_lake_uri(uri: &str) -> Result<(), String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "dataLakeUri must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("s3://")
        || lower.starts_with("s3a://")
        || lower.starts_with("abfss://")
        || lower.starts_with("gs://")
        || lower.starts_with("file:///tmp/"))
    {
        return Err("dataLakeUri must use s3, s3a, abfss, gs, or file:///tmp/".to_string());
    }
    Ok(())
}

pub(crate) fn spark_ingest_intent(request_id: &str, data_lake_uri: &str) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-ingest-validate-publish"),
        engine: "spark-pipeline-server".to_string(),
        target: "dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        kind: "INGEST_VALIDATE_PUBLISH".to_string(),
        endpoint: Some("/v1/jobs".to_string()),
        auth_required: true,
        submit_eligible: true,
        params: json!({
            "source": SERVICE_NAME,
            "dataset": "economics-market-history",
            "schemaVersion": SCHEMA_VERSION,
            "requestId": request_id,
            "dataLakeUri": data_lake_uri,
            "publicSourceIds": public_source_ids(),
            "inputRoutes": ["POST /sources/pull", "POST /ingest"],
            "qualityChecks": [
                "schema-check",
                "finite-price-volume-check",
                "duplicate-date-check",
                "asset-class-partition-check"
            ],
            "outputs": [
                format!("{data_lake_uri}/bronze/market_series"),
                format!("{data_lake_uri}/manifests/economics-market-history.json")
            ]
        }),
        notes: vec![
            "compatible with dd-spark-pipeline-server JobKind.INGEST_VALIDATE_PUBLISH".to_string(),
        ],
    }
}

pub(crate) fn spark_feature_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    spark_master_url: &str,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-spark-feature-build"),
        engine: "spark-pipeline-server".to_string(),
        target: "dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        kind: "SPARK_SUBMIT".to_string(),
        endpoint: Some("/v1/jobs".to_string()),
        auth_required: true,
        submit_eligible: true,
        params: json!({
            "source": SERVICE_NAME,
            "appName": "economics-feature-build",
            "requestId": request_id,
            "master": spark_master_url,
            "mainClass": "com.oresoftware.dd.economics.FeatureBuildJob",
            "appResource": format!("{data_lake_uri}/jobs/economics-feature-build.jar"),
            "args": [
                "--scenario", scenario,
                "--public-source-ids", public_source_ids().join(","),
                "--input", format!("{data_lake_uri}/bronze/market_series"),
                "--output", format!("{data_lake_uri}/silver/features"),
                "--recommendations", format!("{data_lake_uri}/gold/recommendations")
            ],
            "conf": {
                "spark.sql.shuffle.partitions": "96",
                "spark.sql.adaptive.enabled": "true",
                "spark.serializer": "org.apache.spark.serializer.KryoSerializer"
            }
        }),
        notes: vec![
            "placeholder Spark application contract; appResource is a data-lake artifact path, not bundled in this Rust service".to_string(),
        ],
    }
}

pub(crate) fn airflow_dag_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    airflow_api_url: Option<&str>,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-airflow-refresh"),
        engine: "airflow".to_string(),
        target: airflow_api_url
            .unwrap_or(DEFAULT_AIRFLOW_API_URL)
            .to_string(),
        kind: "TRIGGER_DAG".to_string(),
        endpoint: Some("/api/v1/dags/economics_market_refresh/dagRuns".to_string()),
        auth_required: true,
        submit_eligible: false,
        params: json!({
            "dagRunId": format!("{request_id}-economics-market-refresh"),
            "conf": {
                "source": SERVICE_NAME,
                "schemaVersion": SCHEMA_VERSION,
                "scenario": scenario,
                "dataLakeUri": data_lake_uri,
                "publicSourceIds": public_source_ids(),
                "sparkPipelineJobKinds": ["INGEST_VALIDATE_PUBLISH", "SPARK_SUBMIT"]
            }
        }),
        notes: vec![
            "Airflow submission is plan-only until a service-account auth path is configured"
                .to_string(),
        ],
    }
}

pub(crate) fn databricks_job_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    databricks_host: Option<&str>,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-databricks-run-now"),
        engine: "databricks".to_string(),
        target: databricks_host
            .unwrap_or("databricks-managed-workspace")
            .to_string(),
        kind: "DATABRICKS_RUN_NOW".to_string(),
        endpoint: Some("/api/2.1/jobs/run-now".to_string()),
        auth_required: true,
        submit_eligible: false,
        params: json!({
            "idempotencyToken": request_id,
            "jobName": "economics-feature-and-recommendation-refresh",
            "notebookParams": {
                "source": SERVICE_NAME,
                "schemaVersion": SCHEMA_VERSION,
                "scenario": scenario,
                "dataLakeUri": data_lake_uri,
                "publicSourceIds": public_source_ids()
            },
            "credentialEnv": ["ECONOMICS_DATABRICKS_HOST", "ECONOMICS_DATABRICKS_TOKEN"]
        }),
        notes: vec![
            "Databricks token status is exposed only as a boolean; token values are never returned"
                .to_string(),
        ],
    }
}

pub(crate) fn nats_pipeline_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    subject: &str,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-nats-public-data-pipeline"),
        engine: "nats".to_string(),
        target: subject.to_string(),
        kind: "PUBLIC_DATA_PIPELINE_INTENT".to_string(),
        endpoint: None,
        auth_required: false,
        submit_eligible: false,
        params: json!({
            "messageKind": "economics.pipeline.intent",
            "source": SERVICE_NAME,
            "requestId": request_id,
            "schemaVersion": SCHEMA_VERSION,
            "scenario": scenario,
            "dataLakeUri": data_lake_uri,
            "publicSourceIds": public_source_ids(),
            "createdAtMs": now_ms()
        }),
        notes: vec![
            "published to dd.remote.public_data.pipeline.jobs or ECONOMICS_PIPELINE_INTENT_SUBJECT when NATS is configured".to_string(),
        ],
    }
}

pub(crate) async fn publish_pipeline_plan(state: &AppState, plan: &PipelinePlanResponse) {
    state
        .metrics
        .pipeline_publish_attempts_total
        .fetch_add(1, Ordering::Relaxed);
    let Some(nats) = state.nats.as_ref() else {
        state
            .metrics
            .pipeline_publish_failure_total
            .fetch_add(1, Ordering::Relaxed);
        emit_log(
            "WARN",
            "economics.pipeline.plan.publish.skipped",
            "pipeline plan publish requested but NATS is not configured",
            json!({
                "requestId": &plan.request_id,
                "subject": state.config.pipeline_intent_subject,
                "natsConfigured": false
            }),
        );
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "economics.pipeline.plan",
        "source": SERVICE_NAME,
        "plan": plan
    })) {
        Ok(payload) => payload,
        Err(error) => {
            state
                .metrics
                .pipeline_publish_failure_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            emit_log(
                "ERROR",
                "economics.pipeline.plan.encode.error",
                "failed to encode economics pipeline plan",
                json!({
                    "error": error_summary(&error.to_string()),
                    "requestId": &plan.request_id
                }),
            );
            return;
        }
    };
    match nats
        .publish(state.config.pipeline_intent_subject.clone(), payload.into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .pipeline_publish_success_total
                .fetch_add(1, Ordering::Relaxed);
            emit_log(
                "INFO",
                "economics.pipeline.plan.publish.ok",
                "pipeline plan published to NATS",
                json!({
                    "requestId": &plan.request_id,
                    "subject": state.config.pipeline_intent_subject,
                    "jobIntentCount": plan.job_intents.len()
                }),
            );
        }
        Err(error) => {
            state
                .metrics
                .pipeline_publish_failure_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            emit_log(
                "ERROR",
                "economics.pipeline.plan.publish.error",
                "failed to publish pipeline plan to NATS",
                json!({
                    "requestId": &plan.request_id,
                    "subject": state.config.pipeline_intent_subject,
                    "error": error_summary(&error.to_string())
                }),
            );
        }
    }
}

pub(crate) fn validate_pipeline_submit_url(config: &Config) -> Result<String, String> {
    let Some(base) = config.spark_pipeline_url.as_deref() else {
        return Err("ECONOMICS_SPARK_PIPELINE_URL is not configured".to_string());
    };
    validate_http_base_url(
        base,
        config.allow_external_pipeline_urls,
        "spark pipeline URL",
    )?;
    Ok(format!("{}/v1/jobs", base.trim_end_matches('/')))
}

pub(crate) async fn submit_pipeline_plan(
    state: &AppState,
    plan: &PipelinePlanResponse,
) -> Result<Vec<PipelineSubmittedJob>, String> {
    if !state.config.allow_pipeline_submit {
        return Err(
            "pipeline submission is disabled; set ECONOMICS_ENABLE_PIPELINE_SUBMIT=true"
                .to_string(),
        );
    }
    let submit_url = validate_pipeline_submit_url(&state.config)?;
    let auth_value = optional_env(&state.config.spark_pipeline_auth_env).ok_or_else(|| {
        format!(
            "spark pipeline auth env {} is not configured",
            state.config.spark_pipeline_auth_env
        )
    })?;
    let mut submitted = Vec::new();
    for intent in plan
        .job_intents
        .iter()
        .filter(|intent| intent.engine == "spark-pipeline-server" && intent.submit_eligible)
    {
        let payload = json!({
            "kind": intent.kind,
            "params": intent.params
        });
        let response = state
            .http
            .post(&submit_url)
            .header("x-server-auth", &auth_value)
            .json(&payload)
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let accepted = (200..300).contains(&status);
                let body = response.json::<Value>().await.ok();
                if accepted {
                    state
                        .metrics
                        .pipeline_submit_success_total
                        .fetch_add(1, Ordering::Relaxed);
                    emit_log(
                        "INFO",
                        "economics.pipeline.submit.ok",
                        "pipeline job submitted to Spark pipeline server",
                        json!({
                            "requestId": &plan.request_id,
                            "intentId": &intent.id,
                            "kind": &intent.kind,
                            "httpStatus": status
                        }),
                    );
                } else {
                    state
                        .metrics
                        .pipeline_submit_failure_total
                        .fetch_add(1, Ordering::Relaxed);
                    emit_log(
                        "WARN",
                        "economics.pipeline.submit.rejected",
                        "Spark pipeline server rejected a submitted economics job",
                        json!({
                            "requestId": &plan.request_id,
                            "intentId": &intent.id,
                            "kind": &intent.kind,
                            "httpStatus": status
                        }),
                    );
                }
                submitted.push(PipelineSubmittedJob {
                    intent_id: intent.id.clone(),
                    target: submit_url.clone(),
                    http_status: Some(status),
                    accepted,
                    response: body,
                    error: None,
                });
            }
            Err(error) => {
                state
                    .metrics
                    .pipeline_submit_failure_total
                    .fetch_add(1, Ordering::Relaxed);
                emit_log(
                    "ERROR",
                    "economics.pipeline.submit.error",
                    "failed to submit economics job to Spark pipeline server",
                    json!({
                        "requestId": &plan.request_id,
                        "intentId": &intent.id,
                        "kind": &intent.kind,
                        "error": error_summary(&error.to_string())
                    }),
                );
                submitted.push(PipelineSubmittedJob {
                    intent_id: intent.id.clone(),
                    target: submit_url.clone(),
                    http_status: None,
                    accepted: false,
                    response: None,
                    error: Some(error_summary(&error.to_string())),
                });
            }
        }
    }
    Ok(submitted)
}
