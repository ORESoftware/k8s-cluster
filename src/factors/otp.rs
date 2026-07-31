impl FactorService {
    async fn create_otp_challenge(
        &self,
        claims: &OreClaims,
        kind: ChallengeKind,
        pepper: &[u8],
    ) -> Result<(ChallengeStart, String, String), AuthError> {
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let code = generate_code()?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(OTP_TTL_MINUTES);
        let (db_kind, destination, delivery) = match kind {
            ChallengeKind::EmailOtp => {
                if !claims.email_verified {
                    return Err(AuthError::Forbidden);
                }
                let email = claims
                    .email
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(AuthError::BadRequest("verified email is required"))?;
                ("email_otp", email, "email")
            }
            ChallengeKind::SmsOtp => {
                let phone = self.verified_phone(user_id).await?;
                ("sms_otp", phone, "sms")
            }
        };
        let challenge_id = Uuid::new_v4();
        let tag = otp_tag(pepper, challenge_id, &code)?;
        self.db
            .execute(statement(
                "INSERT INTO shared_auth.auth_challenges \
                    (challenge_id, shared_user_id, session_id, kind, destination_hint, code_tag, state, max_attempts, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, $8)",
                vec![
                    challenge_id.into(),
                    user_id.into(),
                    session_id.into(),
                    db_kind.to_owned().into(),
                    mask_destination(&destination).into(),
                    tag.into(),
                    MAX_OTP_ATTEMPTS.into(),
                    expires_at.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        Ok((
            ChallengeStart {
                challenge_id: challenge_id.to_string(),
                expires_at: expires_at.to_rfc3339(),
                delivery: delivery.to_owned(),
            },
            destination,
            code,
        ))
    }

    async fn verify_otp_challenge(
        &self,
        claims: &OreClaims,
        challenge_id: Uuid,
        code: &str,
        pepper: &[u8],
        externally_verified: bool,
    ) -> Result<&'static str, AuthError> {
        validate_otp(code)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let row = self
            .db
            .query_one(statement(
                "SELECT kind, code_tag FROM shared_auth.auth_challenges \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
                   AND kind IN ('email_otp', 'sms_otp') AND consumed_at IS NULL \
                   AND expires_at > now() AND attempts < max_attempts",
                vec![challenge_id.into(), user_id.into(), session_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let kind: String = row.try_get("", "kind").map_err(db_error)?;
        let expected: Vec<u8> = row.try_get("", "code_tag").map_err(db_error)?;
        let presented = otp_tag(pepper, challenge_id, code)?;
        if !externally_verified && !constant_time_bytes_eq(&expected, &presented, pepper) {
            self.db
                .execute(statement(
                    "UPDATE shared_auth.auth_challenges SET attempts = attempts + 1 \
                     WHERE challenge_id = $1 AND consumed_at IS NULL",
                    vec![challenge_id.into()],
                ))
                .await
                .map_err(db_error)?;
            return Err(AuthError::Unauthorized);
        }
        let result = self
            .db
            .execute(statement(
                "UPDATE shared_auth.auth_challenges SET consumed_at = now(), attempts = attempts + 1 \
                 WHERE challenge_id = $1 AND consumed_at IS NULL",
                vec![challenge_id.into()],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() != 1 {
            return Err(AuthError::Unauthorized);
        }
        match kind.as_str() {
            "email_otp" => Ok("email_otp"),
            "sms_otp" => Ok("sms_otp"),
            _ => Err(AuthError::Unauthorized),
        }
    }
}
