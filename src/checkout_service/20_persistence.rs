async fn select_checkout_connection(
    db: &DatabaseConnection,
    tenant_id: Uuid,
) -> Result<StripeConnection, CheckoutError> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT id,
                   shard_key,
                   external_account_id,
                   (metadata->>'checkout_default' = 'true') AS checkout_default
            FROM provider_connections
            WHERE tenant_id = $1
              AND provider::text = 'stripe'
              AND status::text = 'active'
              AND external_account_id IS NOT NULL
            ORDER BY updated_at DESC, id
            LIMIT 20
            "#,
            [tenant_id.into()],
        ))
        .await?;

    let mut connections = rows
        .into_iter()
        .map(stripe_connection_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    connections.retain(|connection| valid_stripe_account_id(&connection.account_id));

    let mut defaults = connections
        .iter()
        .filter(|connection| connection.checkout_default)
        .cloned()
        .collect::<Vec<_>>();
    if defaults.len() > 1 {
        return Err(CheckoutError::Conflict(
            "tenant has multiple Stripe connections marked checkout_default".to_string(),
        ));
    }
    if let Some(connection) = defaults.pop() {
        return Ok(connection);
    }
    match connections.len() {
        0 => Err(CheckoutError::Conflict(
            "tenant has no active Stripe Connect account for checkout".to_string(),
        )),
        1 => Ok(connections.remove(0)),
        _ => Err(CheckoutError::Conflict(
            "tenant has multiple active Stripe connections; mark exactly one metadata.checkout_default=true"
                .to_string(),
        )),
    }
}

async fn load_checkout_connection(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    connection_id: Uuid,
) -> Result<StripeConnection, CheckoutError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT id,
                   shard_key,
                   external_account_id,
                   (metadata->>'checkout_default' = 'true') AS checkout_default
            FROM provider_connections
            WHERE tenant_id = $1
              AND id = $2
              AND provider::text = 'stripe'
              AND status::text = 'active'
              AND external_account_id IS NOT NULL
            "#,
            [tenant_id.into(), connection_id.into()],
        ))
        .await?
        .ok_or_else(|| {
            CheckoutError::ProviderUnavailable(
                "the Stripe connection used by this checkout is no longer active".to_string(),
            )
        })?;
    let connection = stripe_connection_from_row(row)?;
    if !valid_stripe_account_id(&connection.account_id) {
        return Err(CheckoutError::ProviderUnavailable(
            "the active Stripe connection has an invalid account identifier".to_string(),
        ));
    }
    Ok(connection)
}

fn stripe_connection_from_row(row: QueryResult) -> Result<StripeConnection, CheckoutError> {
    Ok(StripeConnection {
        id: row_get(&row, "id")?,
        shard_key: row_get(&row, "shard_key")?,
        account_id: row_get(&row, "external_account_id")?,
        checkout_default: row_get(&row, "checkout_default")?,
    })
}

async fn insert_checkout_intent(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    connection: &StripeConnection,
    idempotency_hash: &str,
    fingerprint: &str,
    intent: &NormalizedCheckoutIntent,
) -> Result<bool, CheckoutError> {
    let id = Uuid::new_v4();
    let metadata = serde_json::to_value(&intent.metadata)?;
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            INSERT INTO checkout_sessions (
                id,
                tenant_id,
                shard_key,
                provider_connection_id,
                idempotency_key_hash,
                intent_fingerprint,
                client_reference_id,
                customer_email_hash,
                amount_minor,
                currency,
                description,
                metadata
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9::numeric, $10, $11, $12
            )
            ON CONFLICT (tenant_id, idempotency_key_hash) DO NOTHING
            "#,
            [
                id.into(),
                tenant_id.into(),
                connection.shard_key.into(),
                connection.id.into(),
                idempotency_hash.to_string().into(),
                fingerprint.to_string().into(),
                intent.client_reference_id.clone().into(),
                customer_email_hash(&intent.customer_email).into(),
                intent.amount_minor.into(),
                intent.currency.clone().into(),
                intent.description.clone().into(),
                metadata.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected() == 1)
}

