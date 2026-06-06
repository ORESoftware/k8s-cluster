use axum::http::StatusCode;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::rand_core::OsRng;
use serde_json::json;

use crate::config::Config;

use super::braintree::BraintreeOAuth;
use super::bridge::{BridgeApi, BridgeCredential};
use super::circle::{CircleApi, CircleCredential};
use super::coinbase::{CoinbaseCommerceApi, CoinbaseCredential, CoinbasePrimeApi, CoinbaseVariant};
use super::coinflow::{CoinflowApi, CoinflowCredential};
use super::fireblocks::{FireblocksApi, FireblocksCredential};
use super::gocardless::{GoCardlessApi, GoCardlessCredential};
use super::mercury::{MercuryApi, MercuryCredential};
use super::mock_http::{ExpectedRequest, ProviderMock};
use super::moneygram::{MoneyGramApi, MoneyGramCredential};
use super::paypal::{PaypalOAuth, verify_webhook_signature as verify_paypal_webhook_signature};
use super::plaid::PlaidLink;
use super::remitly::{RemitlyApi, RemitlyCredential};
use super::revolut::{RevolutApi, RevolutCredential};
use super::stripe::StripeApi;
use super::western_union::{WesternUnionApi, WesternUnionCredential};
use super::wise::{WiseApi, WiseCredential};

#[tokio::test]
async fn stripe_balance_transactions_use_connected_account_headers() {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("sk_test:")
    );
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/v1/balance_transactions")
            .query("limit", "2")
            .query("ending_before", "bt_prev")
            .header("authorization", auth)
            .header("stripe-account", "acct_123")
            .header("stripe-version", "2026-04-22.dahlia")
            .respond_json(json!({
                "data": [{
                    "id": "bt_1",
                    "amount": 1000,
                    "fee": 30,
                    "net": 970,
                    "currency": "usd",
                    "type": "charge",
                    "status": "available",
                    "created": 1716423000
                }],
                "has_more": false
            })),
    ])
    .await;

    let api = StripeApi::with_base_url_for_tests(
        "sk_test".into(),
        "acct_123".into(),
        "2026-04-22.dahlia".into(),
        mock.base_url(),
    );
    let (items, has_more) = api
        .list_balance_transactions(Some("bt_prev"), 2)
        .await
        .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "bt_1");
    assert!(!has_more);
    mock.assert_finished().await;
}

#[tokio::test]
async fn bridge_transfers_use_api_key_and_cursor_query() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/transfers")
            .query("limit", "50")
            .query("starting_after", "tr_prev")
            .header("api-key", "bridge_key")
            .respond_json(json!({
                "data": [{
                    "id": "tr_1",
                    "state": "payment_processed",
                    "amount": "12.34",
                    "currency": "USD"
                }],
                "count": 1
            })),
    ])
    .await;

    let api = BridgeApi::with_base_url_for_tests(
        BridgeCredential {
            api_key: "bridge_key".into(),
            webhook_secret: None,
            webhook_public_key_pem: None,
            environment: "sandbox".into(),
        },
        mock.base_url(),
    );
    let (items, next) = api.list_transfers(50, Some("tr_prev")).await.unwrap();

    assert_eq!(items[0].id, "tr_1");
    assert_eq!(next, Some("tr_1".into()));
    mock.assert_finished().await;
}

