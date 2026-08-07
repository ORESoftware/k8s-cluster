#[derive(Clone, Debug)]
struct AllowedReturnPrefix {
    scheme: String,
    host: String,
    port: Option<u16>,
    path_prefix: String,
}

impl AllowedReturnPrefix {
    fn parse(value: &str) -> anyhow::Result<Self> {
        let parsed = reqwest::Url::parse(value)
            .map_err(|error| anyhow::anyhow!("invalid checkout return URL prefix: {error}"))?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            anyhow::bail!(
                "checkout return URL prefixes must not contain userinfo, a query, or a fragment"
            );
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("checkout return URL prefix must contain a host"))?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        validate_https_or_loopback(&parsed)?;
        let path_prefix = normalize_path_prefix(parsed.path());
        Ok(Self {
            scheme: parsed.scheme().to_string(),
            host,
            port: parsed.port_or_known_default(),
            path_prefix,
        })
    }

    fn permits(&self, candidate: &reqwest::Url) -> bool {
        let Some(host) = candidate.host_str() else {
            return false;
        };
        self.scheme == candidate.scheme()
            && self.host == host.trim_end_matches('.').to_ascii_lowercase()
            && self.port == candidate.port_or_known_default()
            && path_is_within(candidate.path(), &self.path_prefix)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCheckoutRequest {
    client_reference_id: String,
    amount_minor: i64,
    currency: String,
    description: String,
    customer_email: String,
    success_url: String,
    cancel_url: String,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
struct NormalizedCheckoutIntent {
    client_reference_id: String,
    amount_minor: i64,
    currency: String,
    description: String,
    customer_email: String,
    success_url: String,
    cancel_url: String,
    metadata: BTreeMap<String, String>,
}

impl CreateCheckoutRequest {
    fn normalize(self, cfg: &CheckoutConfig) -> Result<NormalizedCheckoutIntent, CheckoutError> {
        if !(MIN_AMOUNT_MINOR..=MAX_AMOUNT_MINOR).contains(&self.amount_minor) {
            return Err(CheckoutError::BadRequest(format!(
                "amount_minor must be between {MIN_AMOUNT_MINOR} and {MAX_AMOUNT_MINOR}"
            )));
        }
        let currency = normalize_currency(&self.currency)?;
        let client_reference_id = bounded_text(
            "client_reference_id",
            &self.client_reference_id,
            1,
            200,
        )?;
        let description = bounded_text("description", &self.description, 1, 200)?;
        let customer_email = normalize_email(&self.customer_email)?;
        let success_url = validate_checkout_return_url(
            "success_url",
            &self.success_url,
            &cfg.return_url_prefixes,
        )?;
        let cancel_url = validate_checkout_return_url(
            "cancel_url",
            &self.cancel_url,
            &cfg.return_url_prefixes,
        )?;
        let metadata = normalize_metadata(self.metadata)?;

        Ok(NormalizedCheckoutIntent {
            client_reference_id,
            amount_minor: self.amount_minor,
            currency,
            description,
            customer_email,
            success_url,
            cancel_url,
            metadata,
        })
    }
}

#[derive(Clone, Debug)]
struct StripeConnection {
    id: Uuid,
    shard_key: i64,
    account_id: String,
    checkout_default: bool,
}

#[derive(Clone, Debug)]
struct StoredCheckout {
    id: Uuid,
    tenant_id: Uuid,
    shard_key: i64,
    provider_connection_id: Uuid,
    idempotency_key_hash: String,
    intent_fingerprint: String,
    client_reference_id: String,
    amount_minor: i64,
    currency: String,
    description: String,
    metadata: JsonValue,
    provider_session_id: Option<String>,
    checkout_url: Option<String>,
    session_status: String,
    payment_status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl StoredCheckout {
    fn view(&self) -> Result<CheckoutSessionView, CheckoutError> {
        let id = self.provider_session_id.clone().ok_or_else(|| {
            CheckoutError::Internal(format!(
                "checkout intent {} has no provider session",
                self.id
            ))
        })?;
        Ok(CheckoutSessionView {
            id,
            url: self.checkout_url.clone(),
            status: self.session_status.clone(),
            payment_status: self.payment_status.clone(),
            amount_minor: self.amount_minor,
            currency: self.currency.clone(),
            client_reference_id: self.client_reference_id.clone(),
            provider_connection_id: self.provider_connection_id,
            quaestor_id: self.id,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
struct CheckoutSessionView {
    id: String,
    url: Option<String>,
    status: String,
    payment_status: String,
    amount_minor: i64,
    currency: String,
    client_reference_id: String,
    provider_connection_id: Uuid,
    quaestor_id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
struct StripeCheckoutSession {
    id: String,
    url: Option<String>,
    status: Option<String>,
    payment_status: Option<String>,
    amount_total: Option<i64>,
    currency: Option<String>,
    client_reference_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StripeErrorEnvelope {
    error: StripeErrorObject,
}

#[derive(Debug, Deserialize)]
struct StripeErrorObject {
    #[serde(rename = "type")]
    kind: Option<String>,
    code: Option<String>,
    param: Option<String>,
    message: Option<String>,
}

#[derive(Clone, Debug)]
struct NormalizedStripeSession {
    id: String,
    url: Option<String>,
    status: String,
    payment_status: String,
    amount_minor: i64,
    currency: String,
    client_reference_id: String,
}

fn normalize_currency(value: &str) -> Result<String, CheckoutError> {
    let value = value.trim().to_ascii_uppercase();
    if value.len() != 3 || !value.chars().all(|character| character.is_ascii_alphabetic()) {
        return Err(CheckoutError::BadRequest(
            "currency must be a three-letter alphabetic code".to_string(),
        ));
    }
    Ok(value)
}

fn bounded_text(
    field: &str,
    value: &str,
    min_bytes: usize,
    max_bytes: usize,
) -> Result<String, CheckoutError> {
    let value = value.trim();
    if !(min_bytes..=max_bytes).contains(&value.len())
        || value.chars().any(char::is_control)
    {
        return Err(CheckoutError::BadRequest(format!(
            "{field} must contain {min_bytes}..={max_bytes} bytes and no control characters"
        )));
    }
    Ok(value.to_string())
}

fn normalize_email(value: &str) -> Result<String, CheckoutError> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() < 3
        || value.len() > 320
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(CheckoutError::BadRequest(
            "customer_email must be a valid email address".to_string(),
        ));
    }
    let Some((local, domain)) = value.split_once('@') else {
        return Err(CheckoutError::BadRequest(
            "customer_email must contain @".to_string(),
        ));
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err(CheckoutError::BadRequest(
            "customer_email must contain a local part and domain".to_string(),
        ));
    }
    Ok(value)
}

fn normalize_metadata(
    metadata: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, CheckoutError> {
    if metadata.len() > 20 {
        return Err(CheckoutError::BadRequest(
            "metadata may contain at most 20 entries".to_string(),
        ));
    }
    let mut normalized = BTreeMap::new();
    let mut total_bytes = 0_usize;
    for (key, value) in metadata {
        let key = key.trim().to_string();
        if key.is_empty()
            || key.len() > 40
            || key.starts_with("quaestor_")
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err(CheckoutError::BadRequest(
                "metadata keys must contain 1..=40 ASCII letters, digits, underscores, or hyphens and must not start with quaestor_"
                    .to_string(),
            ));
        }
        let value = value.trim().to_string();
        if value.len() > 500 || value.chars().any(char::is_control) {
            return Err(CheckoutError::BadRequest(format!(
                "metadata value {key:?} must contain at most 500 bytes and no control characters"
            )));
        }
        total_bytes = total_bytes.saturating_add(key.len()).saturating_add(value.len());
        normalized.insert(key, value);
    }
    if total_bytes > 16 * 1024 {
        return Err(CheckoutError::BadRequest(
            "metadata must contain at most 16384 bytes".to_string(),
        ));
    }
    Ok(normalized)
}

fn validate_checkout_return_url(
    field: &str,
    value: &str,
    prefixes: &[AllowedReturnPrefix],
) -> Result<String, CheckoutError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2_048 || value.chars().any(char::is_control) {
        return Err(CheckoutError::BadRequest(format!(
            "{field} must contain 1..=2048 bytes and no control characters"
        )));
    }
    let rendered = value.replace("{CHECKOUT_SESSION_ID}", "cs_test_validation");
    if rendered.contains('{') || rendered.contains('}') {
        return Err(CheckoutError::BadRequest(format!(
            "{field} contains an unsupported template placeholder"
        )));
    }
    let parsed = reqwest::Url::parse(&rendered)
        .map_err(|_| CheckoutError::BadRequest(format!("{field} must be an absolute URL")))?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(CheckoutError::BadRequest(format!(
            "{field} must not contain userinfo or a fragment"
        )));
    }
    validate_https_or_loopback(&parsed)
        .map_err(|error| CheckoutError::BadRequest(format!("{field}: {error}")))?;
    if !prefixes.iter().any(|prefix| prefix.permits(&parsed)) {
        return Err(CheckoutError::BadRequest(format!(
            "{field} is not under an approved return URL prefix"
        )));
    }
    Ok(value.to_string())
}

fn validate_https_or_loopback(url: &reqwest::Url) -> anyhow::Result<()> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL must contain a host"))?;
    let secure = url.scheme() == "https";
    let loopback = url.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !secure && !loopback {
        anyhow::bail!("URL must use HTTPS outside loopback development");
    }
    Ok(())
}

fn normalize_path_prefix(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    if path == "/" {
        "/".to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

fn path_is_within(candidate: &str, prefix: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    candidate == prefix
        || candidate
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn require_checkout_bearer(
    headers: &HeaderMap,
    expected: &str,
) -> Result<(), CheckoutError> {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(CheckoutError::Unauthorized)?;
    if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        return Err(CheckoutError::Unauthorized);
    }
    Ok(())
}

fn require_allowed_tenant(cfg: &CheckoutConfig, tenant_id: Uuid) -> Result<(), CheckoutError> {
    if !cfg.allowed_tenants.contains(&tenant_id) {
        return Err(CheckoutError::NotFound);
    }
    Ok(())
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<String, CheckoutError> {
    let value = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| CheckoutError::BadRequest("Idempotency-Key is required".to_string()))?
        .trim();
    if !(8..=200).contains(&value.len())
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '/')
        })
    {
        return Err(CheckoutError::BadRequest(
            "Idempotency-Key must contain 8..=200 URL-safe ASCII characters".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn checkout_intent_fingerprint(
    intent: &NormalizedCheckoutIntent,
) -> Result<String, CheckoutError> {
    let bytes = serde_json::to_vec(intent)?;
    Ok(sha256_prefixed(
        b"quaestor:checkout-intent:v1\0",
        &bytes,
    ))
}

fn idempotency_key_hash(value: &str) -> String {
    sha256_prefixed(b"quaestor:checkout-idempotency-key:v1\0", value.as_bytes())
}

fn customer_email_hash(value: &str) -> String {
    sha256_prefixed(b"quaestor:checkout-customer-email:v1\0", value.as_bytes())
}

fn sha256_prefixed(domain: &[u8], input: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(input);
    format!("sha256:v1:{}", hex::encode(digest.finalize()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn valid_stripe_session_id(value: &str) -> bool {
    value.len() >= 8
        && value.len() <= 255
        && value.starts_with("cs_")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn valid_stripe_account_id(value: &str) -> bool {
    value.len() >= 8
        && value.len() <= 255
        && value.starts_with("acct_")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_string()
}
