//! Postgres persistence for biometric-recovery enrollment and ceremonies.
//!
//! The schema stores one-way identifier/ceremony hashes, opaque provider
//! references, normalized booleans/confidence values, and audit state. It never
//! stores government-ID images, face frames/templates, voice audio, or speaker
//! embeddings.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeDelta};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::config::DbConfig;
use crate::error::AuthError;

#[derive(Clone)]
pub struct RecoveryStore {
    db: Arc<DatabaseConnection>,
}

#[derive(Clone, Debug)]
pub struct RecoveryAccount {
    pub shared_user_id: Uuid,
    pub identity_reference_id: Option<String>,
    pub voice_reference_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewCeremony {
    pub ceremony_id: Uuid,
    pub purpose: &'static str,
    pub shared_user_id: Option<Uuid>,
    pub identifier_hash: String,
    pub ceremony_secret_hash: String,
    pub identity_session_id: String,
    pub voice_session_id: String,
    pub identity_binding_present: bool,
    pub requires_manual_review: bool,
    pub consent_version: String,
    pub expires_at: DateTime<FixedOffset>,
}

#[derive(Clone, Debug)]
pub struct CeremonyRecord {
    pub ceremony_id: Uuid,
    pub purpose: String,
    pub shared_user_id: Option<Uuid>,
    pub status: String,
    pub identity_session_id: String,
    pub voice_session_id: String,
    pub identity_binding_present: bool,
    pub requires_manual_review: bool,
    pub consent_version: String,
    pub expires_at: DateTime<FixedOffset>,
    pub available_at: Option<DateTime<FixedOffset>>,
    pub consumed_at: Option<DateTime<FixedOffset>>,
    pub identity_reference_id: Option<String>,
    pub voice_reference_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EvidenceSnapshot {
    pub identity_result_id: Option<String>,
    pub voice_result_id: Option<String>,
    pub identity_reference_id: Option<String>,
    pub voice_reference_id: Option<String>,
    pub document_verified: Option<bool>,
    pub document_confidence: Option<f64>,
    pub face_match: Option<bool>,
    pub face_liveness: Option<bool>,
    pub face_confidence: Option<f64>,
    pub advisory_speaker_match: Option<bool>,
    pub voice_liveness: Option<bool>,
    pub phrase_match: Option<bool>,
    pub voice_liveness_confidence: Option<f64>,
    pub advisory_speaker_confidence: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualReviewDecision {
    Approve,
    Reject,
}

impl RecoveryStore {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        // Recovery has its own deliberately small pool so the feature can be
        // disabled without changing the main identity store. Both pools point
        // at the same authoritative Postgres schema.
        let mut options = ConnectOptions::new(config.url.clone());
        options
            .max_connections(config.max_connections.clamp(1, 3))
            .min_connections(1)
            .connect_timeout(std::time::Duration::from_secs(5))
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(300))
            .sqlx_logging(false);
        let db = Database::connect(options).await?;
        Ok(Self { db: Arc::new(db) })
    }

    pub async fn active_user(&self, shared_user_id: Uuid) -> Result<bool, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT EXISTS (SELECT 1 FROM shared_auth.principals \
                 WHERE shared_user_id = $1 AND status = 'active') AS active",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;
        row.try_get("", "active").map_err(db_error)
    }