#[tokio::test]
async fn circle_transfers_use_bearer_auth_and_page_cursor() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/v1/businessAccount/transfers")
            .query("pageSize", "10")
            .query("pageAfter", "cursor_1")
            .header("authorization", "Bearer circle_key")
            .header("accept", "application/json")
            .respond_json(json!({
                "data": [{
                    "id": "circle_tr_1",
                    "amount": { "amount": "5.00", "currency": "USD" },
                    "status": "complete"
                }]
            })),
    ])
    .await;

    let api = CircleApi::with_base_url_for_tests(
        CircleCredential {
            api_key: "circle_key".into(),
            environment: "sandbox".into(),
            webhook_secret: None,
        },
        mock.base_url(),
    );
    let page = api.list_transfers(Some("cursor_1"), 10).await.unwrap();

    assert_eq!(page.data[0].id, "circle_tr_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn coinflow_webhook_activity_uses_merchant_headers() {
    let start = utc("2026-01-01T00:00:00Z");
    let end = utc("2026-01-02T00:00:00Z");
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/api/merchant/webhooks")
            .query("page", "3")
            .query("limit", "25")
            .query("startDate", start.to_rfc3339())
            .query("endDate", end.to_rfc3339())
            .header("authorization", "coinflow_key")
            .header("x-coinflow-auth-merchant-id", "merchant_1")
            .respond_json(json!({
                "data": [{
                    "id": "evt_1",
                    "type": "payment.settled",
                    "amount_cents": 1299,
                    "currency": "USD"
                }],
                "page": 3,
                "total_pages": 4
            })),
    ])
    .await;

    let api = CoinflowApi::with_base_url_for_tests(
        CoinflowCredential {
            api_key: "coinflow_key".into(),
            merchant_id: "merchant_1".into(),
            environment: "sandbox".into(),
            webhook_validation_key: None,
        },
        mock.base_url(),
    );
    let (items, has_more) = api
        .list_webhook_activity(Some(start), Some(end), 3, 25)
        .await
        .unwrap();

    assert_eq!(items[0].id, "evt_1");
    assert!(has_more);
    mock.assert_finished().await;
}

#[tokio::test]
async fn coinbase_commerce_charges_use_versioned_api_headers() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/charges")
            .query("limit", "20")
            .query("order", "asc")
            .query("starting_after", "charge_prev")
            .header("x-cc-api-key", "commerce_key")
            .header("x-cc-version", "2018-03-22")
            .respond_json(json!({
                "data": [{ "id": "charge_1" }],
                "pagination": { "cursor_range": ["charge_0", "charge_1"] }
            })),
    ])
    .await;

    let api = CoinbaseCommerceApi::with_base_url_for_tests(
        CoinbaseCredential {
            api_key: "commerce_key".into(),
            webhook_secret: "whsec".into(),
            variant: CoinbaseVariant::Commerce,
            api_secret: None,
            passphrase: None,
            portfolio_id: None,
        },
        mock.base_url(),
    );
    let (items, next) = api.list_charges(20, Some("charge_prev")).await.unwrap();

    assert_eq!(items[0].id, "charge_1");
    assert_eq!(next, Some("charge_1".into()));
    mock.assert_finished().await;
}

#[tokio::test]
async fn coinbase_prime_transactions_use_signed_headers() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/v1/portfolios/portfolio_1/transactions")
            .query("limit", "100")
            .query("cursor", "cursor_1")
            .header("x-cb-access-key", "prime_key")
            .header("x-cb-access-passphrase", "passphrase")
            .header("accept", "application/json")
            .header_present("x-cb-access-signature")
            .header_present("x-cb-access-timestamp")
            .respond_json(json!({
                "transactions": [{
                    "id": "prime_tx_1",
                    "type": "DEPOSIT",
                    "status": "TRANSACTION_COMPLETED"
                }],
                "pagination": { "next_cursor": null, "has_next": false }
            })),
    ])
    .await;

    let api = CoinbasePrimeApi::with_base_url_for_tests(
        CoinbaseCredential {
            api_key: "prime_key".into(),
            webhook_secret: "whsec".into(),
            variant: CoinbaseVariant::Prime,
            api_secret: Some("c2hoaGgK".into()),
            passphrase: Some("passphrase".into()),
            portfolio_id: Some("portfolio_1".into()),
        },
        mock.base_url(),
    )
    .unwrap();
    let page = api.list_transactions(Some("cursor_1"), 100).await.unwrap();

    assert_eq!(page.transactions[0].id, "prime_tx_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn fireblocks_transactions_use_jwt_and_api_key_headers() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/v1/transactions")
            .query("limit", "200")
            .query("orderBy", "createdAt")
            .query("sort", "ASC")
            .query("after", "1716423000000")
            .header("x-api-key", "fireblocks_key")
            .header("accept", "application/json")
            .header_present("authorization")
            .respond_json(json!([{
                "id": "fb_tx_1",
                "status": "COMPLETED",
                "assetId": "USDC"
            }])),
    ])
    .await;

    let api = FireblocksApi::with_base_url_for_tests(
        FireblocksCredential {
            api_key: "fireblocks_key".into(),
            api_secret_pem: test_rsa_private_pem(),
            webhook_public_key_pem: None,
            environment: "sandbox".into(),
        },
        mock.base_url(),
    );
    let items = api
        .list_transactions(Some(1_716_423_000_000), 200)
        .await
        .unwrap();

    assert_eq!(items[0].id, "fb_tx_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn gocardless_payments_use_version_header_and_created_filter() {
    let created = utc("2026-02-01T00:00:00Z");
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/payments")
            .query("limit", "25")
            .query("after", "PM_prev")
            .query("created_at[gte]", created.to_rfc3339())
            .header("authorization", "Bearer gc_token")
            .header("gocardless-version", "2015-07-06")
            .header("accept", "application/json")
            .respond_json(json!({
                "payments": [{
                    "id": "PM0001",
                    "amount": 1000,
                    "amount_refunded": 0,
                    "currency": "GBP"
                }],
                "meta": { "cursors": { "after": "PM0001" } }
            })),
    ])
    .await;

    let api = GoCardlessApi::with_base_url_for_tests(
        GoCardlessCredential {
            access_token: "gc_token".into(),
            webhook_secret: None,
            environment: "sandbox".into(),
        },
        mock.base_url(),
    );
    let (items, next) = api
        .list_payments(25, Some("PM_prev"), Some(created))
        .await
        .unwrap();

    assert_eq!(items[0].id, "PM0001");
    assert_eq!(next, Some("PM0001".into()));
    mock.assert_finished().await;
}

