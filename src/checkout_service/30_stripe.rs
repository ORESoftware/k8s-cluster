async fn create_stripe_checkout(
    state: &CheckoutApiState,
    tenant_id: Uuid,
    connection: &StripeConnection,
    checkout_id: Uuid,
    idempotency_hash: &str,
    intent: &NormalizedCheckoutIntent,
) -> Result<NormalizedStripeSession, CheckoutError> {
    let form = stripe_checkout_form(tenant_id, checkout_id, intent)?;
    let provider_idempotency_key = format!(
        "quaestor:{tenant_id}:{}",
        idempotency_hash
            .strip_prefix("sha256:v1:")
            .unwrap_or(idempotency_hash)
    );
    let endpoint = format!("{}/v1/checkout/sessions", state.cfg.stripe_api_base);
    let response = state
        .http
        .post(endpoint)
        .bearer_auth(&state.cfg.stripe_api_key)
        .header("Stripe-Version", &state.cfg.stripe_api_version)
        .header("Stripe-Account", &connection.account_id)
        .header("Idempotency-Key", provider_idempotency_key)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form)
        .send()
        .await
        .map_err(|error| {
            CheckoutError::ProviderUnavailable(format!("Stripe request transport error: {error}"))
        })?;
    let session = decode_stripe_response(response, "create checkout session").await?;
    normalize_stripe_session(
        session,
        intent.amount_minor,
        &intent.currency,
        &intent.client_reference_id,
        &state.cfg.checkout_url_hosts,
        true,
    )
}

async fn retrieve_stripe_checkout(
    state: &CheckoutApiState,
    connection: &StripeConnection,
    stored: &StoredCheckout,
) -> Result<NormalizedStripeSession, CheckoutError> {
    let session_id = stored
        .provider_session_id
        .as_deref()
        .filter(|value| valid_stripe_session_id(value))
        .ok_or(CheckoutError::NotFound)?;
    let endpoint = format!(
        "{}/v1/checkout/sessions/{session_id}",
        state.cfg.stripe_api_base
    );
    let response = state
        .http
        .get(endpoint)
        .bearer_auth(&state.cfg.stripe_api_key)
        .header("Stripe-Version", &state.cfg.stripe_api_version)
        .header("Stripe-Account", &connection.account_id)
        .send()
        .await
        .map_err(|error| {
            CheckoutError::ProviderUnavailable(format!("Stripe request transport error: {error}"))
        })?;
    let session = decode_stripe_response(response, "retrieve checkout session").await?;
    normalize_stripe_session(
        session,
        stored.amount_minor,
        &stored.currency,
        &stored.client_reference_id,
        &state.cfg.checkout_url_hosts,
        false,
    )
}

fn stripe_checkout_form(
    tenant_id: Uuid,
    checkout_id: Uuid,
    intent: &NormalizedCheckoutIntent,
) -> Result<String, CheckoutError> {
    let mut parameters = vec![
        ("mode".to_string(), "payment".to_string()),
        ("success_url".to_string(), intent.success_url.clone()),
        ("cancel_url".to_string(), intent.cancel_url.clone()),
        (
            "client_reference_id".to_string(),
            intent.client_reference_id.clone(),
        ),
        ("customer_email".to_string(), intent.customer_email.clone()),
        ("line_items[0][quantity]".to_string(), "1".to_string()),
        (
            "line_items[0][price_data][currency]".to_string(),
            intent.currency.to_ascii_lowercase(),
        ),
        (
            "line_items[0][price_data][unit_amount]".to_string(),
            intent.amount_minor.to_string(),
        ),
        (
            "line_items[0][price_data][product_data][name]".to_string(),
            intent.description.clone(),
        ),
    ];

    let internal_metadata = [
        ("quaestor_checkout_id", checkout_id.to_string()),
        ("quaestor_tenant_id", tenant_id.to_string()),
        (
            "quaestor_client_reference_id",
            intent.client_reference_id.clone(),
        ),
    ];
    for (key, value) in internal_metadata {
        parameters.push((format!("metadata[{key}]"), value.clone()));
        parameters.push((format!("payment_intent_data[metadata][{key}]"), value));
    }
    for (key, value) in &intent.metadata {
        parameters.push((format!("metadata[{key}]"), value.clone()));
        parameters.push((
            format!("payment_intent_data[metadata][{key}]"),
            value.clone(),
        ));
    }

    serde_urlencoded::to_string(parameters)
        .map_err(|error| CheckoutError::Internal(format!("encode Stripe form: {error}")))
}

