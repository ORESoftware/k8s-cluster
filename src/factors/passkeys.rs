impl FactorService {
    async fn start_passkey_registration(
        &self,
        claims: &OreClaims,
        label: Option<&str>,
    ) -> Result<CeremonyStart, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let existing = self.passkeys_for(user_id).await?;
        let exclude = (!existing.is_empty()).then(|| {
            existing
                .iter()
                .map(|(_, passkey)| passkey.cred_id().clone())
                .collect()
        });
        let username = claims
            .email
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&claims.sub);
        let display_name = normalize_label(label)?.unwrap_or_else(|| username.to_owned());
        let (options, registration) = webauthn
            .start_passkey_registration(user_id, username, &display_name, exclude)
            .map_err(|error| {
                tracing::warn!(error = %error, "passkey registration start failed");
                AuthError::BadRequest("unable to start passkey registration")
            })?;
        let state = serde_json::to_value(&registration).map_err(|_| AuthError::Internal)?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(PASSKEY_TTL_MINUTES);
        let challenge_id = self
            .insert_passkey_challenge(
                user_id,
                session_id,
                "passkey_register",
                state,
                expires_at,
            )
            .await?;
        Ok(CeremonyStart {
            challenge_id: challenge_id.to_string(),
            options: serde_json::to_value(options).map_err(|_| AuthError::Internal)?,
            expires_at: expires_at.to_rfc3339(),
        })
    }

    async fn finish_passkey_registration(
        &self,
        claims: &OreClaims,
        challenge_id: Uuid,
        credential: Value,
        label: Option<&str>,
    ) -> Result<Factor, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let state = self
            .take_challenge(user_id, session_id, challenge_id, "passkey_register")
            .await?;
        let registration: PasskeyRegistration =
            serde_json::from_value(state).map_err(|_| AuthError::Internal)?;
        let external_id = credential_id(&credential)?;
        let response: RegisterPublicKeyCredential = serde_json::from_value(credential)
            .map_err(|_| AuthError::BadRequest("invalid passkey credential"))?;
        let passkey = webauthn
            .finish_passkey_registration(&response, &registration)
            .map_err(|error| {
                tracing::info!(error = %error, "passkey registration rejected");
                AuthError::Unauthorized
            })?;
        let factor_id = Uuid::new_v4();
        let public_data = serde_json::to_value(passkey).map_err(|_| AuthError::Internal)?;
        let label = normalize_label(label)?;
        let row = self
            .db
            .query_one_raw(statement(
                "INSERT INTO shared_auth.auth_factors \
                    (factor_id, shared_user_id, kind, label, public_data, external_id, enabled, confirmed_at) \
                 VALUES ($1, $2, 'passkey', $3, $4, $5, true, now()) \
                 RETURNING factor_id, kind, label, enabled, confirmed_at, last_used_at, created_at",
                vec![
                    factor_id.into(),
                    user_id.into(),
                    label.into(),
                    public_data.into(),
                    external_id.into(),
                ],
            ))
            .await
            .map_err(|error| {
                tracing::info!(%error, "duplicate or invalid passkey registration");
                AuthError::Conflict
            })?
            .ok_or(AuthError::Internal)?;
        factor_from_row(&row)
    }

    async fn start_passkey_authentication(
        &self,
        claims: &OreClaims,
    ) -> Result<CeremonyStart, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let stored = self.passkeys_for(user_id).await?;
        if stored.is_empty() {
            return Err(AuthError::BadRequest("no passkeys are enrolled"));
        }
        let passkeys = stored
            .into_iter()
            .map(|(_, passkey)| passkey)
            .collect::<Vec<_>>();
        let (options, authentication) = webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(|error| {
                tracing::warn!(error = %error, "passkey authentication start failed");
                AuthError::BadRequest("unable to start passkey authentication")
            })?;
        let state = serde_json::to_value(authentication).map_err(|_| AuthError::Internal)?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(PASSKEY_TTL_MINUTES);
        let challenge_id = self
            .insert_passkey_challenge(user_id, session_id, "passkey_auth", state, expires_at)
            .await?;
        Ok(CeremonyStart {
            challenge_id: challenge_id.to_string(),
            options: serde_json::to_value(options).map_err(|_| AuthError::Internal)?,
            expires_at: expires_at.to_rfc3339(),
        })
    }

    async fn finish_passkey_authentication(
        &self,
        claims: &OreClaims,
        challenge_id: Uuid,
        credential: Value,
    ) -> Result<(), AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let state = self
            .take_challenge(user_id, session_id, challenge_id, "passkey_auth")
            .await?;
        let authentication: PasskeyAuthentication =
            serde_json::from_value(state).map_err(|_| AuthError::Internal)?;
        let external_id = credential_id(&credential)?;
        let response: PublicKeyCredential = serde_json::from_value(credential)
            .map_err(|_| AuthError::BadRequest("invalid passkey credential"))?;
        let result = webauthn
            .finish_passkey_authentication(&response, &authentication)
            .map_err(|error| {
                tracing::info!(error = %error, "passkey authentication rejected");
                AuthError::Unauthorized
            })?;

        // WebAuthn verification uses the credential snapshot captured when the
        // ceremony started. Lock and reload the current row before applying the
        // result so two valid-looking concurrent assertions cannot both advance
        // from the same old signature counter.
        let transaction = self.db.begin().await.map_err(db_error)?;
        let row = transaction
            .query_one_raw(statement(
                "SELECT public_data FROM shared_auth.auth_factors \
                 WHERE shared_user_id = $1 AND kind = 'passkey' AND external_id = $2 \
                   AND enabled = true FOR UPDATE",
                vec![user_id.into(), external_id.clone().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let data: Value = row.try_get("", "public_data").map_err(db_error)?;
        let mut stored: Passkey =
            serde_json::from_value(data).map_err(|_| AuthError::Internal)?;
        let changed = stored
            .update_credential(&result)
            .ok_or(AuthError::Unauthorized)?;
        if result.needs_update() && !changed {
            // Another ceremony already committed this counter/backup-state
            // transition. Treat the stale assertion as a replay rather than
            // minting a second step-up token.
            return Err(AuthError::Unauthorized);
        }
        let public_data = serde_json::to_value(stored).map_err(|_| AuthError::Internal)?;
        let update = transaction
            .execute_raw(statement(
                "UPDATE shared_auth.auth_factors \
                 SET public_data = $3, last_used_at = now(), updated_at = now() \
                 WHERE shared_user_id = $1 AND external_id = $2 \
                   AND kind = 'passkey' AND enabled = true",
                vec![user_id.into(), external_id.into(), public_data.into()],
            ))
            .await
            .map_err(db_error)?;
        if update.rows_affected() != 1 {
            return Err(AuthError::Unauthorized);
        }
        transaction.commit().await.map_err(db_error)
    }

    async fn passkeys_for(&self, user_id: Uuid) -> Result<Vec<(String, Passkey)>, AuthError> {
        let rows = self
            .db
            .query_all_raw(statement(
                "SELECT external_id, public_data FROM shared_auth.auth_factors \
                 WHERE shared_user_id = $1 AND kind = 'passkey' AND enabled = true",
                vec![user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let external_id: String = row.try_get("", "external_id").map_err(db_error)?;
                let data: Value = row.try_get("", "public_data").map_err(db_error)?;
                let passkey = serde_json::from_value(data).map_err(|_| AuthError::Internal)?;
                Ok((external_id, passkey))
            })
            .collect()
    }

    async fn insert_passkey_challenge(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        kind: &'static str,
        state: Value,
        expires_at: DateTime<FixedOffset>,
    ) -> Result<Uuid, AuthError> {
        let challenge_id = Uuid::new_v4();
        let transaction = self.db.begin().await.map_err(db_error)?;
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
                    kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        let active = transaction
            .query_one_raw(statement(
                "SELECT count(*)::bigint AS active_count \
                 FROM shared_auth.auth_challenges \
                 WHERE shared_user_id = $1 AND session_id = $2 AND kind = $3 \
                   AND consumed_at IS NULL AND expires_at > now()",
                vec![
                    user_id.into(),
                    session_id.into(),
                    kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;
        let active_count: i64 = active.try_get("", "active_count").map_err(db_error)?;
        if active_count >= MAX_ACTIVE_PASSKEY_CEREMONIES {
            return Err(AuthError::RateLimited);
        }
        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.auth_challenges \
                    (challenge_id, shared_user_id, session_id, kind, state, max_attempts, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, 1, $6)",
                vec![
                    challenge_id.into(),
                    user_id.into(),
                    session_id.into(),
                    kind.to_owned().into(),
                    state.into(),
                    expires_at.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(challenge_id)
    }

    async fn take_challenge(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        challenge_id: Uuid,
        kind: &str,
    ) -> Result<Value, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "UPDATE shared_auth.auth_challenges SET consumed_at = now(), attempts = attempts + 1 \
                 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 AND kind = $4 \
                   AND consumed_at IS NULL AND expires_at > now() AND attempts < max_attempts \
                 RETURNING state",
                vec![
                    challenge_id.into(),
                    user_id.into(),
                    session_id.into(),
                    kind.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        row.try_get("", "state").map_err(db_error)
    }
}