#[tokio::test]
async fn mercury_transactions_use_account_path_and_date_query() {
    let start = utc("2026-03-01T00:00:00Z");
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/account/acct_1/transactions")
            .query("limit", "75")
            .query("offset", "150")
            .query("order", "asc")
            .query("start", "2026-03-01")
            .header("authorization", "Bearer mercury_key")
            .respond_json(json!({
                "transactions": [{
                    "id": "mercury_tx_1",
                    "amount": -42.50,
                    "currency": "USD"
                }],
                "total": 1
            })),
    ])
    .await;

    let api = MercuryApi::with_base_url_for_tests(
        MercuryCredential {
            api_key: "mercury_key".into(),
            watched_account_ids: Vec::new(),
            webhook_secret: None,
        },
        mock.base_url(),
    );
    let items = api
        .list_transactions("acct_1", 75, 150, Some(start))
        .await
        .unwrap();

    assert_eq!(items[0].id, "mercury_tx_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn revolut_transactions_use_bearer_auth_and_time_window() {
    let from = utc("2026-04-01T00:00:00Z");
    let to = utc("2026-04-02T00:00:00Z");
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/transactions")
            .query("count", "100")
            .query("from", from.to_rfc3339())
            .query("to", to.to_rfc3339())
            .header("authorization", "Bearer revolut_token")
            .respond_json(json!([{
                "id": "rev_tx_1",
                "type": "card_payment",
                "state": "completed"
            }])),
    ])
    .await;

    let api = RevolutApi::with_base_url_for_tests(
        RevolutCredential {
            access_token: "revolut_token".into(),
            refresh_token: None,
            environment: "sandbox".into(),
            webhook_secret: None,
        },
        mock.base_url(),
    );
    let items = api
        .list_transactions(Some(from), Some(to), 100)
        .await
        .unwrap();

    assert_eq!(items[0].id, "rev_tx_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn wise_activities_use_profile_path_and_cursor() {
    let since = utc("2026-05-01T00:00:00Z");
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/v1/profiles/profile_1/activities")
            .query("size", "100")
            .query("nextCursor", "cursor_1")
            .query("since", since.to_rfc3339())
            .header("authorization", "Bearer wise_token")
            .respond_json(json!({
                "cursor": "cursor_2",
                "activities": [{
                    "id": "wise_act_1",
                    "type": "TRANSFER"
                }]
            })),
    ])
    .await;

    let api = WiseApi::with_base_url_for_tests(
        WiseCredential {
            api_token: "wise_token".into(),
            profile_id: "profile_1".into(),
            environment: "sandbox".into(),
        },
        mock.base_url(),
    );
    let (items, next) = api
        .list_activities(Some("cursor_1"), Some(since), None, 500)
        .await
        .unwrap();

    assert_eq!(items[0].id, "wise_act_1");
    assert_eq!(next, Some("cursor_2".into()));
    mock.assert_finished().await;
}