async fn decode_stripe_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<StripeCheckoutSession, CheckoutError> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    if status.is_success() {
        return response.json::<StripeCheckoutSession>().await.map_err(|error| {
            CheckoutError::ProviderUnavailable(format!(
                "Stripe returned an invalid success response for {operation}: {error}"
            ))
        });
    }

    let provider_error = response.json::<StripeErrorEnvelope>().await.ok();
    let kind = provider_error
        .as_ref()
        .and_then(|envelope| envelope.error.kind.as_deref())
        .unwrap_or("unknown");
    let code = provider_error
        .as_ref()
        .and_then(|envelope| envelope.error.code.as_deref())
        .unwrap_or("unknown");
    let param = provider_error
        .as_ref()
        .and_then(|envelope| envelope.error.param.as_deref())
        .unwrap_or("unknown");
    let message = provider_error
        .as_ref()
        .and_then(|envelope| envelope.error.message.as_deref())
        .map(|value| truncate_utf8_bytes(value, 300))
        .unwrap_or_else(|| "unparseable provider error".to_string());
    tracing::warn!(
        %status,
        %request_id,
        provider_error_type = %kind,
        provider_error_code = %code,
        provider_error_param = %param,
        provider_error_message = %message,
        %operation,
        "Stripe rejected hosted checkout operation"
    );
    Err(CheckoutError::ProviderUnavailable(format!(
        "Stripe returned {status} for {operation}; request_id={request_id}"
    )))
}

fn normalize_stripe_session(
    session: StripeCheckoutSession,
    expected_amount_minor: i64,
    expected_currency: &str,
    expected_reference: &str,
    allowed_checkout_hosts: &HashSet<String>,
    checkout_url_required: bool,
) -> Result<NormalizedStripeSession, CheckoutError> {
    if !valid_stripe_session_id(&session.id) {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe returned an invalid checkout session id".to_string(),
        ));
    }
    let status = session.status.ok_or_else(|| {
        CheckoutError::ProviderUnavailable("Stripe returned no checkout status".to_string())
    })?;
    if !matches!(status.as_str(), "open" | "complete" | "expired") {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe returned an unknown checkout status".to_string(),
        ));
    }
    let payment_status = session.payment_status.ok_or_else(|| {
        CheckoutError::ProviderUnavailable("Stripe returned no payment status".to_string())
    })?;
    if !matches!(
        payment_status.as_str(),
        "paid" | "unpaid" | "no_payment_required"
    ) {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe returned an unknown payment status".to_string(),
        ));
    }
    let amount_minor = session.amount_total.ok_or_else(|| {
        CheckoutError::ProviderUnavailable("Stripe returned no checkout amount".to_string())
    })?;
    if amount_minor != expected_amount_minor {
        return Err(CheckoutError::ProviderUnavailable(format!(
            "Stripe checkout amount mismatch: expected {expected_amount_minor}, got {amount_minor}"
        )));
    }
    let currency = session
        .currency
        .ok_or_else(|| {
            CheckoutError::ProviderUnavailable("Stripe returned no checkout currency".to_string())
        })?
        .to_ascii_uppercase();
    if currency != expected_currency {
        return Err(CheckoutError::ProviderUnavailable(format!(
            "Stripe checkout currency mismatch: expected {expected_currency}, got {currency}"
        )));
    }
    let client_reference_id = session.client_reference_id.ok_or_else(|| {
        CheckoutError::ProviderUnavailable(
            "Stripe returned no checkout client_reference_id".to_string(),
        )
    })?;
    if client_reference_id != expected_reference {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe checkout client_reference_id mismatch".to_string(),
        ));
    }

    let url = match session.url {
        Some(value) => {
            validate_hosted_checkout_url(&value, allowed_checkout_hosts)?;
            Some(value)
        }
        None if checkout_url_required => {
            return Err(CheckoutError::ProviderUnavailable(
                "Stripe returned no hosted checkout URL".to_string(),
            ));
        }
        None => None,
    };

    Ok(NormalizedStripeSession {
        id: session.id,
        url,
        status,
        payment_status,
        amount_minor,
        currency,
        client_reference_id,
    })
}

fn validate_hosted_checkout_url(
    value: &str,
    allowed_checkout_hosts: &HashSet<String>,
) -> Result<(), CheckoutError> {
    let parsed = reqwest::Url::parse(value).map_err(|_| {
        CheckoutError::ProviderUnavailable("Stripe returned an invalid checkout URL".to_string())
    })?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CheckoutError::ProviderUnavailable(
            "Stripe returned an unsafe checkout URL".to_string(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| {
            CheckoutError::ProviderUnavailable(
                "Stripe returned a checkout URL without a host".to_string(),
            )
        })?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !allowed_checkout_hosts.contains(&host) {
        return Err(CheckoutError::ProviderUnavailable(format!(
            "Stripe returned a checkout URL on an unapproved host: {host}"
        )));
    }
    Ok(())
}
