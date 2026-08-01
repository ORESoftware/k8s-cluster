#[derive(Serialize)]
pub struct Capabilities {
    mfa_enabled: bool,
    methods: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    threefa_import_scheme: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    biometric_model: Option<&'static str>,
}

#[derive(Serialize)]
pub struct Factor {
    factor_id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<String>,
    created_at: String,
}

#[derive(Deserialize)]
pub struct TotpEnrollRequest {
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
pub struct TotpEnrollment {
    factor_id: String,
    secret_base32: String,
    otpauth_uri: String,
    threefa_import_uri: String,
}

#[derive(Deserialize)]
pub struct TotpConfirmRequest {
    factor_id: String,
    code: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeKind {
    EmailOtp,
    SmsOtp,
}

#[derive(Deserialize)]
pub struct ChallengeRequest {
    kind: ChallengeKind,
}

#[derive(Serialize)]
pub struct ChallengeStart {
    challenge_id: String,
    expires_at: String,
    delivery: String,
}

#[derive(Deserialize)]
pub struct ChallengeVerifyRequest {
    code: String,
}

#[derive(Serialize)]
pub struct CeremonyStart {
    challenge_id: String,
    options: Value,
    expires_at: String,
}

#[derive(Deserialize)]
pub struct PasskeyStartRequest {
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
pub struct PasskeyFinishRequest {
    challenge_id: String,
    credential: Value,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
pub struct StepUpResponse {
    access_token: String,
    token_type: &'static str,
    expires_at: u64,
    amr: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acr: Option<String>,
}

pub async fn capabilities(State(state): State<AppState>) -> Json<Capabilities> {
    let factors = state.factors.as_ref();
    let mut methods = Vec::new();
    // Email OTP does not need the magic-link callback URL. Advertising it from
    // MagicLinkConfig::is_enabled() incorrectly hid a fully configured SendGrid
    // OTP lane whenever passwordless links were intentionally disabled.
    if email_otp_is_enabled(&state) {
        methods.push("email_otp".to_owned());
    }
    if sms_otp_is_enabled(&state) {
        methods.push("sms_otp".to_owned());
    }
    if factors.is_some_and(FactorService::supports_totp) {
        methods.push("totp".to_owned());
    }
    if factors.is_some_and(FactorService::supports_passkeys) {
        methods.push("passkey".to_owned());
    }
    Json(Capabilities {
        mfa_enabled: !methods.is_empty(),
        threefa_import_scheme: methods
            .iter()
            .any(|method| method == "totp")
            .then_some("otpauth"),
        biometric_model: methods
            .iter()
            .any(|method| method == "passkey")
            .then_some("platform_authenticator_webauthn"),
        methods,
    })
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Factor>>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.list_factors(claim_user_id(&claims)?).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_factor_id): Path<String>,
) -> Result<StatusCode, AuthError> {
    let claims = claims(&state, &headers).await?;
    let factor_id = parse_uuid(&raw_factor_id, "invalid factor id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service
        .delete_factor(claim_user_id(&claims)?, factor_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn enroll_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TotpEnrollRequest>,
) -> Result<(StatusCode, Json<TotpEnrollment>), AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let account = claims.email.as_deref().unwrap_or(&claims.sub);
    let enrollment = service
        .enroll_totp(
            claim_user_id(&claims)?,
            account,
            request.label.as_deref(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(enrollment)))
}

pub async fn confirm_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TotpConfirmRequest>,
) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let factor_id = parse_uuid(&request.factor_id, "invalid factor id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service
        .confirm_totp(claim_user_id(&claims)?, factor_id, &request.code)
        .await?;
    Ok(Json(step_up(&state, &claims, "totp")?))
}

pub async fn create_challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChallengeRequest>,
) -> Result<(StatusCode, Json<ChallengeStart>), AuthError> {
    let claims = claims(&state, &headers).await?;
    match request.kind {
        ChallengeKind::EmailOtp if !email_otp_is_enabled(&state) => {
            return Err(AuthError::Unavailable);
        }
        ChallengeKind::SmsOtp if !sms_otp_is_enabled(&state) => {
            return Err(AuthError::Unavailable);
        }
        _ => {}
    }
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let (response, destination, code) = service
        .create_otp_challenge(&claims, request.kind, pepper.as_bytes())
        .await?;
    let delivery = match request.kind {
        ChallengeKind::EmailOtp => send_email_otp(&state, &destination, &code).await,
        ChallengeKind::SmsOtp => {
            crate::twilio::start_sms_verification(
                &state.http,
                &state.config.twilio_verify,
                &destination,
            )
            .await
        }
    };
    if let Err(error) = delivery {
        // The code was never delivered. Consume the durable challenge so it
        // does not occupy the active-challenge budget or become usable after a
        // provider retry whose outcome the server did not observe.
        if let Ok(challenge_id) = Uuid::parse_str(&response.challenge_id) {
            let user_id = claim_user_id(&claims)?;
            let session_id = claim_session_id(&claims)?;
            if let Err(cancel_error) = service
                .cancel_challenge(user_id, session_id, challenge_id)
                .await
            {
                tracing::warn!(error = %cancel_error, %challenge_id, "failed to cancel undelivered OTP challenge");
            }
        }
        return Err(error);
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub async fn verify_challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_challenge_id): Path<String>,
    Json(request): Json<ChallengeVerifyRequest>,
) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&raw_challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;

    let row = service
        .db
        .query_one_raw(statement(
            "SELECT kind FROM shared_auth.auth_challenges \
             WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
               AND consumed_at IS NULL AND expires_at > now() AND attempts < max_attempts",
            vec![
                challenge_id.into(),
                claim_user_id(&claims)?.into(),
                claim_session_id(&claims)?.into(),
            ],
        ))
        .await
        .map_err(db_error)?
        .ok_or(AuthError::Unauthorized)?;
    let kind: String = row.try_get("", "kind").map_err(db_error)?;
    if kind == "sms_otp" {
        let phone = service.verified_phone(claim_user_id(&claims)?).await?;
        let valid = crate::twilio::check_sms_verification(
            &state.http,
            &state.config.twilio_verify,
            &phone,
            &request.code,
        )
        .await?;
        if !valid {
            service
                .record_failed_otp_attempt(&claims, challenge_id, "sms_otp")
                .await?;
            return Err(AuthError::Unauthorized);
        }
    }
    let method = service
        .verify_otp_challenge(
            &claims,
            challenge_id,
            &request.code,
            pepper.as_bytes(),
            kind == "sms_otp",
        )
        .await?;
    Ok(Json(step_up(&state, &claims, method)?))
}

pub async fn start_passkey_registration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PasskeyStartRequest>,
) -> Result<Json<CeremonyStart>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(
        service
            .start_passkey_registration(&claims, request.label.as_deref())
            .await?,
    ))
}

pub async fn finish_passkey_registration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PasskeyFinishRequest>,
) -> Result<Json<Factor>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&request.challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(
        service
            .finish_passkey_registration(
                &claims,
                challenge_id,
                request.credential,
                request.label.as_deref(),
            )
            .await?,
    ))
}

pub async fn start_passkey_authentication(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CeremonyStart>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.start_passkey_authentication(&claims).await?))
}

pub async fn finish_passkey_authentication(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PasskeyFinishRequest>,
) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&request.challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service
        .finish_passkey_authentication(&claims, challenge_id, request.credential)
        .await?;
    Ok(Json(step_up(&state, &claims, "passkey")?))
}