#[tokio::test]
async fn remitly_partner_transfers_use_configured_base_url_and_partner_header() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/transfers")
            .query("limit", "25")
            .query("cursor", "cursor_1")
            .query("recipientId", "recipient_1")
            .header("authorization", "Bearer remitly_key")
            .header("x-remitly-partner-id", "partner_1")
            .respond_json(json!({
                "data": [{
                    "id": "remitly_tr_1",
                    "recipientId": "recipient_1",
                    "status": "delivered",
                    "sendAmount": "100.00",
                    "sendCurrency": "USD",
                    "receiveAmount": "5125.00",
                    "receiveCurrency": "PHP"
                }],
                "nextCursor": "cursor_2"
            })),
    ])
    .await;

    let api = RemitlyApi::with_base_url_for_tests(
        RemitlyCredential {
            api_key: Some("remitly_key".into()),
            partner_id: Some("partner_1".into()),
            watched_recipients: vec!["recipient_1".into()],
            api_base_url: None,
            environment: "sandbox".into(),
            notes: None,
        },
        mock.base_url(),
    )
    .unwrap();
    let (items, next) = api
        .list_partner_transfers(25, Some("cursor_1"), Some("recipient_1"))
        .await
        .unwrap();

    assert_eq!(items[0].id, "remitly_tr_1");
    assert_eq!(items[0].recipient_id.as_deref(), Some("recipient_1"));
    assert_eq!(next.as_deref(), Some("cursor_2"));
    mock.assert_finished().await;
}

#[tokio::test]
async fn moneygram_status_lookup_gets_token_then_queries_reference_number() {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("mg_client:mg_secret")
    );
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/oauth/accesstoken")
            .query("grant_type", "client_credentials")
            .header("authorization", auth)
            .header("accept", "application/json")
            .respond_json(json!({
                "access_token": "mg_access",
                "expires_in": "3599",
                "token_type": "BearerToken"
            })),
        ExpectedRequest::get("/status/v1/transactions")
            .query("agentPartnerId", "agent_1")
            .query("referenceNumber", "12345678")
            .query("userLanguage", "en-US")
            .query("targetAudience", "AGENT_FACING")
            .header("authorization", "Bearer mg_access")
            .header("accept", "application/json")
            .header_present("x-mg-clientrequestid")
            .respond_json(json!({
                "transactionId": "mg_tx_1",
                "referenceNumber": "12345678",
                "transactionStatus": "AVAILABLE",
                "transactionSubStatus": "READY_FOR_PICKUP"
            })),
    ])
    .await;

    let api = MoneyGramApi::with_base_url_for_tests(
        MoneyGramCredential {
            client_id: "mg_client".into(),
            client_secret: "mg_secret".into(),
            agent_partner_id: "agent_1".into(),
            user_language: "en-US".into(),
            environment: "sandbox".into(),
            webhook_secret: None,
        },
        mock.base_url(),
    );
    let status = api
        .retrieve_transaction_status("12345678", Some("AGENT_FACING"))
        .await
        .unwrap();

    assert_eq!(status.transaction_id.as_deref(), Some("mg_tx_1"));
    assert_eq!(status.transaction_status.as_deref(), Some("AVAILABLE"));
    mock.assert_finished().await;
}

#[tokio::test]
async fn western_union_holding_balance_uses_client_and_currency_path() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/HoldingBalance/client_1/USD").respond_json(json!({
            "clientId": "client_1",
            "currencyCode": "USD",
            "balance": {
                "currencyCode": "USD",
                "amount": 1250.75
            }
        })),
    ])
    .await;

    let api = WesternUnionApi::with_base_url_for_tests(
        WesternUnionCredential {
            client_id: "client_1".into(),
            environment: "sandbox".into(),
            client_certificate_pem: None,
            client_private_key_pem: None,
            notes: None,
        },
        mock.base_url(),
    );
    let balance = api.get_holding_balance("usd").await.unwrap();

    assert_eq!(balance.client_id.as_deref(), Some("client_1"));
    assert_eq!(balance.balance.and_then(|b| b.amount), Some(1250.75));
    mock.assert_finished().await;
}