async fn load_checkout_by_idempotency(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    idempotency_hash: &str,
) -> Result<StoredCheckout, CheckoutError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            checkout_select_sql("tenant_id = $1 AND idempotency_key_hash = $2"),
            [tenant_id.into(), idempotency_hash.to_string().into()],
        ))
        .await?
        .ok_or(CheckoutError::NotFound)?;
    stored_checkout_from_row(row)
}

async fn load_checkout_by_provider_session(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    provider_session_id: &str,
) -> Result<StoredCheckout, CheckoutError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            checkout_select_sql("tenant_id = $1 AND provider_session_id = $2"),
            [tenant_id.into(), provider_session_id.to_string().into()],
        ))
        .await?
        .ok_or(CheckoutError::NotFound)?;
    stored_checkout_from_row(row)
}

async fn load_checkout_by_id(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    checkout_id: Uuid,
) -> Result<StoredCheckout, CheckoutError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            checkout_select_sql("tenant_id = $1 AND id = $2"),
            [tenant_id.into(), checkout_id.into()],
        ))
        .await?
        .ok_or(CheckoutError::NotFound)?;
    stored_checkout_from_row(row)
}

fn checkout_select_sql(predicate: &str) -> String {
    format!(
        r#"
        SELECT id,
               tenant_id,
               shard_key,
               provider_connection_id,
               idempotency_key_hash,
               intent_fingerprint,
               client_reference_id,
               amount_minor::text AS amount_minor,
               currency,
               description,
               metadata,
               provider_session_id,
               checkout_url,
               session_status,
               payment_status,
               created_at,
               updated_at
        FROM checkout_sessions
        WHERE {predicate}
        "#
    )
}

fn stored_checkout_from_row(row: QueryResult) -> Result<StoredCheckout, CheckoutError> {
    let amount_minor_text: String = row_get(&row, "amount_minor")?;
    let amount_minor = amount_minor_text.parse::<i64>().map_err(|error| {
        CheckoutError::Internal(format!("decode checkout amount_minor: {error}"))
    })?;
    let currency: String = row_get(&row, "currency")?;
    Ok(StoredCheckout {
        id: row_get(&row, "id")?,
        tenant_id: row_get(&row, "tenant_id")?,
        shard_key: row_get(&row, "shard_key")?,
        provider_connection_id: row_get(&row, "provider_connection_id")?,
        idempotency_key_hash: row_get(&row, "idempotency_key_hash")?,
        intent_fingerprint: row_get(&row, "intent_fingerprint")?,
        client_reference_id: row_get(&row, "client_reference_id")?,
        amount_minor,
        currency: currency.trim().to_ascii_uppercase(),
        description: row_get(&row, "description")?,
        metadata: row_get(&row, "metadata")?,
        provider_session_id: row_get(&row, "provider_session_id")?,
        checkout_url: row_get(&row, "checkout_url")?,
        session_status: row_get(&row, "session_status")?,
        payment_status: row_get(&row, "payment_status")?,
        created_at: row_get(&row, "created_at")?,
        updated_at: row_get(&row, "updated_at")?,
    })
}

async fn persist_stripe_session(
    db: &DatabaseConnection,
    tenant_id: Uuid,
    checkout_id: Uuid,
    session: &NormalizedStripeSession,
) -> Result<StoredCheckout, CheckoutError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            UPDATE checkout_sessions
            SET provider_session_id = $3,
                checkout_url = COALESCE($4, checkout_url),
                session_status = $5,
                payment_status = $6,
                updated_at = now()
            WHERE tenant_id = $1
              AND id = $2
            "#,
            [
                tenant_id.into(),
                checkout_id.into(),
                session.id.clone().into(),
                session.url.clone().into(),
                session.status.clone().into(),
                session.payment_status.clone().into(),
            ],
        ))
        .await?;
    if result.rows_affected() != 1 {
        return Err(CheckoutError::NotFound);
    }
    load_checkout_by_id(db, tenant_id, checkout_id).await
}

fn row_get<T>(row: &QueryResult, column: &str) -> Result<T, CheckoutError>
where
    T: TryGetable,
{
    row.try_get("", column).map_err(|error| {
        CheckoutError::Internal(format!("decode database column {column}: {error}"))
    })
}
