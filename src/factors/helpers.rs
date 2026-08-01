impl FactorService {
    async fn verified_phone(&self, user_id: Uuid) -> Result<String, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT phone FROM shared_auth.principals \
                 WHERE shared_user_id = $1 AND status = 'active' AND phone_verified = true",
                vec![user_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::BadRequest("verified phone is required"))?;
        let phone: Option<String> = row.try_get("", "phone").map_err(db_error)?;
        phone.ok_or(AuthError::BadRequest("verified phone is required"))
    }

    async fn cancel_challenge(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        challenge_id: Uuid,
    ) -> Result<(), AuthError> {
        self.db
            .execute_raw(statement(
                "UPDATE shared_auth.auth_challenges \
                 SET consumed_at = coalesce(consumed_at, now()) \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3",
                vec![challenge_id.into(), user_id.into(), session_id.into()],
            ))
            .await
            .map_err(db_error)?;
        Ok(())
    }
}

async fn claims(state: &AppState, headers: &HeaderMap) -> Result<OreClaims, AuthError> {
    active_claims(state, bearer(headers).ok_or(AuthError::Unauthorized)?).await
}

fn step_up(state: &AppState, claims: &OreClaims, method: &str) -> Result<StepUpResponse, AuthError> {
    let minted = session_tokens::mint_step_up(state, claims, method)?;
    Ok(StepUpResponse {
        access_token: minted.token,
        token_type: "Bearer",
        expires_at: minted.expires_at,
        amr: minted.amr,
        acr: minted.acr,
    })
}

fn email_otp_is_enabled(state: &AppState) -> bool {
    state.config.magic_links.sendgrid_api_key.is_some()
        && state.config.magic_links.otp_pepper.is_some()
        && state.config.magic_links.from_email.is_some()
}

fn sms_otp_is_enabled(state: &AppState) -> bool {
    state.config.twilio_verify.is_enabled() && state.config.magic_links.otp_pepper.is_some()
}

async fn send_email_otp(
    state: &AppState,
    recipient: &str,
    code: &str,
) -> Result<(), AuthError> {
    if !email_otp_is_enabled(state) {
        return Err(AuthError::Unavailable);
    }
    let config = &state.config.magic_links;
    let api_key = config
        .sendgrid_api_key
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let from_email = config.from_email.as_deref().ok_or(AuthError::Unavailable)?;
    let payload = json!({
        "personalizations": [{"to": [{"email": recipient}]}],
        "from": {"email": from_email, "name": config.from_name},
        "subject": "Your verification code",
        "content": [
            {"type": "text/plain", "value": format!("Your one-time verification code is {code}. It expires in {OTP_TTL_MINUTES} minutes.")},
            {"type": "text/html", "value": format!("<p>Your one-time verification code is <strong>{code}</strong>.</p><p>It expires in {OTP_TTL_MINUTES} minutes.</p>")}
        ]
    });
    let response = state
        .http
        .post("https://api.sendgrid.com/v3/mail/send")
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "SendGrid OTP request failed");
            AuthError::Upstream
        })?;
    if response.status() == reqwest::StatusCode::ACCEPTED {
        Ok(())
    } else {
        tracing::warn!(status = response.status().as_u16(), "SendGrid rejected OTP email");
        Err(AuthError::Upstream)
    }
}

fn factor_from_row(row: &sea_orm::QueryResult) -> Result<Factor, AuthError> {
    let factor_id: Uuid = row.try_get("", "factor_id").map_err(db_error)?;
    let kind: String = row.try_get("", "kind").map_err(db_error)?;
    let label: Option<String> = row.try_get("", "label").map_err(db_error)?;
    let enabled: bool = row.try_get("", "enabled").map_err(db_error)?;
    let confirmed_at: Option<DateTime<FixedOffset>> =
        row.try_get("", "confirmed_at").map_err(db_error)?;
    let last_used_at: Option<DateTime<FixedOffset>> =
        row.try_get("", "last_used_at").map_err(db_error)?;
    let created_at: DateTime<FixedOffset> = row.try_get("", "created_at").map_err(db_error)?;
    Ok(Factor {
        factor_id: factor_id.to_string(),
        kind,
        label,
        enabled,
        confirmed_at: confirmed_at.map(|value| value.to_rfc3339()),
        last_used_at: last_used_at.map(|value| value.to_rfc3339()),
        created_at: created_at.to_rfc3339(),
    })
}