#[tokio::test]
async fn western_union_batch_payments_percent_encodes_path_segments() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/customers/client_1/batches/batch%2F2026%2F01/payments")
            .respond_json(json!({
                "payments": [{
                    "id": "wu_payment_1",
                    "status": "paid",
                    "partnerReference": "partner_ref_1",
                    "amount": 2500,
                    "currencyCode": "USD"
                }]
            })),
    ])
    .await;

    let api = WesternUnionApi::with_base_url_for_tests(
        WesternUnionCredential {
            client_id: "client_1".into(),
            environment: "sandbox".into(),
            client_certificate_pem: None,
            client_private_key_pem: None,
            notes: None,
        },
        mock.base_url(),
    );
    let payments = api.list_batch_payments("batch/2026/01").await.unwrap();

    assert_eq!(payments[0].id, "wu_payment_1");
    mock.assert_finished().await;
}

#[tokio::test]
async fn provider_client_surfaces_non_success_body() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::get("/transfers")
            .query("limit", "1")
            .header("api-key", "bridge_key")
            .respond_status_json(
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error": "rate_limited" }),
            ),
    ])
    .await;

    let api = BridgeApi::with_base_url_for_tests(
        BridgeCredential {
            api_key: "bridge_key".into(),
            webhook_secret: None,
            webhook_public_key_pem: None,
            environment: "sandbox".into(),
        },
        mock.base_url(),
    );
    let err = api.list_transfers(1, None).await.unwrap_err();

    assert!(err.to_string().contains("rate_limited"));
    mock.assert_finished().await;
}

#[tokio::test]
async fn paypal_oauth_exchange_uses_configured_api_base() {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("paypal_client:paypal_secret")
    );
    let mock = ProviderMock::start(vec![
        ExpectedRequest::post("/v1/oauth2/token")
            .header("authorization", auth)
            .header("content-type", "application/x-www-form-urlencoded")
            .respond_json(json!({
                "access_token": "paypal_access",
                "refresh_token": "paypal_refresh",
                "expires_in": 3600,
                "scope": "openid reporting",
                "payer_id": "merchant_1"
            })),
    ])
    .await;

    let mut cfg = Config::for_tests();
    cfg.paypal_client_id = Some("paypal_client".into());
    cfg.paypal_client_secret = Some("paypal_secret".into());
    cfg.paypal_api_base_override = Some(mock.base_url());
    let result = PaypalOAuth::new(&cfg)
        .exchange_code("code_1")
        .await
        .unwrap();

    assert_eq!(result.external_account_id, "merchant_1");
    assert_eq!(
        result.scopes,
        vec!["openid".to_string(), "reporting".to_string()]
    );
    mock.assert_finished().await;
}

#[tokio::test]
async fn paypal_webhook_verify_uses_token_then_json_verify_call() {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("paypal_client:paypal_secret")
    );
    let event = json!({ "id": "WH-1", "event_type": "PAYMENT.CAPTURE.COMPLETED" });
    let mock = ProviderMock::start(vec![
        ExpectedRequest::post("/v1/oauth2/token")
            .header("authorization", auth)
            .header("content-type", "application/x-www-form-urlencoded")
            .respond_json(json!({ "access_token": "paypal_access" })),
        ExpectedRequest::post("/v1/notifications/verify-webhook-signature")
            .header("authorization", "Bearer paypal_access")
            .json_body(json!({
                "auth_algo": "SHA256withRSA",
                "cert_url": "https://api-m.paypal.com/cert",
                "transmission_id": "tid",
                "transmission_sig": "sig",
                "transmission_time": "2026-01-01T00:00:00Z",
                "webhook_id": "webhook_1",
                "webhook_event": event
            }))
            .respond_json(json!({ "verification_status": "SUCCESS" })),
    ])
    .await;

    let mut cfg = Config::for_tests();
    cfg.paypal_client_id = Some("paypal_client".into());
    cfg.paypal_client_secret = Some("paypal_secret".into());
    cfg.paypal_webhook_id = Some("webhook_1".into());
    cfg.paypal_api_base_override = Some(mock.base_url());

    let ok = verify_paypal_webhook_signature(
        &cfg,
        "SHA256withRSA",
        "https://api-m.paypal.com/cert",
        "tid",
        "sig",
        "2026-01-01T00:00:00Z",
        &event,
    )
    .await
    .unwrap();

    assert!(ok);
    mock.assert_finished().await;
}

