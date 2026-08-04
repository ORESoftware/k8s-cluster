async fn create_checkout_session(
    State(state): State<SharedCheckoutState>,
    Path(tenant_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CreateCheckoutRequest>,
) -> Result<Response, CheckoutError> {
    require_checkout_bearer(&headers, &state.cfg.api_bearer)?;
    require_allowed_tenant(&state.cfg, tenant_id)?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let intent = request.normalize(&state.cfg)?;
    let idempotency_hash = idempotency_key_hash(&idempotency_key);
    let fingerprint = checkout_intent_fingerprint(&intent)?;

    let existing = match load_checkout_by_idempotency(&state.db, tenant_id, &idempotency_hash).await {
        Ok(value) => Some(value),
        Err(CheckoutError::NotFound) => None,
        Err(error) => return Err(error),
    };
    if let Some(stored) = existing.as_ref() {
        ensure_matching_checkout_intent(stored, &fingerprint)?;
        if stored.provider_session_id.is_some() {
            return Ok((StatusCode::OK, Json(stored.view()?)).into_response());
        }
    }

    let selected_connection = if existing.is_none() {
        Some(select_checkout_connection(&state.db, tenant_id).await?)
    } else {
        None
    };
    let inserted = if let Some(connection) = selected_connection.as_ref() {
        insert_checkout_intent(
            &state.db,
            tenant_id,
            connection,
            &idempotency_hash,
            &fingerprint,
            &intent,
        )
        .await?
    } else {
        false
    };

    let stored = load_checkout_by_idempotency(&state.db, tenant_id, &idempotency_hash).await?;
    ensure_matching_checkout_intent(&stored, &fingerprint)?;
    if stored.provider_session_id.is_some() {
        return Ok((StatusCode::OK, Json(stored.view()?)).into_response());
    }

    let connection = load_checkout_connection(
        &state.db,
        tenant_id,
        stored.provider_connection_id,
    )
    .await?;
    let provider_session = create_stripe_checkout(
        &state,
        tenant_id,
        &connection,
        stored.id,
        &idempotency_hash,
        &intent,
    )
    .await?;
    let stored = persist_stripe_session(
        &state.db,
        tenant_id,
        stored.id,
        &provider_session,
    )
    .await?;
    let status = if inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(stored.view()?)).into_response())
}

