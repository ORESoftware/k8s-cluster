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

        let transaction = self.db.begin().await.map_err(db_error)?;
        // Serialize challenge starts per session. This makes the resend interval
        // and active-challenge cap reliable across every server replica.
        transaction
            .query_one_raw(statement(
                "SELECT session_id FROM shared_auth.sessions \
                 WHERE session_id = $1 AND shared_user_id = $2 \
                   AND revoked_at IS NULL AND expires_at > now() FOR UPDATE",
                vec![session_id.into(), user_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.auth_challenges SET consumed_at = now() \
                 WHERE shared_user_id = $1 AND session_id = $2 AND kind = $3 \
                   AND consumed_at IS NULL AND expires_at <= now()",
                vec![
                    user_id.into(),
                    session_id.into(),
                    db_kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        let active = transaction
            .query_one_raw(statement(
                "SELECT count(*)::bigint AS active_count, max(created_at) AS latest_created_at \
                 FROM shared_auth.auth_challenges \
                 WHERE shared_user_id = $1 AND session_id = $2 AND kind = $3 \
                   AND consumed_at IS NULL AND expires_at > now()",
                vec![
                    user_id.into(),
                    session_id.into(),
                    db_kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;
        let active_count: i64 = active.try_get("", "active_count").map_err(db_error)?;
        let latest_created_at: Option<DateTime<FixedOffset>> =
            active.try_get("", "latest_created_at").map_err(db_error)?;
        let resend_cutoff = Utc::now().fixed_offset()
            - TimeDelta::seconds(OTP_RESEND_INTERVAL_SECONDS);
        if active_count >= MAX_ACTIVE_OTP_CHALLENGES
            || latest_created_at.is_some_and(|created_at| created_at > resend_cutoff)
        {
            return Err(AuthError::RateLimited);
        }

        transaction
            .execute_raw(statement(
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
        transaction.commit().await.map_err(db_error)?;

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

    async fn record_failed_otp_attempt(
        &self,
        claims: &OreClaims,
        challenge_id: Uuid,
        expected_kind: &str,
    ) -> Result<(), AuthError> {
        if expected_kind != "sms_otp" {
            return Err(AuthError::Unauthorized);
        }
        let result = self
            .db
            .execute_raw(statement(
                "UPDATE shared_auth.auth_challenges SET attempts = attempts + 1 \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
                   AND kind = $4 AND consumed_at IS NULL AND expires_at > now() \
                   AND attempts < max_attempts",
                vec![
                    challenge_id.into(),
                    claim_user_id(claims)?.into(),
                    claim_session_id(claims)?.into(),
                    expected_kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AuthError::Unauthorized)
        }
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
        let transaction = self.db.begin().await.map_err(db_error)?;
        // Lock before checking the tag, expiry, or attempt count. A concurrent
        // verifier can therefore never consume the same challenge twice or
        // succeed after another request exhausted its attempts.
        let row = transaction
            .query_one_raw(statement(
                "SELECT kind, code_tag FROM shared_auth.auth_challenges \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
                   AND kind IN ('email_otp', 'sms_otp') AND consumed_at IS NULL \
                   AND expires_at > now() AND attempts < max_attempts FOR UPDATE",
                vec![challenge_id.into(), user_id.into(), session_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let kind: String = row.try_get("", "kind").map_err(db_error)?;
        let expected: Vec<u8> = row.try_get("", "code_tag").map_err(db_error)?;
        if externally_verified && kind != "sms_otp" {
            return Err(AuthError::Unauthorized);
        }
        let valid = externally_verified || otp_tag_matches(pepper, challenge_id, code, &expected);
        if !valid {
            let result = transaction
                .execute_raw(statement(
                    "UPDATE shared_auth.auth_challenges SET attempts = attempts + 1 \
                     WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
                       AND consumed_at IS NULL AND expires_at > now() AND attempts < max_attempts",
                    vec![challenge_id.into(), user_id.into(), session_id.into()],
                ))
                .await
                .map_err(db_error)?;
            if result.rows_affected() != 1 {
                return Err(AuthError::Unauthorized);
            }
            transaction.commit().await.map_err(db_error)?;
            return Err(AuthError::Unauthorized);
        }

        let result = transaction
            .execute_raw(statement(
                "UPDATE shared_auth.auth_challenges \
                 SET consumed_at = now(), attempts = attempts + 1 \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 \
                   AND kind = $4 AND consumed_at IS NULL AND expires_at > now() \
                   AND attempts < max_attempts",
                vec![
                    challenge_id.into(),
                    user_id.into(),
                    session_id.into(),
                    kind.clone().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() != 1 {
            return Err(AuthError::Unauthorized);
        }
        transaction.commit().await.map_err(db_error)?;
        match kind.as_str() {
            "email_otp" => Ok("email_otp"),
            "sms_otp" => Ok("sms_otp"),
            _ => Err(AuthError::Unauthorized),
        }
    }
}