#[tokio::test]
async fn braintree_oauth_exchange_uses_configured_api_base() {
    let mock = ProviderMock::start(vec![
        ExpectedRequest::post("/oauth/access_tokens")
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .respond_json(json!({
                "credentials": {
                    "access_token": "bt_access",
                    "refresh_token": "bt_refresh",
                    "token_type": "Bearer"
                },
                "merchant": { "public_id": "merchant_bt_1" },
                "scope": "read_only,transactions"
            })),
    ])
    .await;

    let mut cfg = Config::for_tests();
    cfg.braintree_client_id = Some("bt_client".into());
    cfg.braintree_client_secret = Some("bt_secret".into());
    cfg.braintree_api_base_override = Some(mock.base_url());
    let result = BraintreeOAuth::new(&cfg)
        .exchange_code("bt_code")
        .await
        .unwrap();

    assert_eq!(result.external_account_id, "merchant_bt_1");
    assert_eq!(
        result.scopes,
        vec!["read_only".to_string(), "transactions".to_string()]
    );
    mock.assert_finished().await;
}

#[tokio::test]
async fn plaid_link_and_sync_use_json_payload_contracts() {
    let tenant_id = uuid::Uuid::new_v4();
    let mock = ProviderMock::start(vec![
        ExpectedRequest::post("/link/token/create")
            .json_body(json!({
                "client_id": "plaid_client",
                "secret": "plaid_secret",
                "client_name": "billing-server",
                "language": "en",
                "country_codes": ["US"],
                "products": ["transactions"],
                "user": { "client_user_id": tenant_id.to_string() },
                "webhook": "https://billing.example/v1/webhooks/plaid"
            }))
            .respond_json(json!({
                "link_token": "link-sandbox-1",
                "expiration": "2026-01-01T01:00:00Z"
            })),
        ExpectedRequest::post("/item/public_token/exchange")
            .json_body(json!({
                "client_id": "plaid_client",
                "secret": "plaid_secret",
                "public_token": "public-sandbox-1"
            }))
            .respond_json(json!({
                "access_token": "access-sandbox-1",
                "item_id": "item_1"
            })),
        ExpectedRequest::post("/transactions/sync")
            .json_body(json!({
                "client_id": "plaid_client",
                "secret": "plaid_secret",
                "access_token": "access-sandbox-1",
                "cursor": "cursor_1",
                "count": 25
            }))
            .respond_json(json!({
                "added": [{
                    "transaction_id": "plaid_tx_1",
                    "account_id": "acct_1",
                    "amount": -10.25,
                    "iso_currency_code": "USD"
                }],
                "modified": [],
                "removed": [],
                "next_cursor": "cursor_2",
                "has_more": false
            })),
    ])
    .await;

    let mut cfg = Config::for_tests();
    cfg.plaid_client_id = Some("plaid_client".into());
    cfg.plaid_secret = Some("plaid_secret".into());
    cfg.oauth_redirect_base = "https://billing.example".into();
    cfg.plaid_api_base_override = Some(mock.base_url());
    let plaid = PlaidLink::new(&cfg);

    let link_token = plaid.create_link_token(tenant_id).await.unwrap();
    let exchange = plaid
        .exchange_public_token("public-sandbox-1", Some("ins_1"), Some("Bank"))
        .await
        .unwrap();
    let sync = plaid
        .sync_transactions("access-sandbox-1", Some("cursor_1"), 25)
        .await
        .unwrap();

    assert_eq!(link_token, "link-sandbox-1");
    assert_eq!(exchange.external_account_id, "item_1");
    assert_eq!(sync.added[0].transaction_id, "plaid_tx_1");
    assert_eq!(sync.next_cursor, "cursor_2");
    mock.assert_finished().await;
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn test_rsa_private_pem() -> String {
    let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    private.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
}