async fn get_checkout_session(
    State(state): State<SharedCheckoutState>,
    Path((tenant_id, session_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Result<Json<CheckoutSessionView>, CheckoutError> {
    require_checkout_bearer(&headers, &state.cfg.api_bearer)?;
    require_allowed_tenant(&state.cfg, tenant_id)?;
    if !valid_stripe_session_id(&session_id) {
        return Err(CheckoutError::NotFound);
    }
    let stored = load_checkout_by_provider_session(&state.db, tenant_id, &session_id).await?;
    let connection = load_checkout_connection(
        &state.db,
        tenant_id,
        stored.provider_connection_id,
    )
    .await?;
    let provider_session = retrieve_stripe_checkout(&state, &connection, &stored).await?;
    if provider_session.id != session_id {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe returned a different checkout session than requested".to_string(),
        ));
    }
    let stored = persist_stripe_session(
        &state.db,
        tenant_id,
        stored.id,
        &provider_session,
    )
    .await?;
    Ok(Json(stored.view()?))
}

fn ensure_matching_checkout_intent(
    stored: &StoredCheckout,
    expected_fingerprint: &str,
) -> Result<(), CheckoutError> {
    if stored.intent_fingerprint != expected_fingerprint {
        return Err(CheckoutError::Conflict(
            "Idempotency-Key was already used for a different checkout intent".to_string(),
        ));
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::warn!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CheckoutConfig {
        CheckoutConfig {
            host: "127.0.0.1".parse().unwrap(),
            port: 8088,
            database_url: "postgres://localhost/test".to_string(),
            api_bearer: "b".repeat(32),
            allowed_tenants: [Uuid::nil()].into_iter().collect(),
            stripe_api_key: "sk_test_example_not_real".to_string(),
            stripe_api_version: DEFAULT_STRIPE_API_VERSION.to_string(),
            stripe_api_base: "https://api.stripe.com".to_string(),
            return_url_prefixes: vec![
                AllowedReturnPrefix::parse("https://fab.example/jobs").unwrap(),
            ],
            checkout_url_hosts: ["checkout.stripe.com".to_string()]
                .into_iter()
                .collect(),
        }
    }

    fn sample_request() -> CreateCheckoutRequest {
        CreateCheckoutRequest {
            client_reference_id: "01900000-0000-7000-8000-000000000001".to_string(),
            amount_minor: 12_500,
            currency: " usd ".to_string(),
            description: "CNC motorcycle instrument bracket deposit".to_string(),
            customer_email: " RIDER@EXAMPLE.COM ".to_string(),
            success_url: "https://fab.example/jobs/dpt_token/success?session_id={CHECKOUT_SESSION_ID}"
                .to_string(),
            cancel_url: "https://fab.example/jobs/dpt_token/cancel".to_string(),
            metadata: BTreeMap::from([
                ("application".to_string(), "daedalus-fab".to_string()),
                ("vehicle_kind".to_string(), "motorcycle".to_string()),
            ]),
        }
    }

    #[test]
    fn normalizes_checkout_intent_and_return_urls() {
        let intent = sample_request().normalize(&test_config()).unwrap();
        assert_eq!(intent.currency, "USD");
        assert_eq!(intent.customer_email, "rider@example.com");
        assert!(intent.success_url.contains("{CHECKOUT_SESSION_ID}"));
    }

    #[test]
    fn return_prefix_matching_has_a_path_boundary() {
        let prefix = AllowedReturnPrefix::parse("https://fab.example/jobs").unwrap();
        assert!(prefix.permits(&reqwest::Url::parse("https://fab.example/jobs/abc").unwrap()));
        assert!(!prefix.permits(&reqwest::Url::parse("https://fab.example/jobs-evil").unwrap()));
        assert!(!prefix.permits(&reqwest::Url::parse("https://evil.example/jobs/abc").unwrap()));
    }

    #[test]
    fn checkout_fingerprint_is_stable_and_binds_amount() {
        let cfg = test_config();
        let first = sample_request().normalize(&cfg).unwrap();
        let replay = sample_request().normalize(&cfg).unwrap();
        assert_eq!(
            checkout_intent_fingerprint(&first).unwrap(),
            checkout_intent_fingerprint(&replay).unwrap()
        );

        let mut changed = sample_request();
        changed.amount_minor += 1;
        let changed = changed.normalize(&cfg).unwrap();
        assert_ne!(
            checkout_intent_fingerprint(&first).unwrap(),
            checkout_intent_fingerprint(&changed).unwrap()
        );
    }

    #[test]
    fn metadata_reserves_quaestor_namespace() {
        let mut request = sample_request();
        request
            .metadata
            .insert("quaestor_tenant_id".to_string(), "spoofed".to_string());
        assert!(matches!(
            request.normalize(&test_config()),
            Err(CheckoutError::BadRequest(_))
        ));
    }

    #[test]
    fn bearer_and_idempotency_headers_fail_closed() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        );
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("daedalus:request-0001"),
        );
        assert!(require_checkout_bearer(&headers, &"b".repeat(32)).is_ok());
        assert!(require_checkout_bearer(&headers, &"c".repeat(32)).is_err());
        assert_eq!(
            required_idempotency_key(&headers).unwrap(),
            "daedalus:request-0001"
        );
        headers.insert("idempotency-key", HeaderValue::from_static("bad key"));
        assert!(required_idempotency_key(&headers).is_err());
    }

    #[test]
    fn stripe_form_contains_checkout_and_payment_intent_metadata() {
        let intent = sample_request().normalize(&test_config()).unwrap();
        let tenant_id = Uuid::new_v4();
        let checkout_id = Uuid::new_v4();
        let encoded = stripe_checkout_form(tenant_id, checkout_id, &intent).unwrap();
        let parameters: BTreeMap<String, String> = serde_urlencoded::from_str(&encoded).unwrap();
        assert_eq!(
            parameters
                .get("line_items[0][price_data][unit_amount]")
                .map(String::as_str),
            Some("12500")
        );
        assert_eq!(
            parameters
                .get("metadata[quaestor_checkout_id]")
                .map(String::as_str),
            Some(checkout_id.to_string().as_str())
        );
        assert_eq!(
            parameters
                .get("payment_intent_data[metadata][vehicle_kind]")
                .map(String::as_str),
            Some("motorcycle")
        );
    }

    #[test]
    fn stripe_response_validation_binds_money_reference_and_host() {
        let hosts = ["checkout.stripe.com".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();
        let session = StripeCheckoutSession {
            id: "cs_test_123456".to_string(),
            url: Some("https://checkout.stripe.com/c/pay/test".to_string()),
            status: Some("open".to_string()),
            payment_status: Some("unpaid".to_string()),
            amount_total: Some(12_500),
            currency: Some("usd".to_string()),
            client_reference_id: Some("job-1".to_string()),
        };
        assert!(normalize_stripe_session(
            session.clone(),
            12_500,
            "USD",
            "job-1",
            &hosts,
            true,
        )
        .is_ok());

        let mut bad_host = session.clone();
        bad_host.url = Some("https://evil.example/pay".to_string());
        assert!(normalize_stripe_session(
            bad_host, 12_500, "USD", "job-1", &hosts, true,
        )
        .is_err());
        assert!(normalize_stripe_session(
            session, 12_501, "USD", "job-1", &hosts, true,
        )
        .is_err());
    }
}