    pub async fn account_for_email(
        &self,
        normalized_email: &str,
    ) -> Result<Option<RecoveryAccount>, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT u.shared_user_id, b.identity_reference_id, b.voice_reference_id \
                 FROM shared_auth.principals u \
                 JOIN shared_auth.local_credentials c USING (shared_user_id) \
                 LEFT JOIN shared_auth.biometric_recovery_bindings b \
                   ON b.shared_user_id = u.shared_user_id AND b.revoked_at IS NULL \
                 WHERE lower(u.email) = $1 AND u.status = 'active'",
                vec![normalized_email.to_owned().into()],
            ))
            .await
            .map_err(db_error)?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(RecoveryAccount {
            shared_user_id: row.try_get("", "shared_user_id").map_err(db_error)?,
            identity_reference_id: row
                .try_get("", "identity_reference_id")
                .map_err(db_error)?,
            voice_reference_id: row.try_get("", "voice_reference_id").map_err(db_error)?,
        }))
    }

    pub async fn enforce_daily_limit(
        &self,
        identifier_hash: &str,
        purpose: &str,
        limit: i64,
    ) -> Result<(), AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT count(*)::bigint AS count \
                 FROM shared_auth.account_recovery_ceremonies \
                 WHERE identifier_hash = $1 AND purpose = $2 \
                   AND created_at > now() - interval '24 hours'",
                vec![identifier_hash.to_owned().into(), purpose.to_owned().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;
        let count: i64 = row.try_get("", "count").map_err(db_error)?;
        if count >= limit {
            Err(AuthError::RateLimited)
        } else {
            Ok(())
        }
    }

    pub async fn insert_ceremony(&self, ceremony: NewCeremony) -> Result<(), AuthError> {
        self.db
            .execute_raw(statement(
                "INSERT INTO shared_auth.account_recovery_ceremonies \
                    (ceremony_id, purpose, shared_user_id, identifier_hash, \
                     ceremony_secret_hash, identity_session_id, voice_session_id, \
                     identity_binding_present, requires_manual_review, consent_version, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                vec![
                    ceremony.ceremony_id.into(),
                    ceremony.purpose.to_owned().into(),
                    ceremony.shared_user_id.into(),
                    ceremony.identifier_hash.into(),
                    ceremony.ceremony_secret_hash.into(),
                    ceremony.identity_session_id.into(),
                    ceremony.voice_session_id.into(),
                    ceremony.identity_binding_present.into(),
                    ceremony.requires_manual_review.into(),
                    ceremony.consent_version.into(),
                    ceremony.expires_at.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        Ok(())
    }

    pub async fn load_ceremony(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
    ) -> Result<CeremonyRecord, AuthError> {
        self.db
            .query_one_raw(statement(
                "SELECT ceremony_id, purpose, shared_user_id, status, identity_session_id, \
                        voice_session_id, identity_binding_present, requires_manual_review, \
                        consent_version, expires_at, available_at, consumed_at, \
                        identity_reference_id, voice_reference_id \
                 FROM shared_auth.account_recovery_ceremonies \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)
            .and_then(|row| ceremony_from_row(&row))
    }

    pub async fn mark_expired(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
    ) -> Result<(), AuthError> {
        self.db
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies \
                 SET status = 'expired', updated_at = now(), decision_reason = 'expired' \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2 \
                   AND status IN ('pending', 'pending_review') AND expires_at <= now()",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        Ok(())
    }

    pub async fn record_evaluation_attempt(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
    ) -> Result<(), AuthError> {
        let updated = self
            .db
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    attempts = attempts + 1, updated_at = now() \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2 \
                   AND status = 'pending' AND attempts < 10 AND expires_at > now()",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        if updated.rows_affected() != 1 {
            return Err(AuthError::RateLimited);
        }
        Ok(())
    }

    pub async fn save_evidence(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
        status: &str,
        decision_reason: &str,
        available_at: Option<DateTime<FixedOffset>>,
        new_expires_at: Option<DateTime<FixedOffset>>,
        evidence: &EvidenceSnapshot,
    ) -> Result<(), AuthError> {
        let updated = self
            .db
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    status = $3, decision_reason = $4, available_at = $5, \
                    expires_at = COALESCE($6, expires_at), \
                    identity_result_id = $7, voice_result_id = $8, \
                    identity_reference_id = $9, voice_reference_id = $10, \
                    document_verified = $11, document_confidence = $12, \
                    face_match = $13, face_liveness = $14, face_confidence = $15, \
                    advisory_speaker_match = $16, voice_liveness = $17, phrase_match = $18, \
                    voice_liveness_confidence = $19, advisory_speaker_confidence = $20, \
                    updated_at = now() \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2 \
                   AND consumed_at IS NULL",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                    status.to_owned().into(),
                    decision_reason.to_owned().into(),
                    available_at.into(),
                    new_expires_at.into(),
                    evidence.identity_result_id.clone().into(),
                    evidence.voice_result_id.clone().into(),
                    evidence.identity_reference_id.clone().into(),
                    evidence.voice_reference_id.clone().into(),
                    evidence.document_verified.into(),
                    evidence.document_confidence.into(),
                    evidence.face_match.into(),
                    evidence.face_liveness.into(),
                    evidence.face_confidence.into(),
                    evidence.advisory_speaker_match.into(),
                    evidence.voice_liveness.into(),
                    evidence.phrase_match.into(),
                    evidence.voice_liveness_confidence.into(),
                    evidence.advisory_speaker_confidence.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        if updated.rows_affected() != 1 {
            return Err(AuthError::Conflict);
        }
        Ok(())
    }

    pub async fn complete_enrollment(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
        expected_user: Uuid,
        evidence: &EvidenceSnapshot,
    ) -> Result<(), AuthError> {
        let identity_reference_id = evidence
            .identity_reference_id
            .as_deref()
            .ok_or(AuthError::Upstream)?;
        let voice_reference_id = evidence.voice_reference_id.as_deref();
        let transaction = self.db.begin().await.map_err(db_error)?;
        let row = transaction
            .query_one_raw(statement(
                "SELECT shared_user_id, consent_version, status, expires_at \
                 FROM shared_auth.account_recovery_ceremonies \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2 \
                   AND purpose = 'enrollment' FOR UPDATE",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let shared_user_id: Option<Uuid> = row.try_get("", "shared_user_id").map_err(db_error)?;
        let status: String = row.try_get("", "status").map_err(db_error)?;
        let expires_at: DateTime<FixedOffset> =
            row.try_get("", "expires_at").map_err(db_error)?;
        if shared_user_id != Some(expected_user)
            || status != "pending"
            || expires_at <= chrono::Utc::now().fixed_offset()
        {
            return Err(AuthError::Conflict);
        }
        let consent_version: String = row.try_get("", "consent_version").map_err(db_error)?;

        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.biometric_recovery_bindings \
                    (shared_user_id, identity_reference_id, voice_reference_id, \
                     consent_version, consented_at) \
                 VALUES ($1, $2, $3, $4, now()) \
                 ON CONFLICT (shared_user_id) DO UPDATE SET \
                    identity_reference_id = EXCLUDED.identity_reference_id, \
                    voice_reference_id = EXCLUDED.voice_reference_id, \
                    consent_version = EXCLUDED.consent_version, \
                    consented_at = now(), updated_at = now(), revoked_at = NULL",
                vec![
                    expected_user.into(),
                    identity_reference_id.to_owned().into(),
                    voice_reference_id.map(str::to_owned).into(),
                    consent_version.into(),
                ],
            ))
            .await
            .map_err(db_error)?;

        let updated = transaction
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    status = 'enrolled', decision_reason = 'enrollment_complete', \
                    identity_result_id = $3, voice_result_id = $4, \
                    identity_reference_id = $5, voice_reference_id = $6, \
                    document_verified = $7, document_confidence = $8, \
                    face_match = $9, face_liveness = $10, face_confidence = $11, \
                    advisory_speaker_match = $12, voice_liveness = $13, phrase_match = $14, \
                    voice_liveness_confidence = $15, advisory_speaker_confidence = $16, \
                    consumed_at = now(), updated_at = now() \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                    evidence.identity_result_id.clone().into(),
                    evidence.voice_result_id.clone().into(),
                    evidence.identity_reference_id.clone().into(),
                    evidence.voice_reference_id.clone().into(),
                    evidence.document_verified.into(),
                    evidence.document_confidence.into(),
                    evidence.face_match.into(),
                    evidence.face_liveness.into(),
                    evidence.face_confidence.into(),
                    evidence.advisory_speaker_match.into(),
                    evidence.voice_liveness.into(),
                    evidence.phrase_match.into(),
                    evidence.voice_liveness_confidence.into(),
                    evidence.advisory_speaker_confidence.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        if updated.rows_affected() != 1 {
            return Err(AuthError::Conflict);
        }
        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub async fn apply_manual_review(
        &self,
        ceremony_id: Uuid,
        decision: ManualReviewDecision,
        reviewer: &str,
        cooldown_secs: u64,
        redeem_ttl_secs: u64,
    ) -> Result<(), AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        let row = transaction
            .query_one_raw(statement(
                "SELECT shared_user_id, identity_binding_present, consent_version, \
                        identity_reference_id, voice_reference_id, status, expires_at \
                 FROM shared_auth.account_recovery_ceremonies \
                 WHERE ceremony_id = $1 AND purpose = 'recovery' FOR UPDATE",
                vec![ceremony_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let status: String = row.try_get("", "status").map_err(db_error)?;
        let expires_at: DateTime<FixedOffset> =
            row.try_get("", "expires_at").map_err(db_error)?;
        if status != "pending_review" || expires_at <= chrono::Utc::now().fixed_offset() {
            return Err(AuthError::Conflict);
        }

        match decision {
            ManualReviewDecision::Reject => {
                transaction
                    .execute_raw(statement(
                        "UPDATE shared_auth.account_recovery_ceremonies SET \
                            status = 'rejected', decision_reason = 'manual_reject', \
                            reviewed_at = now(), reviewed_by = $2, updated_at = now() \
                         WHERE ceremony_id = $1",
                        vec![ceremony_id.into(), reviewer.to_owned().into()],
                    ))
                    .await
                    .map_err(db_error)?;
            }
            ManualReviewDecision::Approve => {
                let shared_user_id: Option<Uuid> =
                    row.try_get("", "shared_user_id").map_err(db_error)?;
                let shared_user_id = shared_user_id.ok_or(AuthError::Forbidden)?;
                let identity_binding_present: bool = row
                    .try_get("", "identity_binding_present")
                    .map_err(db_error)?;
                if !identity_binding_present {
                    let identity_reference_id: Option<String> = row
                        .try_get("", "identity_reference_id")
                        .map_err(db_error)?;
                    let voice_reference_id: Option<String> = row
                        .try_get("", "voice_reference_id")
                        .map_err(db_error)?;
                    let identity_reference_id =
                        identity_reference_id.ok_or(AuthError::Conflict)?;
                    let consent_version: String =
                        row.try_get("", "consent_version").map_err(db_error)?;
                    transaction
                        .execute_raw(statement(
                            "INSERT INTO shared_auth.biometric_recovery_bindings \
                                (shared_user_id, identity_reference_id, voice_reference_id, \
                                 consent_version, consented_at) \
                             VALUES ($1, $2, $3, $4, now()) \
                             ON CONFLICT (shared_user_id) DO UPDATE SET \
                                identity_reference_id = EXCLUDED.identity_reference_id, \
                                voice_reference_id = EXCLUDED.voice_reference_id, \
                                consent_version = EXCLUDED.consent_version, \
                                consented_at = now(), updated_at = now(), revoked_at = NULL",
                            vec![
                                shared_user_id.into(),
                                identity_reference_id.into(),
                                voice_reference_id.into(),
                                consent_version.into(),
                            ],
                        ))
                        .await
                        .map_err(db_error)?;
                }
                let available_at = chrono::Utc::now().fixed_offset()
                    + TimeDelta::seconds(cooldown_secs as i64);
                let expires_at = available_at + TimeDelta::seconds(redeem_ttl_secs as i64);
                transaction
                    .execute_raw(statement(
                        "UPDATE shared_auth.account_recovery_ceremonies SET \
                            status = 'cooldown', decision_reason = 'manual_approve', \
                            available_at = $2, expires_at = $3, reviewed_at = now(), \
                            reviewed_by = $4, updated_at = now() \
                         WHERE ceremony_id = $1",
                        vec![
                            ceremony_id.into(),
                            available_at.into(),
                            expires_at.into(),
                            reviewer.to_owned().into(),
                        ],
                    ))
                    .await
                    .map_err(db_error)?;
            }
        }

        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub async fn redeem(
        &self,
        ceremony_id: Uuid,
        ceremony_secret_hash: &str,
        password_hash: &str,
    ) -> Result<(), AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        let row = transaction
            .query_one_raw(statement(
                "SELECT shared_user_id, status, available_at, expires_at \
                 FROM shared_auth.account_recovery_ceremonies \
                 WHERE ceremony_id = $1 AND ceremony_secret_hash = $2 \
                   AND purpose = 'recovery' FOR UPDATE",
                vec![
                    ceremony_id.into(),
                    ceremony_secret_hash.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let shared_user_id: Option<Uuid> = row.try_get("", "shared_user_id").map_err(db_error)?;
        let shared_user_id = shared_user_id.ok_or(AuthError::Unauthorized)?;
        let status: String = row.try_get("", "status").map_err(db_error)?;
        let available_at: Option<DateTime<FixedOffset>> =
            row.try_get("", "available_at").map_err(db_error)?;
        let expires_at: DateTime<FixedOffset> =
            row.try_get("", "expires_at").map_err(db_error)?;
        let now = chrono::Utc::now().fixed_offset();
        if status != "cooldown"
            || available_at.map_or(true, |available_at| available_at > now)
            || expires_at <= now
        {
            return Err(AuthError::Conflict);
        }

        let credential = transaction
            .execute_raw(statement(
                "UPDATE shared_auth.local_credentials SET \
                    password_hash = $2, password_changed_at = now(), failed_attempts = 0, \
                    locked_until = NULL, updated_at = now() \
                 WHERE shared_user_id = $1",
                vec![shared_user_id.into(), password_hash.to_owned().into()],
            ))
            .await
            .map_err(db_error)?;
        if credential.rows_affected() != 1 {
            return Err(AuthError::Unauthorized);
        }

        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.sessions SET revoked_at = COALESCE(revoked_at, now()), \
                    updated_at = now() \
                 WHERE shared_user_id = $1 AND revoked_at IS NULL",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    status = 'consumed', decision_reason = 'password_reset', \
                    consumed_at = now(), updated_at = now() \
                 WHERE ceremony_id = $1",
                vec![ceremony_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    status = 'rejected', decision_reason = 'superseded', updated_at = now() \
                 WHERE shared_user_id = $1 AND ceremony_id <> $2 \
                   AND purpose = 'recovery' \
                   AND status IN ('pending', 'pending_review', 'cooldown')",
                vec![shared_user_id.into(), ceremony_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub async fn revoke_binding(&self, shared_user_id: Uuid) -> Result<(), AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.biometric_recovery_bindings SET \
                    revoked_at = COALESCE(revoked_at, now()), updated_at = now() \
                 WHERE shared_user_id = $1",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "UPDATE shared_auth.account_recovery_ceremonies SET \
                    status = 'rejected', decision_reason = 'binding_revoked', updated_at = now() \
                 WHERE shared_user_id = $1 \
                   AND status IN ('pending', 'pending_review', 'cooldown')",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }
}

fn ceremony_from_row(row: &sea_orm::QueryResult) -> Result<CeremonyRecord, AuthError> {
    Ok(CeremonyRecord {
        ceremony_id: row.try_get("", "ceremony_id").map_err(db_error)?,
        purpose: row.try_get("", "purpose").map_err(db_error)?,
        shared_user_id: row.try_get("", "shared_user_id").map_err(db_error)?,
        status: row.try_get("", "status").map_err(db_error)?,
        identity_session_id: row
            .try_get("", "identity_session_id")
            .map_err(db_error)?,
        voice_session_id: row.try_get("", "voice_session_id").map_err(db_error)?,
        identity_binding_present: row
            .try_get("", "identity_binding_present")
            .map_err(db_error)?,
        requires_manual_review: row
            .try_get("", "requires_manual_review")
            .map_err(db_error)?,
        consent_version: row.try_get("", "consent_version").map_err(db_error)?,
        expires_at: row.try_get("", "expires_at").map_err(db_error)?,
        available_at: row.try_get("", "available_at").map_err(db_error)?,
        consumed_at: row.try_get("", "consumed_at").map_err(db_error)?,
        identity_reference_id: row
            .try_get("", "identity_reference_id")
            .map_err(db_error)?,
        voice_reference_id: row
            .try_get("", "voice_reference_id")
            .map_err(db_error)?,
    })
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn db_error<E: std::fmt::Display>(error: E) -> AuthError {
    tracing::error!(%error, "shared-auth recovery database operation failed");
    AuthError::Upstream
}