fn claim_user_id(claims: &OreClaims) -> Result<Uuid, AuthError> {
    Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Unauthorized)
}

fn claim_session_id(claims: &OreClaims) -> Result<Uuid, AuthError> {
    claims
        .sid
        .as_deref()
        .ok_or(AuthError::Unauthorized)
        .and_then(|value| Uuid::parse_str(value).map_err(|_| AuthError::Unauthorized))
}

fn parse_uuid(value: &str, message: &'static str) -> Result<Uuid, AuthError> {
    Uuid::parse_str(value).map_err(|_| AuthError::BadRequest(message))
}

fn credential_id(credential: &Value) -> Result<String, AuthError> {
    credential
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 2048)
        .map(str::to_owned)
        .ok_or(AuthError::BadRequest("invalid passkey credential id"))
}

fn normalize_label(label: Option<&str>) -> Result<Option<String>, AuthError> {
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if label.as_ref().is_some_and(|value| value.len() > 160) {
        Err(AuthError::BadRequest("factor label is too long"))
    } else {
        Ok(label)
    }
}

fn validate_otp(code: &str) -> Result<(), AuthError> {
    if code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(AuthError::BadRequest(
            "verification code must contain six digits",
        ))
    }
}

fn generate_code() -> Result<String, AuthError> {
    let unbiased_zone = u32::MAX - (u32::MAX % TOTP_DIGITS);
    loop {
        let mut bytes = [0u8; 4];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| AuthError::Internal)?;
        let value = u32::from_be_bytes(bytes);
        if value < unbiased_zone {
            return Ok(format!("{:06}", value % TOTP_DIGITS));
        }
    }
}

fn totp_code(secret: &[u8], counter: u64) -> String {
    let mut mac = <Hmac<Sha1> as HmacKeyInit>::new_from_slice(secret)
        .expect("HMAC accepts arbitrary TOTP secret lengths");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    format!("{:06}", binary % TOTP_DIGITS)
}

