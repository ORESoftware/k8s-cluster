impl FactorService {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        let mut options = ConnectOptions::new(config.url.clone());
        options
            .max_connections(config.max_connections.clamp(1, 3))
            .min_connections(1)
            .connect_timeout(std::time::Duration::from_secs(5))
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(300))
            .sqlx_logging(false);
        let db = Database::connect(options).await?;
        let totp_key = optional_hex_key("AUTH_FACTOR_ENCRYPTION_KEY_HEX")?;
        let webauthn = build_webauthn()?;
        Ok(Self {
            db: Arc::new(db),
            totp_key,
            webauthn,
        })
    }

    fn supports_totp(&self) -> bool {
        self.totp_key.is_some()
    }

    fn supports_passkeys(&self) -> bool {
        self.webauthn.is_some()
    }

    async fn list_factors(&self, user_id: Uuid) -> Result<Vec<Factor>, AuthError> {
        let rows = self
            .db
            .query_all_raw(statement(
                "SELECT factor_id, kind, label, enabled, confirmed_at, last_used_at, created_at \
                 FROM shared_auth.auth_factors \
                 WHERE shared_user_id = $1 \
                 ORDER BY created_at ASC",
                vec![user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        rows.iter().map(factor_from_row).collect()
    }

    async fn delete_factor(&self, user_id: Uuid, factor_id: Uuid) -> Result<(), AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        // Lock the user's complete factor set. Two simultaneous removals must
        // serialize so they cannot both observe two enabled factors and delete
        // the last two independently.
        let rows = transaction
            .query_all_raw(statement(
                "SELECT factor_id, enabled FROM shared_auth.auth_factors \
                 WHERE shared_user_id = $1 ORDER BY factor_id FOR UPDATE",
                vec![user_id.into()],
            ))
            .await
            .map_err(db_error)?;

        let mut target_enabled = None;
        let mut enabled_count = 0_i64;
        for row in rows {
            let row_factor_id: Uuid = row.try_get("", "factor_id").map_err(db_error)?;
            let enabled: bool = row.try_get("", "enabled").map_err(db_error)?;
            enabled_count += i64::from(enabled);
            if row_factor_id == factor_id {
                target_enabled = Some(enabled);
            }
        }
        let target_enabled = target_enabled.ok_or(AuthError::BadRequest("unknown factor"))?;
        if target_enabled && enabled_count <= 1 {
            return Err(AuthError::Conflict);
        }

        let result = transaction
            .execute_raw(statement(
                "DELETE FROM shared_auth.auth_factors \
                 WHERE shared_user_id = $1 AND factor_id = $2",
                vec![user_id.into(), factor_id.into()],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() != 1 {
            return Err(AuthError::BadRequest("unknown factor"));
        }
        transaction.commit().await.map_err(db_error)
    }

    async fn enroll_totp(
        &self,
        user_id: Uuid,
        account_name: &str,
        label: Option<&str>,
    ) -> Result<TotpEnrollment, AuthError> {
        let key = self.totp_key.ok_or(AuthError::Unavailable)?;
        let label = normalize_label(label)?;
        let factor_id = Uuid::new_v4();
        let mut secret = [0u8; 20];
        SysRng
            .try_fill_bytes(&mut secret)
            .map_err(|_| AuthError::Internal)?;
        let mut nonce = [0u8; 12];
        SysRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| AuthError::Internal)?;
        // AES-GCM AAD binds the ciphertext to this user, factor, and format
        // version. Copying ciphertext/nonce between rows therefore cannot turn
        // one user's secret into another user's valid factor.
        let ciphertext = encrypt_totp_secret(&key, user_id, factor_id, nonce, &secret)?;
        let public_data = json!({
            "algorithm": "SHA1",
            "digits": 6,
            "period": TOTP_STEP_SECONDS,
            "last_counter": -1,
            "encryption_version": TOTP_ENCRYPTION_VERSION,
        });
        self.db
            .execute_raw(statement(
                "INSERT INTO shared_auth.auth_factors \
                    (factor_id, shared_user_id, kind, label, secret_ciphertext, secret_nonce, public_data) \
                 VALUES ($1, $2, 'totp', $3, $4, $5, $6)",
                vec![
                    factor_id.into(),
                    user_id.into(),
                    label.clone().into(),
                    ciphertext.into(),
                    nonce.to_vec().into(),
                    public_data.into(),
                ],
            ))
            .await
            .map_err(db_error)?;

        let secret_base32 = encode_base32(&secret);
        let issuer = "OreSoftware";
        let account = if account_name.trim().is_empty() {
            user_id.to_string()
        } else {
            account_name.trim().to_owned()
        };
        let path_label = percent_encode(&format!("{issuer}:{account}"));
        let issuer_query = percent_encode(issuer);
        let otpauth_uri = format!(
            "otpauth://totp/{path_label}?secret={secret_base32}&issuer={issuer_query}&algorithm=SHA1&digits=6&period={TOTP_STEP_SECONDS}"
        );
        Ok(TotpEnrollment {
            factor_id: factor_id.to_string(),
            secret_base32,
            threefa_import_uri: otpauth_uri.clone(),
            otpauth_uri,
        })
    }

    async fn confirm_totp(
        &self,
        user_id: Uuid,
        factor_id: Uuid,
        code: &str,
    ) -> Result<(), AuthError> {
        validate_otp(code)?;
        let key = self.totp_key.ok_or(AuthError::Unavailable)?;
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT secret_ciphertext, secret_nonce, \
                        coalesce((public_data ->> 'last_counter')::bigint, -1) AS last_counter, \
                        coalesce((public_data ->> 'encryption_version')::bigint, 0) AS encryption_version \
                 FROM shared_auth.auth_factors \
                 WHERE factor_id = $1 AND shared_user_id = $2 AND kind = 'totp'",
                vec![factor_id.into(), user_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let ciphertext: Vec<u8> = row.try_get("", "secret_ciphertext").map_err(db_error)?;
        let nonce: Vec<u8> = row.try_get("", "secret_nonce").map_err(db_error)?;
        let last_counter: i64 = row.try_get("", "last_counter").map_err(db_error)?;
        let encryption_version: i64 = row.try_get("", "encryption_version").map_err(db_error)?;
        if encryption_version != TOTP_ENCRYPTION_VERSION {
            return Err(AuthError::Internal);
        }
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| AuthError::Internal)?;
        let secret = decrypt_totp_secret(&key, user_id, factor_id, nonce, &ciphertext)?;
        let current = now_secs() / TOTP_STEP_SECONDS;
        let matched = [current.saturating_sub(1), current, current.saturating_add(1)]
            .into_iter()
            .find(|counter| {
                (*counter as i64) > last_counter
                    && constant_time_code_eq(&totp_code(&secret, *counter), code, &secret)
            })
            .ok_or(AuthError::Unauthorized)?;

        // The monotonic predicate makes successful use of a TOTP time-step a
        // compare-and-set operation. Concurrent replay of the same code can
        // update at most one row.
        let result = self
            .db
            .execute_raw(statement(
                "UPDATE shared_auth.auth_factors SET \
                    enabled = true, confirmed_at = coalesce(confirmed_at, now()), \
                    last_used_at = now(), updated_at = now(), \
                    public_data = jsonb_set(public_data, '{last_counter}', to_jsonb($3::bigint), true) \
                 WHERE factor_id = $1 AND shared_user_id = $2 AND kind = 'totp' \
                   AND coalesce((public_data ->> 'last_counter')::bigint, -1) < $3",
                vec![factor_id.into(), user_id.into(), (matched as i64).into()],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AuthError::Unauthorized)
        }
    }
}
