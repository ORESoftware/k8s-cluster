use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::extract::State;
use dd_nats_subject_defs::{PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, RUNTIME_EVENTS_SUBJECT};
use serde_json::json;

use crate::catalog::*;
use crate::dashboard::*;
use crate::forecast::*;
use crate::metrics::*;
use crate::pipeline::*;
use crate::recommendations::*;
use crate::sentiment::*;
use crate::shared::*;
use crate::sources::*;
use crate::state::*;
use crate::types::*;

    fn test_config() -> Config {
        Config {
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            allow_private_source_urls: false,
            allowed_source_hosts: Vec::new(),
            allowed_source_auth_envs: default_source_auth_envs(),
            sentiment_credentials: SentimentCredentialStatus {
                x_bearer_token: true,
                x_api_key: false,
                x_api_secret: false,
                x_access_token: false,
                x_access_token_secret: false,
                reddit_client_id: true,
                reddit_client_secret: true,
                reddit_user_agent: true,
                news_api_key: false,
                stocktwits_token: false,
                gdelt_api_key: false,
            },
            market_data_credentials: MarketDataCredentialStatus {
                fred_api_key: true,
                bea_api_key: true,
                bls_api_key: true,
                treasury_api_key: false,
                census_api_key: false,
                eia_api_key: false,
                coingecko_api_key: true,
                sec_api_key: false,
                crunchbase_api_key: true,
                pitchbook_api_key: false,
                cb_insights_api_key: false,
                dealroom_api_key: false,
                preqin_api_key: false,
            },
            history_years: DEFAULT_HISTORY_YEARS,
            projection_months: DEFAULT_PROJECTION_MONTHS,
            confidence_level: 0.90,
            request_subject: ECONOMICS_FORECAST_REQUEST_SUBJECT.to_string(),
            queue_group: ECONOMICS_QUEUE_GROUP.to_string(),
            result_subject: ECONOMICS_FORECAST_RESULT_SUBJECT.to_string(),
            market_event_subject: ECONOMICS_MARKET_EVENT_SUBJECT.to_string(),
            runtime_event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
            pipeline_intent_subject: PUBLIC_DATA_PIPELINE_JOBS_SUBJECT.to_string(),
            spark_pipeline_url: Some(DEFAULT_SPARK_PIPELINE_URL.to_string()),
            spark_pipeline_auth_env: "SERVER_AUTH_SECRET".to_string(),
            spark_master_url: DEFAULT_SPARK_MASTER_URL.to_string(),
            airflow_api_url: Some(DEFAULT_AIRFLOW_API_URL.to_string()),
            databricks_host: Some("https://example.cloud.databricks.com".to_string()),
            data_lake_uri: DEFAULT_DATA_LAKE_URI.to_string(),
            allow_pipeline_submit: false,
            allow_external_pipeline_urls: false,
        }
    }

    fn test_state() -> AppState {
        AppState {
            config: Arc::new(test_config()),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("dd-economics-server/0.1 test-source-pull")
                .build()
                .unwrap(),
            series_store: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn source_id_pull_request(source_id: &str) -> ApiPullRequest {
        ApiPullRequest {
            request_id: None,
            source_id: Some(source_id.to_string()),
            url: None,
            parser: None,
            instrument_id: None,
            display_name: None,
            asset_class: None,
            currency: None,
            source: None,
            root_pointer: None,
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        }
    }

    #[test]
    fn forecast_uses_equation_catalog_and_projects_requested_horizon() {
        let request = example_request();
        let response = generate_forecast(&test_config(), request).expect("forecast succeeds");

        assert_eq!(response.schema_version, SCHEMA_VERSION);
        assert_eq!(response.horizon_months, DEFAULT_PROJECTION_MONTHS);
        assert!(response.equations.len() >= 10);
        assert!(response.projections.len() >= 5);
        assert!(response
            .projections
            .iter()
            .all(|projection| projection.points.len() == DEFAULT_PROJECTION_MONTHS as usize));
    }

    #[test]
    fn liquidity_crunch_penalizes_crypto_more_than_bonds() {
        let mut request = example_request();
        request.scenario = Some("liquidity-crunch".to_string());
        let response = generate_forecast(&test_config(), request).expect("forecast succeeds");
        let crypto = response
            .projections
            .iter()
            .find(|projection| projection.asset_class == "crypto")
            .expect("crypto projection");
        let bond = response
            .projections
            .iter()
            .find(|projection| projection.asset_class == "bonds")
            .expect("bond projection");

        assert!(crypto.annualized_drift < bond.annualized_drift);
    }

    #[test]
    fn invalid_series_prices_are_rejected() {
        let mut request = example_request();
        let series = request.series.as_mut().unwrap();
        series[0].observations[0].price = 0.0;

        let error = generate_forecast(&test_config(), request).expect_err("invalid price rejected");
        assert!(error.contains("price must be finite and positive"));
    }

    #[test]
    fn source_url_policy_blocks_private_hosts_by_default() {
        let url = reqwest::Url::parse("http://127.0.0.1:9000/data.json").unwrap();
        let error = validate_source_url(&url, false).expect_err("private http blocked");

        assert!(error.contains("ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS"));
    }

    #[test]
    fn parses_json_series_from_pointer_fields() {
        let request = ApiPullRequest {
            request_id: None,
            source_id: None,
            url: Some("https://example.com/data.json".to_string()),
            parser: Some(SourceParser::JsonRecords),
            instrument_id: Some("TEST".to_string()),
            display_name: Some("Test".to_string()),
            asset_class: Some("equities".to_string()),
            currency: Some("USD".to_string()),
            source: Some("unit".to_string()),
            root_pointer: Some("/prices".to_string()),
            date_field: Some("d".to_string()),
            price_field: Some("p".to_string()),
            volume_field: Some("v".to_string()),
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        };
        let value = json!({
            "prices": [
                { "d": "2026-01", "p": "100.5", "v": 10 },
                { "d": "2026-02", "p": 102.0, "v": 11 }
            ]
        });

        let series = series_from_json(&request, &value).expect("series parsed");

        assert_eq!(series.instrument_id, "TEST");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(series.observations[0].price, 100.5);
        assert_eq!(series.observations[1].volume, Some(11.0));
    }

    #[test]
    fn des_surface_is_available_for_runtime_discovery() {
        let surface = des_surface_descriptor();

        assert_eq!(surface["crate"], "des_engine");
        assert!(surface["modules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "acausal"));
    }

    #[test]
    fn sentiment_placeholder_scores_documents_and_reports_credentials() {
        let response = analyze_sentiment(
            &test_config(),
            SentimentAnalyzeRequest {
                request_id: Some("sentiment-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                query: Some("$BTC oil recession".to_string()),
                instrument_ids: Some(vec!["BTC-USD".to_string(), "CL=F".to_string()]),
                documents: vec![
                    SentimentDocument {
                        source: "x-twitter".to_string(),
                        text: "$BTC bullish breakout with strong inflow and adoption".to_string(),
                        url: None,
                        author: None,
                        published_at: None,
                        weight: Some(2.0),
                    },
                    SentimentDocument {
                        source: "reddit".to_string(),
                        text: "Oil looks weak after recession and liquidity crunch chatter"
                            .to_string(),
                        url: None,
                        author: None,
                        published_at: None,
                        weight: Some(1.0),
                    },
                ],
            },
        )
        .expect("sentiment analysis succeeds");

        assert_eq!(response.request_id, "sentiment-unit");
        assert_eq!(response.document_count, 2);
        assert!(response.average_sentiment > 0.0);
        assert!(response.credential_status.x_bearer_token);
        assert!(response.credential_status.reddit_client_id);
        assert_eq!(response.source_scores.len(), 2);
        assert!(response.top_terms.iter().any(|term| term == "$btc"));
    }

    #[test]
    fn recommendations_return_company_and_commodity_rankings() {
        let response = generate_recommendations(
            &test_config(),
            RecommendationRequest {
                request_id: Some("recommendation-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                horizon_months: Some(18),
                company_limit: Some(20),
                commodity_limit: Some(30),
                scenario: Some("base".to_string()),
                series: Some(sample_market_series()),
                macro_context: Some(MacroContext {
                    policy_rate: Some(0.045),
                    expected_inflation: Some(0.026),
                    inflation: Some(0.031),
                    real_growth: Some(0.020),
                    ..MacroContext::default()
                }),
                macro_fiscal_context: Some(default_macro_fiscal_context()),
                venture_capital_context: Some(sample_venture_capital_context()),
                sentiment_context: Some(SentimentSignalContext {
                    average_sentiment: Some(0.10),
                    instrument_scores: None,
                    sector_scores: None,
                }),
            },
        )
        .expect("recommendations succeed");

        assert_eq!(response.request_id, "recommendation-unit");
        assert_eq!(response.company_buys.len(), 20);
        assert_eq!(response.company_dumps.len(), 20);
        assert_eq!(response.commodity_buys.len(), 30);
        assert_eq!(response.commodity_sells_or_dumps.len(), 30);
        assert!(response.company_buys[0].score >= response.company_buys[19].score);
        assert!(response.company_dumps[0].score <= response.company_dumps[19].score);
        assert!(response
            .methodology
            .iter()
            .any(|item| item.contains("VC sector flow")));
    }

    #[test]
    fn pipeline_plan_emits_spark_airflow_databricks_and_nats_intents() {
        let state = test_state();
        let plan = pipeline_plan_from_request(
            &state,
            PipelinePlanRequest {
                request_id: Some("pipeline-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                scenario: Some("soft-landing".to_string()),
                data_lake_uri: Some("s3a://dd-economics/unit".to_string()),
                include_recommendations: Some(true),
                publish_to_nats: Some(false),
                job_kinds: None,
                recommendation_request: None,
            },
        )
        .expect("pipeline plan succeeds");

        assert_eq!(plan.request_id, "pipeline-unit");
        assert_eq!(plan.job_intents.len(), 5);
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "spark-pipeline-server"
                && intent.kind == "INGEST_VALIDATE_PUBLISH"
                && intent.submit_eligible));
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "airflow" && !intent.submit_eligible));
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "databricks" && !intent.submit_eligible));
        assert_eq!(
            plan.pipeline_status.pipeline_intent_subject,
            PUBLIC_DATA_PIPELINE_JOBS_SUBJECT
        );
    }

    #[test]
    fn recommendation_validation_rejects_unbounded_vc_context() {
        let mut context = sample_venture_capital_context();
        context.deals[0].amount = f64::INFINITY;
        let error = generate_recommendations(
            &test_config(),
            RecommendationRequest {
                request_id: Some("bad-vc".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                horizon_months: Some(18),
                company_limit: Some(20),
                commodity_limit: Some(30),
                scenario: Some("base".to_string()),
                series: Some(sample_market_series()),
                macro_context: None,
                macro_fiscal_context: Some(default_macro_fiscal_context()),
                venture_capital_context: Some(context),
                sentiment_context: None,
            },
        )
        .expect_err("invalid vc amount rejected");

        assert!(error.contains("ventureCapitalContext.deals"));
    }

    #[test]
    fn pipeline_submit_url_rejects_external_hosts_by_default() {
        let mut config = test_config();
        config.spark_pipeline_url = Some("https://spark.example.com".to_string());
        config.allow_external_pipeline_urls = false;

        let error = validate_pipeline_submit_url(&config).expect_err("external URL rejected");

        assert!(error.contains("cluster-local"));
    }

    #[test]
    fn pipeline_submit_url_rejects_credentials_queries_and_fragments() {
        let mut config = test_config();
        config.spark_pipeline_url = Some(
            "http://user:secret@dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        );
        let error = validate_pipeline_submit_url(&config).expect_err("credentials rejected");
        assert!(error.contains("credentials"));

        config.spark_pipeline_url = Some(
            "http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085?token=secret".to_string(),
        );
        let error = validate_pipeline_submit_url(&config).expect_err("query rejected");
        assert!(error.contains("query strings"));

        config.spark_pipeline_url =
            Some("http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085/#frag".to_string());
        let error = validate_pipeline_submit_url(&config).expect_err("fragment rejected");
        assert!(error.contains("fragments"));
    }

    #[tokio::test]
    async fn source_auth_header_env_must_be_allowed() {
        let state = test_state();
        let request = ApiPullRequest {
            request_id: Some("auth-env-unit".to_string()),
            source_id: None,
            url: Some("https://api.worldbank.org/v2/country/US".to_string()),
            parser: None,
            instrument_id: None,
            display_name: None,
            asset_class: None,
            currency: None,
            source: Some("unit".to_string()),
            root_pointer: None,
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: Some("SERVER_AUTH_SECRET".to_string()),
            auth_header_name: Some("authorization".to_string()),
        };

        let error = pull_source(&state, request)
            .await
            .expect_err("non-economics auth env rejected before request");
        assert!(error.contains("authHeaderEnv"));
    }

    #[test]
    fn source_auth_header_name_blocks_transport_headers() {
        let error = validate_source_auth_header_name("host").expect_err("host header rejected");

        assert!(error.contains("hop-by-hop"));
    }

    #[test]
    fn public_source_catalog_covers_tradeable_and_macro_assets() {
        let ids = public_source_ids();

        assert!(ids.contains(&"treasury-debt-to-penny"));
        assert!(ids.contains(&"worldbank-us-gdp-current-usd"));
        assert!(ids.contains(&"coingecko-bitcoin-usd"));
        assert!(ids.contains(&"fred-wti-oil"));
        assert!(ids.contains(&"fred-gold"));
        assert!(ids.contains(&"fred-silver"));
        assert!(ids.contains(&"fred-sp500"));
        assert!(ids.contains(&"fred-mortgage30"));
        assert!(ids.contains(&"fred-usd-eur"));
        assert!(public_source_hosts().contains(&"api.fiscaldata.treasury.gov"));
    }

    #[test]
    fn source_id_template_fills_pull_metadata_and_rejects_url_override() {
        let mut request = source_id_pull_request("treasury-debt-to-penny");
        let template = apply_public_source_template(&mut request)
            .expect("source template resolves")
            .expect("template present");

        assert_eq!(template.host, "api.fiscaldata.treasury.gov");
        assert_eq!(request.instrument_id.as_deref(), Some("US-PUBLIC-DEBT"));
        assert_eq!(request.parser, Some(SourceParser::JsonRecords));
        validate_api_pull_request(&request, Some(&template)).expect("template request validates");

        let mut override_request = source_id_pull_request("treasury-debt-to-penny");
        override_request.url = Some("https://example.com/not-the-template.json".to_string());
        let error = apply_public_source_template(&mut override_request)
            .expect_err("sourceId URL override rejected");
        assert!(error.contains("url overrides"));
    }

    #[test]
    fn parses_treasury_fiscaldata_json_and_reports_quality() {
        let mut request = source_id_pull_request("treasury-debt-to-penny");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        let body = br#"{
            "data": [
                {"record_date":"2026-06-03","tot_pub_debt_out_amt":"39204974715248.65"},
                {"record_date":"2026-06-04","tot_pub_debt_out_amt":"39232150577283.87"},
                {"record_date":"2026-06-05","tot_pub_debt_out_amt":null}
            ]
        }"#;

        let (series, quality) = series_from_bytes(&request, body).expect("treasury series parsed");

        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "US-PUBLIC-DEBT");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(quality.dropped_points, 1);
        assert_eq!(quality.first_date.as_deref(), Some("2026-06-03"));
        assert_eq!(quality.last_date.as_deref(), Some("2026-06-04"));
    }

    #[test]
    fn parses_worldbank_records_and_skips_latest_null() {
        let mut request = source_id_pull_request("worldbank-us-gdp-current-usd");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        let body = br#"[
            {"page":1,"pages":1,"per_page":3,"total":3},
            [
                {"date":"2025","value":null},
                {"date":"2024","value":28750956130731.2},
                {"date":"2023","value":27292170793214.4}
            ]
        ]"#;

        let (series, quality) = series_from_bytes(&request, body).expect("worldbank series parsed");

        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "US-GDP-CURRENT-USD");
        assert_eq!(series.observations[0].date, "2023");
        assert_eq!(quality.dropped_points, 1);
    }

    #[test]
    fn parses_coingecko_tuple_arrays() {
        let mut request = source_id_pull_request("coingecko-bitcoin-usd");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        let body = br#"{
            "prices": [
                [1780790400000,60861.88012897632],
                [1780704000000,60921.79441516493]
            ],
            "market_caps": [],
            "total_volumes": []
        }"#;

        let (series, quality) = series_from_bytes(&request, body).expect("coingecko series parsed");

        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "BTC-USD");
        assert_eq!(series.observations[0].date, "1780704000000");
        assert_eq!(quality.parser, SourceParser::JsonTupleArray);
        assert_eq!(quality.observed_points, 2);
    }

    #[test]
    fn parses_csv_records_and_drops_missing_values() {
        let request = ApiPullRequest {
            request_id: None,
            source_id: None,
            url: Some("https://example.com/dgs10.csv".to_string()),
            parser: Some(SourceParser::CsvRecords),
            instrument_id: Some("DGS10".to_string()),
            display_name: Some("10-Year Treasury".to_string()),
            asset_class: Some("rates".to_string()),
            currency: Some("PCT".to_string()),
            source: Some("unit-csv".to_string()),
            root_pointer: None,
            date_field: Some("observation_date".to_string()),
            price_field: Some("DGS10".to_string()),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        };
        let body = "observation_date,DGS10\n2026-06-01,4.45\n2026-06-02,.\n2026-06-03,4.41\n";

        let (series, quality) =
            series_from_bytes(&request, body.as_bytes()).expect("csv series parsed");

        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(quality.dropped_points, 1);
        assert_eq!(quality.min_price, Some(4.41));
    }

    #[test]
    fn source_policy_blocks_private_redirect_targets_and_custom_ports() {
        let link_local = reqwest::Url::parse("https://169.254.169.254/latest/meta-data").unwrap();
        let error = validate_source_url(&link_local, false).expect_err("link-local blocked");
        assert!(error.contains("ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS"));

        let custom_port = reqwest::Url::parse("https://api.worldbank.org:8443/v2").unwrap();
        let error = validate_source_url(&custom_port, false).expect_err("custom port blocked");
        assert!(error.contains("custom source URL ports"));

        let public_172 = reqwest::Url::parse("https://172.200.1.1/data.json").unwrap();
        validate_source_url(&public_172, false).expect("172.200/16 is not RFC1918 private");
    }

    #[test]
    fn source_host_allowlist_restricts_ad_hoc_public_pulls() {
        let allowed = vec!["api.worldbank.org".to_string()];
        let worldbank = reqwest::Url::parse("https://api.worldbank.org/v2/country/US").unwrap();
        let coingecko = reqwest::Url::parse("https://api.coingecko.com/api/v3/ping").unwrap();

        validate_source_host_allowlist(&worldbank, &allowed).expect("worldbank allowed");
        let error = validate_source_host_allowlist(&coingecko, &allowed)
            .expect_err("coingecko blocked by allowlist");
        assert!(error.contains("ECONOMICS_ALLOWED_SOURCE_HOSTS"));
    }

    #[test]
    fn duplicate_ingested_observation_dates_are_rejected() {
        let series = MarketSeries {
            instrument_id: "DUP".to_string(),
            display_name: None,
            asset_class: "equities".to_string(),
            currency: Some("USD".to_string()),
            source: Some("unit".to_string()),
            observations: vec![
                MarketObservation {
                    date: "2026-01-01".to_string(),
                    price: 100.0,
                    volume: None,
                },
                MarketObservation {
                    date: "2026-01-01".to_string(),
                    price: 101.0,
                    volume: None,
                },
            ],
            features: None,
        };

        let error = validate_series(&[series]).expect_err("duplicate date rejected");
        assert!(error.contains("duplicated"));
    }

    #[test]
    fn observability_payload_advertises_explicit_otel_and_dd_log_schema() {
        let state = test_state();
        let payload = observability_payload(&state);

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["loki"]["structuredLogSchema"], "dd.log.v1");
        assert_eq!(payload["otel"]["mode"], "explicit-only");
        assert_eq!(payload["otel"]["autoInstrumentation"], false);
        assert_eq!(payload["otel"]["runtimeMonkeyPatching"], false);
        assert_eq!(payload["prometheus"]["metricsRoute"], "GET /metrics");
    }

    #[test]
    fn integration_health_payload_reports_dependency_status_without_secrets() {
        let mut config = test_config();
        config.server_auth_secret = Some("ultra-private-unit-token".to_string());
        let state = AppState {
            config: Arc::new(config),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("dd-economics-server/0.1 test-source-pull")
                .build()
                .unwrap(),
            series_store: Arc::new(RwLock::new(BTreeMap::new())),
        };
        let payload = integration_health_payload(&state);
        let dependencies = payload["dependencies"]
            .as_array()
            .expect("dependencies array");

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["coreReady"], true);
        assert!(dependencies
            .iter()
            .any(|dependency| dependency["id"] == "source-auth-env-allowlist"
                && dependency["status"] == "ready"));
        assert!(dependencies
            .iter()
            .any(|dependency| dependency["id"] == "spark-pipeline-server"));
        assert!(!payload.to_string().contains("ultra-private-unit-token"));
    }

    #[test]
    fn telemetry_log_record_uses_dd_log_v1_envelope() {
        let record = telemetry_log_record(
            "INFO",
            "economics.unit.test",
            "unit test log",
            json!({ "requestId": "unit" }),
        );

        assert_eq!(record["schema"], "dd.log.v1");
        assert_eq!(record["severity_text"], "INFO");
        assert_eq!(record["severity_number"], 9);
        assert_eq!(record["resource_service_name"], SERVICE_NAME);
        assert_eq!(record["event_name"], "economics.unit.test");
        assert_eq!(record["attributes"]["requestId"], "unit");
    }

    #[tokio::test]
    async fn metrics_expose_source_and_observability_counters() {
        let response = metrics(State(test_state())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("metrics body bytes");
        let body = String::from_utf8(bytes.to_vec()).expect("metrics utf8");

        assert!(body.contains("dd_economics_server_source_pull_success_total"));
        assert!(body.contains("dd_economics_server_source_pull_failure_total"));
        assert!(body.contains("dd_economics_server_source_pull_bytes_total"));
        assert!(body.contains("dd_economics_server_source_pull_stored_points_total"));
        assert!(body.contains("dd_economics_server_source_pull_last_success_unix_seconds"));
        assert!(body.contains("dd_economics_server_observability_requests_total"));
        assert!(body.contains("dd_economics_server_integration_health_requests_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_attempts_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_success_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_failure_total"));
        assert!(body.contains("dd_economics_server_pipeline_submit_success_total"));
        assert!(body.contains("dd_economics_server_pipeline_submit_failure_total"));
    }

    #[tokio::test]
    #[ignore = "uses live public APIs and should be run manually"]
    async fn public_source_templates_fetch_live_external_data_when_available() {
        let state = test_state();
        let mut successes = 0usize;
        for source_id in [
            "treasury-debt-to-penny",
            "worldbank-us-gdp-current-usd",
            "coingecko-bitcoin-usd",
        ] {
            match pull_source(&state, source_id_pull_request(source_id)).await {
                Ok(response) => {
                    assert!(response.stored_points >= 2);
                    assert!(response.quality.is_some());
                    successes += 1;
                }
                Err(error) => {
                    tracing::error!("live public source {source_id} unavailable or changed: {error}");
                }
            }
        }
        if successes == 0 {
            tracing::error!("no live public sources were reachable; skipping external assertions");
            return;
        }
        let stored = state.series_store.read().unwrap().len();
        assert!(stored >= successes);
    }