fn otp_tag(key: &[u8], challenge_id: Uuid, code: &str) -> Result<Vec<u8>, AuthError> {
    let mut mac = <Hmac<Sha256> as HmacKeyInit>::new_from_slice(key)
        .map_err(|_| AuthError::Internal)?;
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn otp_tag_matches(key: &[u8], challenge_id: Uuid, code: &str, expected: &[u8]) -> bool {
    let Ok(mut mac) = <Hmac<Sha256> as HmacKeyInit>::new_from_slice(key) else {
        return false;
    };
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    mac.verify_slice(expected).is_ok()
}

fn constant_time_code_eq(expected: &str, presented: &str, key: &[u8]) -> bool {
    let Ok(mut expected_mac) = <Hmac<Sha256> as HmacKeyInit>::new_from_slice(key) else {
        return false;
    };
    expected_mac.update(expected.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();

    let Ok(mut presented_mac) = <Hmac<Sha256> as HmacKeyInit>::new_from_slice(key) else {
        return false;
    };
    presented_mac.update(presented.as_bytes());
    presented_mac.verify_slice(&expected_tag).is_ok()
}

fn totp_aad(user_id: Uuid, factor_id: Uuid) -> Vec<u8> {
    format!(
        "shared-auth:totp:v{TOTP_ENCRYPTION_VERSION}:{user_id}:{factor_id}"
    )
    .into_bytes()
}

fn encrypt_totp_secret(
    key: &[u8; 32],
    user_id: Uuid,
    factor_id: Uuid,
    nonce: [u8; 12],
    secret: &[u8],
) -> Result<Vec<u8>, AuthError> {
    let cipher = <Aes256Gcm as AeadKeyInit>::new_from_slice(key)
        .map_err(|_| AuthError::Internal)?;
    let nonce = Nonce::from(nonce);
    let aad = totp_aad(user_id, factor_id);
    cipher
        .encrypt(
            &nonce,
            Payload {
                msg: secret,
                aad: &aad,
            },
        )
        .map_err(|_| AuthError::Internal)
}

fn decrypt_totp_secret(
    key: &[u8; 32],
    user_id: Uuid,
    factor_id: Uuid,
    nonce: [u8; 12],
    ciphertext: &[u8],
) -> Result<Vec<u8>, AuthError> {
    let cipher = <Aes256Gcm as AeadKeyInit>::new_from_slice(key)
        .map_err(|_| AuthError::Internal)?;
    let nonce = Nonce::from(nonce);
    let aad = totp_aad(user_id, factor_id);
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| AuthError::Internal)
}

fn encode_base32(input: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in input {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    output
}

fn percent_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn mask_destination(value: &str) -> String {
    if let Some((local, domain)) = value.split_once('@') {
        let prefix = local.chars().next().unwrap_or('•');
        return format!("{prefix}•••@{domain}");
    }
    let suffix = value.chars().rev().take(4).collect::<Vec<_>>();
    format!("••••{}", suffix.into_iter().rev().collect::<String>())
}

fn optional_hex_key(name: &'static str) -> anyhow::Result<Option<[u8; 32]>> {
    let Some(raw) = std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("{name} must contain exactly 64 hexadecimal characters");
    }
    let mut key = [0u8; 32];
    for (index, pair) in raw.as_bytes().chunks_exact(2).enumerate() {
        key[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(Some(key))
}

fn hex_nibble(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => anyhow::bail!("invalid hexadecimal digit"),
    }
}

fn validate_webauthn_config(rp_id: &str, origin: &Url, rp_name: &str) -> anyhow::Result<()> {
    let rp_id = rp_id.trim().trim_end_matches('.').to_ascii_lowercase();
    let rp_name = rp_name.trim();
    if rp_id.is_empty() || rp_id.len() > 253 {
        anyhow::bail!("AUTH_WEBAUTHN_RP_ID must be a valid DNS name or IP address");
    }
    if rp_name.is_empty() || rp_name.len() > 128 {
        anyhow::bail!("AUTH_WEBAUTHN_RP_NAME must contain between 1 and 128 characters");
    }
    if origin.username() != ""
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !matches!(origin.path(), "" | "/")
    {
        anyhow::bail!(
            "AUTH_WEBAUTHN_RP_ORIGIN must be an origin without credentials, path, query, or fragment"
        );
    }
    let host = origin
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("AUTH_WEBAUTHN_RP_ORIGIN must contain a host"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");
    if origin.scheme() != "https" && !(origin.scheme() == "http" && loopback) {
        anyhow::bail!("AUTH_WEBAUTHN_RP_ORIGIN must use HTTPS or loopback HTTP");
    }
    let valid_rp = host == rp_id
        || (rp_id.parse::<std::net::IpAddr>().is_err()
            && host
                .strip_suffix(&rp_id)
                .is_some_and(|prefix| prefix.ends_with('.')));
    if !valid_rp {
        anyhow::bail!("AUTH_WEBAUTHN_RP_ID must equal or be a registrable suffix of the origin host");
    }
    Ok(())
}

fn build_webauthn() -> anyhow::Result<Option<Arc<Webauthn>>> {
    let rp_id = optional_env("AUTH_WEBAUTHN_RP_ID");
    let origin = optional_env("AUTH_WEBAUTHN_RP_ORIGIN");
    let rp_name = optional_env("AUTH_WEBAUTHN_RP_NAME");
    match (rp_id, origin, rp_name) {
        (None, None, None) => Ok(None),
        (Some(rp_id), Some(origin), Some(rp_name)) => {
            let origin = Url::parse(&origin)?;
            validate_webauthn_config(&rp_id, &origin, &rp_name)?;
            let builder = WebauthnBuilder::new(&rp_id, &origin)?.rp_name(&rp_name);
            Ok(Some(Arc::new(builder.build()?)))
        }
        _ => anyhow::bail!(
            "AUTH_WEBAUTHN_RP_ID, AUTH_WEBAUTHN_RP_ORIGIN, and AUTH_WEBAUTHN_RP_NAME must be set together"
        ),
    }
}

fn optional_env(name: &'static str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn db_error(error: impl std::fmt::Display) -> AuthError {
    tracing::warn!(%error, "factor database operation failed");
    AuthError::Upstream
}
