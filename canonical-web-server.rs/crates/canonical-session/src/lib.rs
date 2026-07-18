use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use canonical_auth::{
    validated_session_id, AuthContext, AuthProvider, AuthProviderError, AuthTokens,
    CredentialSource,
};
use canonical_store::{
    begin_session_revocation_transaction, begin_user_transaction, database_now,
    entity::{user_profile, web_session},
    verify_session_revoker_database_role,
};
use chrono::{Duration as ChronoDuration, Utc};
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{watch, Mutex as AsyncMutex},
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("authentication failed")]
    Unauthorized,
    #[error("invalid session request: {0}")]
    BadRequest(String),
    #[error("upstream authentication service failed")]
    AuthUpstream,
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
    #[error("session cryptography failed")]
    Crypto,
}

// Keep the state-machine implementation readable while exposing a
// transport-neutral error name to dependent crates.
use SessionError as AppError;

const REVOCATION_BATCH_SIZE: u64 = 32;
const REVOCATION_WORKER_INTERVAL: Duration = Duration::from_secs(60);
// `SupabaseAuth` and `canonical_store::connect_database` each use a 10-second
// timeout. The longest revocation branch after claiming a lease is sign-out,
// refresh, sign-out (three auth calls) plus two pool acquisitions to persist
// rotation and the terminal result: 50 seconds. Seventy-five seconds leaves a
// 25-second scheduling margin so a second replica cannot reuse the credentials
// while the first valid claim can still be completing.
const REVOCATION_LEASE: ChronoDuration = ChronoDuration::seconds(75);
const REVOCATION_RETENTION: ChronoDuration = ChronoDuration::days(7);
const REFRESH_LEASE: ChronoDuration = ChronoDuration::seconds(30);
const REFRESH_WAIT: Duration = Duration::from_secs(22);
const REFRESH_POLL_INITIAL: Duration = Duration::from_millis(50);
const REFRESH_POLL_MAX: Duration = Duration::from_secs(1);
const MAX_REFRESH_COORDINATORS: usize = 4_096;

/// Coalesces a refresh burst inside one process while the row-scoped database
/// lease remains the cross-replica authority. Weak entries disappear as soon
/// as the last waiter finishes, and the hard cap fails closed under an attack
/// that presents many unrelated session identifiers at once.
#[derive(Default)]
struct RefreshCoordinator {
    entries: StdMutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl RefreshCoordinator {
    fn lock_for(&self, id_hash: &str) -> Option<Arc<AsyncMutex<()>>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|_, lock| lock.strong_count() > 0);

        if let Some(lock) = entries.get(id_hash).and_then(Weak::upgrade) {
            return Some(lock);
        }
        if entries.len() >= MAX_REFRESH_COORDINATORS {
            return None;
        }

        let lock = Arc::new(AsyncMutex::new(()));
        entries.insert(id_hash.to_owned(), Arc::downgrade(&lock));
        Some(lock)
    }

    fn prune(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, lock| lock.strong_count() > 0);
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[derive(Clone)]
pub struct SessionService {
    db: DatabaseConnection,
    auth: Arc<dyn AuthProvider>,
    cipher: Aes256Gcm,
    ttl: Duration,
    revocation_mode: RevocationMode,
    refresh_coordinator: Arc<RefreshCoordinator>,
}

#[derive(Clone, Copy)]
enum RevocationMode {
    Runtime,
    DedicatedRevoker,
}

/// The separately deployed, database-role-checked Supabase logout reconciler.
///
/// The customer web process can enqueue and attempt its own logout, but only
/// this type runs durable scans/pruning through `canonical_session_revoker`.
#[derive(Clone)]
pub struct SessionRevoker {
    sessions: SessionService,
}

pub struct CreatedSession {
    pub raw_id: String,
    pub csrf_token: String,
    pub context: AuthContext,
}

/// Owns the narrowly scoped retry loop for Supabase logout reconciliation.
/// It never accesses user-owned RLS tables and is intentionally not a general
/// background-job facility.
pub struct RevocationWorkerTask {
    shutdown: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for RevocationWorkerTask {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

impl RevocationWorkerTask {
    /// Stops the loop and waits until no reconciliation transaction is still
    /// using the database pool.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl SessionRevoker {
    pub fn new(
        db: DatabaseConnection,
        auth: Arc<dyn AuthProvider>,
        encryption_key: &[u8],
    ) -> Result<Self, AppError> {
        Ok(Self {
            sessions: SessionService::new_with_mode(
                db,
                auth,
                encryption_key,
                Duration::ZERO,
                RevocationMode::DedicatedRevoker,
            )?,
        })
    }

    pub async fn reconcile_once(&self) -> Result<(), AppError> {
        self.sessions.reconcile_revocations().await
    }

    /// Fails startup before the retry loop if the database URL does not use
    /// the exact isolated revoker login and safe catalog attributes.
    pub async fn verify_database_role(&self) -> Result<(), AppError> {
        verify_session_revoker_database_role(&self.sessions.db)
            .await
            .map_err(AppError::from)
    }

    pub fn spawn(&self) -> RevocationWorkerTask {
        let revoker = self.clone();
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            let mut ticker = time::interval(REVOCATION_WORKER_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(error) = revoker.reconcile_once().await {
                            tracing::error!(%error, "Supabase logout reconciliation failed");
                        }
                    }
                    result = shutdown_rx.changed() => {
                        if result.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        RevocationWorkerTask {
            shutdown,
            handle: Some(handle),
        }
    }
}

struct RevocationClaim {
    id_hash: String,
    access_token: String,
    refresh_token: String,
    access_expires_at: chrono::DateTime<Utc>,
    supabase_session_id: Option<uuid::Uuid>,
    attempts: i32,
    user_id: uuid::Uuid,
}

struct RefreshClaim {
    id_hash: String,
    lease_id: uuid::Uuid,
    user_id: uuid::Uuid,
    supabase_session_id: Option<uuid::Uuid>,
    refresh_token: String,
}

enum RefreshClaimResult {
    Fresh(Box<web_session::Model>),
    Claimed(RefreshClaim),
    Busy,
}

enum RevocationRefreshFailure {
    Upstream(AuthProviderError),
    Local(AppError),
}

impl SessionService {
    pub fn new(
        db: DatabaseConnection,
        auth: Arc<dyn AuthProvider>,
        encryption_key: &[u8],
        ttl: Duration,
    ) -> Result<Self, AppError> {
        Self::new_with_mode(db, auth, encryption_key, ttl, RevocationMode::Runtime)
    }

    fn new_with_mode(
        db: DatabaseConnection,
        auth: Arc<dyn AuthProvider>,
        encryption_key: &[u8],
        ttl: Duration,
        revocation_mode: RevocationMode,
    ) -> Result<Self, AppError> {
        let cipher = Aes256Gcm::new_from_slice(encryption_key).map_err(|_| AppError::Crypto)?;
        Ok(Self {
            db,
            auth,
            cipher,
            ttl,
            revocation_mode,
            refresh_coordinator: Arc::new(RefreshCoordinator::default()),
        })
    }

    pub async fn create(&self, tokens: AuthTokens) -> Result<CreatedSession, AppError> {
        let raw_id = random_token();
        let id_hash = hash_token(&raw_id);
        let csrf_token = random_token();
        let now = Utc::now();
        let expires_at = now
            + ChronoDuration::from_std(self.ttl)
                .map_err(|_| AppError::BadRequest("session TTL is too large".into()))?;
        let email = tokens.user.email.clone().unwrap_or_default();
        let supabase_session_id =
            validated_session_id(&tokens.access_token, tokens.user.id).map_err(map_auth_error)?;
        let transaction = begin_user_transaction(&self.db, tokens.user.id).await?;

        web_session::ActiveModel {
            id_hash: Set(id_hash.clone()),
            user_id: Set(tokens.user.id),
            email: Set(email.clone()),
            supabase_session_id: Set(supabase_session_id),
            encrypted_access_token: Set(self.encrypt(&tokens.access_token)?),
            encrypted_refresh_token: Set(self.encrypt(&tokens.refresh_token)?),
            access_expires_at: Set(tokens.expires_at),
            refresh_lease_id: Set(None),
            refresh_lease_expires_at: Set(None),
            csrf_token: Set(csrf_token.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            expires_at: Set(expires_at),
            revoked_at: Set(None),
            revocation_pending_at: Set(None),
            revocation_next_attempt_at: Set(None),
            revocation_attempts: Set(0),
            upstream_revoked_at: Set(None),
            revocation_abandoned_at: Set(None),
            revocation_failure_kind: Set(None),
        }
        .insert(&transaction)
        .await?;

        user_profile::Entity::insert(user_profile::ActiveModel {
            user_id: Set(tokens.user.id),
            email: Set(email.clone()),
            display_name: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(user_profile::Column::UserId)
                .update_columns([user_profile::Column::Email, user_profile::Column::UpdatedAt])
                .to_owned(),
        )
        .exec(&transaction)
        .await?;
        transaction.commit().await?;

        Ok(CreatedSession {
            raw_id,
            csrf_token: csrf_token.clone(),
            context: AuthContext {
                user_id: tokens.user.id,
                email,
                source: CredentialSource::SessionCookie,
                supabase_session_id,
                session_hash: Some(id_hash),
                csrf_token: Some(csrf_token),
                expires_at,
            },
        })
    }

    pub async fn authenticate(&self, raw_id: &str) -> Result<AuthContext, AppError> {
        let id_hash = hash_token(raw_id);
        self.authenticate_hash(&id_hash).await
    }

    pub async fn revalidate(&self, context: &AuthContext) -> Result<(), AppError> {
        match context.session_hash.as_deref() {
            Some(id_hash) => {
                self.authenticate_hash(id_hash).await?;
                Ok(())
            }
            None if context.expires_at > Utc::now() => {
                if let Some(session_id) = context.supabase_session_id {
                    self.reject_locally_revoked_bearer(context.user_id, session_id)
                        .await?;
                }
                Ok(())
            }
            None => Err(AppError::Unauthorized),
        }
    }

    async fn authenticate_hash(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let model = web_session::Entity::find_by_id(id_hash)
            .one(&self.db)
            .await?
            .ok_or(AppError::Unauthorized)?;
        self.validate_model(&model)?;

        if model.access_expires_at <= Utc::now() + ChronoDuration::seconds(60) {
            return self.refresh_fenced(id_hash).await;
        }
        Ok(context_from_model(model))
    }

    pub async fn authenticate_bearer(&self, token: &str) -> Result<AuthContext, AppError> {
        let user = self
            .auth
            .user_for_token(token)
            .await
            .map_err(map_auth_error)?;
        let supabase_session_id = validated_session_id(token, user.id).map_err(map_auth_error)?;
        if let Some(session_id) = supabase_session_id {
            self.reject_locally_revoked_bearer(user.id, session_id)
                .await?;
        }
        Ok(AuthContext {
            user_id: user.id,
            email: user.email.unwrap_or_default(),
            source: CredentialSource::Bearer,
            supabase_session_id,
            session_hash: None,
            csrf_token: None,
            // `/user` authenticates the token. The decoded `exp` is used only
            // to bound a WebSocket lifetime, never as proof of identity.
            expires_at: bearer_expiry(token)
                .unwrap_or_else(|| Utc::now() + ChronoDuration::minutes(5)),
        })
    }

    async fn reject_locally_revoked_bearer(
        &self,
        user_id: uuid::Uuid,
        session_id: uuid::Uuid,
    ) -> Result<(), AppError> {
        let locally_revoked = web_session::Entity::find()
            .filter(web_session::Column::UserId.eq(user_id))
            .filter(web_session::Column::SupabaseSessionId.eq(session_id))
            .filter(web_session::Column::RevokedAt.is_not_null())
            .one(&self.db)
            .await?
            .is_some();
        if locally_revoked {
            Err(AppError::Unauthorized)
        } else {
            Ok(())
        }
    }

    pub async fn revoke(&self, raw_id: &str) -> Result<(), AppError> {
        let id_hash = hash_token(raw_id);
        self.queue_revocation(&id_hash).await?;
        if let Err(error) = self.attempt_revocation(&id_hash).await {
            // The local session is already revoked. Persisted retry state
            // prevents an upstream outage from turning logout into a failure.
            tracing::warn!(%error, "Supabase logout deferred for retry");
        }
        Ok(())
    }

    /// Centralizes the transaction boundary for the session-revocation state
    /// machine. Customer-triggered logout uses its normal least-privilege
    /// connection; the separate revoker must pass the exact database-role
    /// check on every transaction.
    async fn begin_revocation_transaction(&self) -> Result<DatabaseTransaction, sea_orm::DbErr> {
        match self.revocation_mode {
            RevocationMode::Runtime => self.db.begin().await,
            RevocationMode::DedicatedRevoker => {
                begin_session_revocation_transaction(&self.db).await
            }
        }
    }

    /// Queues expired local sessions and retries a bounded batch of durable
    /// upstream logout operations. This is safe to run from several replicas:
    /// each attempt first takes a short database-backed lease.
    async fn reconcile_revocations(&self) -> Result<(), AppError> {
        let transaction = self.begin_revocation_transaction().await?;
        let now = database_now(&transaction).await?;
        let expired = web_session::Entity::find()
            .filter(web_session::Column::RevokedAt.is_null())
            .filter(web_session::Column::ExpiresAt.lte(now))
            .order_by_asc(web_session::Column::ExpiresAt)
            .limit(REVOCATION_BATCH_SIZE)
            .all(&transaction)
            .await?;
        for session in expired {
            Self::mark_revocation(session, &transaction, now).await?;
        }

        let pending = web_session::Entity::find()
            .filter(web_session::Column::RevocationPendingAt.is_not_null())
            .filter(web_session::Column::UpstreamRevokedAt.is_null())
            .filter(web_session::Column::RevocationAbandonedAt.is_null())
            .filter(web_session::Column::RevocationNextAttemptAt.lte(now))
            .order_by_asc(web_session::Column::RevocationNextAttemptAt)
            .limit(REVOCATION_BATCH_SIZE)
            .all(&transaction)
            .await?;
        let pending_ids = pending
            .into_iter()
            .map(|session| session.id_hash)
            .collect::<Vec<_>>();

        let retention_cutoff = now - REVOCATION_RETENTION;
        let confirmed_pruned = web_session::Entity::delete_many()
            .filter(web_session::Column::UpstreamRevokedAt.lte(retention_cutoff))
            .filter(web_session::Column::AccessExpiresAt.lte(now))
            .exec(&transaction)
            .await?;
        let abandoned_pruned = web_session::Entity::delete_many()
            .filter(web_session::Column::RevocationAbandonedAt.lte(retention_cutoff))
            .filter(web_session::Column::AccessExpiresAt.lte(now))
            .exec(&transaction)
            .await?;
        transaction.commit().await?;

        for id_hash in pending_ids {
            if let Err(error) = self.attempt_revocation(&id_hash).await {
                tracing::warn!(%error, "Supabase logout retry was deferred");
            }
        }
        let pruned = confirmed_pruned
            .rows_affected
            .saturating_add(abandoned_pruned.rows_affected);
        if pruned > 0 {
            tracing::info!(count = pruned, "pruned terminal revoked sessions");
        }
        Ok(())
    }

    async fn queue_revocation(&self, id_hash: &str) -> Result<(), AppError> {
        // A customer logout must become locally authoritative even if the
        // separate revoker process is unavailable, so the request uses the
        // normal web-session grant and only enqueues durable work here.
        let transaction = self.db.begin().await?;
        let model = web_session::Entity::find_by_id(id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?;
        let Some(model) = model else {
            transaction.commit().await?;
            return Ok(());
        };
        let now = database_now(&transaction).await?;
        Self::mark_revocation(model, &transaction, now).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn mark_revocation(
        model: web_session::Model,
        transaction: &DatabaseTransaction,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), AppError> {
        if model.upstream_revoked_at.is_some() || model.revocation_abandoned_at.is_some() {
            return Ok(());
        }
        let revoked_at = model.revoked_at.or(Some(now));
        let pending_at = model.revocation_pending_at.or(Some(now));
        let next_attempt_at = if model.revocation_pending_at.is_none() {
            Some(now)
        } else {
            model.revocation_next_attempt_at
        };
        let mut active: web_session::ActiveModel = model.into();
        active.revoked_at = Set(revoked_at);
        active.revocation_pending_at = Set(pending_at);
        active.revocation_next_attempt_at = Set(next_attempt_at);
        // Logout is locally authoritative. Clearing an in-flight refresh lease
        // makes the refresh result fail its fence instead of reviving a session
        // that was revoked while the upstream request was in flight.
        active.refresh_lease_id = Set(None);
        active.refresh_lease_expires_at = Set(None);
        active.updated_at = Set(now);
        active.update(transaction).await?;
        Ok(())
    }

    async fn attempt_revocation(&self, id_hash: &str) -> Result<(), AppError> {
        let Some(mut claim) = self.claim_revocation(id_hash).await? else {
            return Ok(());
        };

        let mut refreshed = false;
        if claim.access_expires_at <= Utc::now() + ChronoDuration::seconds(5) {
            match self.refresh_revocation_claim(&claim).await {
                Ok(rotated) => {
                    claim = rotated;
                    refreshed = true;
                }
                Err(RevocationRefreshFailure::Upstream(AuthProviderError::InvalidCredentials)) => {
                    // Neither stored credential is accepted, but no positive
                    // logout acknowledgement was received. Record a distinct
                    // terminal state instead of claiming upstream success.
                    return self
                        .abandon_revocation(&claim, "credentials_rejected")
                        .await;
                }
                Err(RevocationRefreshFailure::Upstream(error)) => {
                    self.defer_revocation(&claim, &error).await?;
                    return Err(map_auth_error(error));
                }
                Err(RevocationRefreshFailure::Local(error)) => return Err(error),
            }
        }

        loop {
            match self.auth.sign_out(&claim.access_token).await {
                Ok(()) => return self.confirm_revocation(&claim).await,
                // A 401 is not proof that logout succeeded. It may only mean
                // that the access token expired between the lease and request.
                // Refresh once, persist the rotated pair, and retry logout.
                Err(AuthProviderError::InvalidCredentials) if !refreshed => {
                    match self.refresh_revocation_claim(&claim).await {
                        Ok(rotated) => {
                            claim = rotated;
                            refreshed = true;
                        }
                        Err(RevocationRefreshFailure::Upstream(
                            AuthProviderError::InvalidCredentials,
                        )) => {
                            return self
                                .abandon_revocation(&claim, "credentials_rejected")
                                .await;
                        }
                        Err(RevocationRefreshFailure::Upstream(error)) => {
                            self.defer_revocation(&claim, &error).await?;
                            return Err(map_auth_error(error));
                        }
                        Err(RevocationRefreshFailure::Local(error)) => return Err(error),
                    }
                }
                Err(error) => {
                    self.defer_revocation(&claim, &error).await?;
                    return Err(map_auth_error(error));
                }
            }
        }
    }

    async fn claim_revocation(&self, id_hash: &str) -> Result<Option<RevocationClaim>, AppError> {
        let transaction = self.begin_revocation_transaction().await?;
        let model = web_session::Entity::find_by_id(id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?;
        let Some(model) = model else {
            transaction.commit().await?;
            return Ok(None);
        };
        let now = database_now(&transaction).await?;
        if model.revoked_at.is_none()
            || model.upstream_revoked_at.is_some()
            || model.revocation_abandoned_at.is_some()
            || model
                .revocation_next_attempt_at
                .is_some_and(|next_attempt| next_attempt > now)
        {
            transaction.commit().await?;
            return Ok(None);
        }

        let token_pair =
            match self
                .decrypt(&model.encrypted_access_token)
                .and_then(|access_token| {
                    self.decrypt(&model.encrypted_refresh_token)
                        .map(|refresh_token| (access_token, refresh_token))
                }) {
                Ok(tokens) => tokens,
                Err(error) => {
                    let attempts = model.revocation_attempts.saturating_add(1);
                    let mut active: web_session::ActiveModel = model.into();
                    active.revocation_attempts = Set(attempts);
                    active.revocation_pending_at = Set(None);
                    active.revocation_next_attempt_at = Set(None);
                    active.revocation_abandoned_at = Set(Some(now));
                    active.revocation_failure_kind = Set(Some("token_decryption_failed".into()));
                    active.updated_at = Set(now);
                    active.update(&transaction).await?;
                    transaction.commit().await?;
                    tracing::error!(%error, "unable to decrypt session for Supabase logout retry");
                    return Ok(None);
                }
            };

        let user_id = model.user_id;
        // Older rows created before Supabase session identity was stored can
        // still be made locally authoritative at logout. Derive the identity
        // from the already provider-issued access JWT and persist it in the
        // same lease transaction before any upstream sign-out attempt. Opaque
        // alternate-provider tokens remain compatible and resolve to None.
        let observed_session_id =
            validated_session_id(&token_pair.0, user_id).map_err(map_auth_error)?;
        let supabase_session_id =
            resolve_rotated_session_id(model.supabase_session_id, observed_session_id)
                .map_err(map_auth_error)?;
        let attempts = model.revocation_attempts.saturating_add(1);
        let access_expires_at = model.access_expires_at;
        let mut active: web_session::ActiveModel = model.into();
        active.supabase_session_id = Set(supabase_session_id);
        active.revocation_attempts = Set(attempts);
        active.revocation_next_attempt_at = Set(Some(now + REVOCATION_LEASE));
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;
        Ok(Some(RevocationClaim {
            id_hash: id_hash.to_owned(),
            access_token: token_pair.0,
            refresh_token: token_pair.1,
            access_expires_at,
            supabase_session_id,
            attempts,
            user_id,
        }))
    }

    async fn refresh_revocation_claim(
        &self,
        claim: &RevocationClaim,
    ) -> Result<RevocationClaim, RevocationRefreshFailure> {
        let tokens = self
            .auth
            .refresh(&claim.refresh_token)
            .await
            .map_err(RevocationRefreshFailure::Upstream)?;
        if tokens.user.id != claim.user_id {
            return Err(RevocationRefreshFailure::Upstream(
                AuthProviderError::InvalidResponse,
            ));
        }
        let observed_session_id = validated_session_id(&tokens.access_token, claim.user_id)
            .map_err(RevocationRefreshFailure::Upstream)?;
        let supabase_session_id =
            resolve_rotated_session_id(claim.supabase_session_id, observed_session_id)
                .map_err(RevocationRefreshFailure::Upstream)?;
        self.persist_revocation_rotation(claim, tokens, supabase_session_id)
            .await
            .map_err(RevocationRefreshFailure::Local)
    }

    /// Persists a rotated refresh token before using the accompanying access
    /// token for logout. Matching the claim attempt prevents a stale replica
    /// from overwriting credentials stored by a newer lease holder.
    async fn persist_revocation_rotation(
        &self,
        claim: &RevocationClaim,
        tokens: AuthTokens,
        supabase_session_id: Option<uuid::Uuid>,
    ) -> Result<RevocationClaim, AppError> {
        let encrypted_access_token = self.encrypt(&tokens.access_token)?;
        let encrypted_refresh_token = self.encrypt(&tokens.refresh_token)?;
        let transaction = self.begin_revocation_transaction().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Err(AppError::Unauthorized);
        };
        if model.upstream_revoked_at.is_some()
            || model.revocation_abandoned_at.is_some()
            || model.revocation_attempts != claim.attempts
        {
            transaction.commit().await?;
            return Err(AppError::Unauthorized);
        }

        let now = database_now(&transaction).await?;
        let email = tokens.user.email.clone().unwrap_or(model.email.clone());
        let mut active: web_session::ActiveModel = model.into();
        active.email = Set(email);
        active.supabase_session_id = Set(supabase_session_id);
        active.encrypted_access_token = Set(encrypted_access_token);
        active.encrypted_refresh_token = Set(encrypted_refresh_token);
        active.access_expires_at = Set(tokens.expires_at);
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;

        Ok(RevocationClaim {
            id_hash: claim.id_hash.clone(),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            access_expires_at: tokens.expires_at,
            supabase_session_id,
            attempts: claim.attempts,
            user_id: claim.user_id,
        })
    }

    async fn confirm_revocation(&self, claim: &RevocationClaim) -> Result<(), AppError> {
        let transaction = self.begin_revocation_transaction().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if model.upstream_revoked_at.is_some()
            || model.revocation_abandoned_at.is_some()
            || model.revocation_attempts != claim.attempts
        {
            transaction.commit().await?;
            return Ok(());
        }
        let now = database_now(&transaction).await?;
        let mut active: web_session::ActiveModel = model.into();
        active.upstream_revoked_at = Set(Some(now));
        active.revocation_abandoned_at = Set(None);
        active.revocation_failure_kind = Set(None);
        active.revocation_pending_at = Set(None);
        active.revocation_next_attempt_at = Set(None);
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;
        tracing::info!(user_id = %claim.user_id, attempts = claim.attempts, "Supabase logout reconciled");
        Ok(())
    }

    async fn abandon_revocation(
        &self,
        claim: &RevocationClaim,
        failure_kind: &str,
    ) -> Result<(), AppError> {
        let transaction = self.begin_revocation_transaction().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if model.upstream_revoked_at.is_some()
            || model.revocation_abandoned_at.is_some()
            || model.revocation_attempts != claim.attempts
        {
            transaction.commit().await?;
            return Ok(());
        }
        let now = database_now(&transaction).await?;
        let mut active: web_session::ActiveModel = model.into();
        active.revocation_abandoned_at = Set(Some(now));
        active.revocation_failure_kind = Set(Some(failure_kind.to_owned()));
        active.revocation_pending_at = Set(None);
        active.revocation_next_attempt_at = Set(None);
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;
        tracing::error!(
            user_id = %claim.user_id,
            attempts = claim.attempts,
            failure_kind,
            "Supabase logout ended without a positive upstream acknowledgement"
        );
        Ok(())
    }

    async fn defer_revocation(
        &self,
        claim: &RevocationClaim,
        error: &AuthProviderError,
    ) -> Result<(), AppError> {
        let transaction = self.begin_revocation_transaction().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if model.upstream_revoked_at.is_some()
            || model.revocation_abandoned_at.is_some()
            || model.revocation_attempts != claim.attempts
        {
            transaction.commit().await?;
            return Ok(());
        }
        let now = database_now(&transaction).await?;
        let pending_at = model.revocation_pending_at.unwrap_or(now);
        let mut active: web_session::ActiveModel = model.into();
        active.revocation_pending_at = Set(Some(pending_at));
        active.revocation_next_attempt_at = Set(Some(now + revocation_retry_delay(claim.attempts)));
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;
        tracing::warn!(
            user_id = %claim.user_id,
            attempts = claim.attempts,
            error = %error,
            "Supabase logout queued for retry"
        );
        Ok(())
    }

    /// Refreshes an expiring session without holding a database connection or
    /// row lock across the upstream request. A transaction claims a durable,
    /// row-scoped lease and commits before network I/O; the lease UUID is then
    /// used as a fence when persisting rotated credentials. If a lease expires
    /// and another replica takes over, the old response cannot clobber the
    /// newer token pair.
    async fn refresh_fenced(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let local_lock = self
            .refresh_coordinator
            .lock_for(id_hash)
            .ok_or(AppError::AuthUpstream)?;

        let result = match local_lock.clone().try_lock_owned() {
            Ok(leader) => {
                let result = self.refresh_as_local_leader(id_hash).await;
                drop(leader);
                result
            }
            Err(_) => {
                // The local leader owns all cross-replica polling and upstream
                // I/O. Followers wait without a database connection, then do
                // exactly one authoritative reread of its durable outcome.
                let follower = local_lock.clone().lock_owned().await;
                let result = self.outcome_after_lost_refresh(id_hash).await;
                drop(follower);
                result
            }
        };
        drop(local_lock);
        self.refresh_coordinator.prune();
        result
    }

    async fn refresh_as_local_leader(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let wait_deadline = time::Instant::now() + REFRESH_WAIT;
        let mut poll_interval = REFRESH_POLL_INITIAL;
        loop {
            match self.claim_refresh(id_hash).await? {
                RefreshClaimResult::Fresh(model) => return Ok(context_from_model(*model)),
                RefreshClaimResult::Claimed(claim) => {
                    return self.execute_refresh_claim(claim).await;
                }
                RefreshClaimResult::Busy if time::Instant::now() < wait_deadline => {
                    let remaining = wait_deadline.saturating_duration_since(time::Instant::now());
                    time::sleep(poll_interval.min(remaining)).await;
                    poll_interval = next_refresh_poll_interval(poll_interval);
                }
                RefreshClaimResult::Busy => return Err(AppError::AuthUpstream),
            }
        }
    }

    async fn claim_refresh(&self, id_hash: &str) -> Result<RefreshClaimResult, AppError> {
        let transaction = self.db.begin().await?;
        let model = web_session::Entity::find_by_id(id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
            .ok_or(AppError::Unauthorized)?;
        self.validate_model(&model)?;
        let now = database_now(&transaction).await?;

        if model.access_expires_at > now + ChronoDuration::seconds(60) {
            transaction.commit().await?;
            return Ok(RefreshClaimResult::Fresh(Box::new(model)));
        }

        if refresh_lease_is_active(model.refresh_lease_id, model.refresh_lease_expires_at, now) {
            transaction.commit().await?;
            return Ok(RefreshClaimResult::Busy);
        }

        let refresh_token = match self.decrypt(&model.encrypted_refresh_token) {
            Ok(token) => token,
            Err(error) => {
                transaction.rollback().await?;
                return Err(error);
            }
        };
        let lease_id = uuid::Uuid::new_v4();
        let user_id = model.user_id;
        let supabase_session_id = model.supabase_session_id;
        let mut active: web_session::ActiveModel = model.into();
        active.refresh_lease_id = Set(Some(lease_id));
        active.refresh_lease_expires_at = Set(Some(now + REFRESH_LEASE));
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;

        Ok(RefreshClaimResult::Claimed(RefreshClaim {
            id_hash: id_hash.to_owned(),
            lease_id,
            user_id,
            supabase_session_id,
            refresh_token,
        }))
    }

    async fn execute_refresh_claim(&self, claim: RefreshClaim) -> Result<AuthContext, AppError> {
        let tokens = match self.auth.refresh(&claim.refresh_token).await {
            Ok(tokens) => tokens,
            Err(AuthProviderError::InvalidCredentials) => {
                return if self.finish_refresh_claim(&claim, true).await? {
                    Err(AppError::Unauthorized)
                } else {
                    self.outcome_after_lost_refresh(&claim.id_hash).await
                };
            }
            Err(error) => {
                return if self.finish_refresh_claim(&claim, false).await? {
                    Err(map_auth_error(error))
                } else {
                    self.outcome_after_lost_refresh(&claim.id_hash).await
                };
            }
        };
        if tokens.user.id != claim.user_id {
            return if self.finish_refresh_claim(&claim, true).await? {
                Err(AppError::Unauthorized)
            } else {
                self.outcome_after_lost_refresh(&claim.id_hash).await
            };
        }
        let observed_session_id = match validated_session_id(&tokens.access_token, tokens.user.id) {
            Ok(session_id) => session_id,
            Err(error) => {
                return if self.finish_refresh_claim(&claim, true).await? {
                    Err(map_auth_error(error))
                } else {
                    self.outcome_after_lost_refresh(&claim.id_hash).await
                };
            }
        };
        let supabase_session_id =
            match resolve_rotated_session_id(claim.supabase_session_id, observed_session_id) {
                Ok(session_id) => session_id,
                Err(error) => {
                    return if self.finish_refresh_claim(&claim, true).await? {
                        Err(map_auth_error(error))
                    } else {
                        self.outcome_after_lost_refresh(&claim.id_hash).await
                    };
                }
            };

        let encrypted_access_token = match self.encrypt(&tokens.access_token) {
            Ok(token) => token,
            Err(error) => {
                let _ = self.finish_refresh_claim(&claim, false).await;
                return Err(error);
            }
        };
        let encrypted_refresh_token = match self.encrypt(&tokens.refresh_token) {
            Ok(token) => token,
            Err(error) => {
                let _ = self.finish_refresh_claim(&claim, false).await;
                return Err(error);
            }
        };
        let transaction = self.db.begin().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Err(AppError::Unauthorized);
        };
        if model.refresh_lease_id != Some(claim.lease_id) {
            transaction.commit().await?;
            return self.outcome_after_lost_refresh(&claim.id_hash).await;
        }
        let now = database_now(&transaction).await?;
        if self.validate_model(&model).is_err() {
            let mut active: web_session::ActiveModel = model.into();
            active.refresh_lease_id = Set(None);
            active.refresh_lease_expires_at = Set(None);
            active.updated_at = Set(now);
            active.update(&transaction).await?;
            transaction.commit().await?;
            return Err(AppError::Unauthorized);
        }
        let mut active: web_session::ActiveModel = model.into();
        active.email = Set(tokens.user.email.clone().unwrap_or_default());
        active.supabase_session_id = Set(supabase_session_id);
        active.encrypted_access_token = Set(encrypted_access_token);
        active.encrypted_refresh_token = Set(encrypted_refresh_token);
        active.access_expires_at = Set(tokens.expires_at);
        active.refresh_lease_id = Set(None);
        active.refresh_lease_expires_at = Set(None);
        active.updated_at = Set(now);
        let updated = active.update(&transaction).await?;
        transaction.commit().await?;
        Ok(context_from_model(updated))
    }

    /// Clears or revokes a refresh claim only if its fence still owns the row.
    /// Returning `false` means a newer replica or logout won; callers must not
    /// apply any conclusion from the stale upstream response.
    async fn finish_refresh_claim(
        &self,
        claim: &RefreshClaim,
        revoke: bool,
    ) -> Result<bool, AppError> {
        let transaction = self.db.begin().await?;
        let Some(model) = web_session::Entity::find_by_id(&claim.id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        if model.refresh_lease_id != Some(claim.lease_id) {
            transaction.commit().await?;
            return Ok(false);
        }
        let now = database_now(&transaction).await?;
        if revoke {
            Self::mark_revocation(model, &transaction, now).await?;
        } else {
            let mut active: web_session::ActiveModel = model.into();
            active.refresh_lease_id = Set(None);
            active.refresh_lease_expires_at = Set(None);
            active.updated_at = Set(now);
            active.update(&transaction).await?;
        }
        transaction.commit().await?;
        Ok(true)
    }

    async fn outcome_after_lost_refresh(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let transaction = self.db.begin().await?;
        let model = web_session::Entity::find_by_id(id_hash)
            .one(&transaction)
            .await?
            .ok_or(AppError::Unauthorized)?;
        self.validate_model(&model)?;
        let now = database_now(&transaction).await?;
        transaction.commit().await?;
        if model.refresh_lease_id.is_none() && model.access_expires_at > now {
            Ok(context_from_model(model))
        } else {
            Err(AppError::AuthUpstream)
        }
    }

    fn validate_model(&self, model: &web_session::Model) -> Result<(), AppError> {
        if model.revoked_at.is_some() || model.expires_at <= Utc::now() {
            Err(AppError::Unauthorized)
        } else {
            Ok(())
        }
    }

    fn encrypt(&self, plaintext: &str) -> Result<String, AppError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AppError::Crypto)?;
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| AppError::Crypto)?;
        let mut encoded = nonce_bytes.to_vec();
        encoded.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(encoded))
    }

    fn decrypt(&self, encoded: &str) -> Result<String, AppError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| AppError::Crypto)?;
        let (nonce_bytes, ciphertext) = bytes.split_at_checked(12).ok_or(AppError::Crypto)?;
        let nonce = nonce_bytes.try_into().map_err(|_| AppError::Crypto)?;
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| AppError::Crypto)?;
        String::from_utf8(plaintext).map_err(|_| AppError::Crypto)
    }
}

fn revocation_retry_delay(attempts: i32) -> ChronoDuration {
    let exponent = attempts.clamp(1, 10) as u32;
    let seconds = 2_i64.saturating_pow(exponent).min(3_600);
    ChronoDuration::seconds(seconds)
}

fn refresh_lease_is_active(
    lease_id: Option<uuid::Uuid>,
    expires_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> bool {
    lease_id.is_some() && expires_at.is_some_and(|expires_at| expires_at > now)
}

fn next_refresh_poll_interval(current: Duration) -> Duration {
    current
        .checked_mul(2)
        .unwrap_or(REFRESH_POLL_MAX)
        .min(REFRESH_POLL_MAX)
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn hash_token(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn context_from_model(model: web_session::Model) -> AuthContext {
    AuthContext {
        user_id: model.user_id,
        email: model.email,
        source: CredentialSource::SessionCookie,
        supabase_session_id: model.supabase_session_id,
        session_hash: Some(model.id_hash),
        csrf_token: Some(model.csrf_token),
        expires_at: model.expires_at,
    }
}

fn map_auth_error(error: AuthProviderError) -> AppError {
    match error {
        AuthProviderError::InvalidCredentials => AppError::Unauthorized,
        AuthProviderError::Unavailable | AuthProviderError::InvalidResponse => {
            AppError::AuthUpstream
        }
    }
}

fn resolve_rotated_session_id(
    stored: Option<uuid::Uuid>,
    observed: Option<uuid::Uuid>,
) -> Result<Option<uuid::Uuid>, AuthProviderError> {
    match (stored, observed) {
        (Some(expected), Some(actual)) if expected != actual => {
            Err(AuthProviderError::InvalidResponse)
        }
        (Some(expected), _) => Ok(Some(expected)),
        (None, observed) => Ok(observed),
    }
}

fn bearer_expiry(token: &str) -> Option<chrono::DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let timestamp = value.get("exp")?.as_i64()?;
    chrono::TimeZone::timestamp_opt(&Utc, timestamp, 0).single()
}

#[cfg(test)]
mod tests {
    use super::{
        next_refresh_poll_interval, refresh_lease_is_active, RefreshClaimResult,
        RefreshCoordinator, SessionRevoker, SessionService, MAX_REFRESH_COORDINATORS,
        REFRESH_POLL_INITIAL, REFRESH_POLL_MAX, REVOCATION_LEASE,
    };
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use canonical_auth::{AuthProvider, AuthProviderError, AuthTokens, SupabaseUser};
    use canonical_store::{entity::web_session, migration::Migrator};
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, Database, DatabaseConnection, EntityTrait};
    use sea_orm_migration::MigratorTrait;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    use uuid::Uuid;

    #[test]
    fn revocation_lease_exceeds_the_worst_case_post_claim_budget() {
        // Three 10-second auth requests and two 10-second pool acquisitions
        // are possible on the 401-refresh-retry branch.
        let worst_case = ChronoDuration::seconds((3 * 10) + (2 * 10));
        assert!(REVOCATION_LEASE > worst_case);
    }

    #[test]
    fn refresh_lease_uses_an_explicit_shared_clock() {
        let now = Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap();
        let lease_id = Uuid::from_u128(1);

        assert!(refresh_lease_is_active(
            Some(lease_id),
            Some(now + ChronoDuration::seconds(1)),
            now,
        ));
        assert!(!refresh_lease_is_active(Some(lease_id), Some(now), now,));
        assert!(!refresh_lease_is_active(
            Some(lease_id),
            Some(now - ChronoDuration::seconds(1)),
            now,
        ));
        assert!(!refresh_lease_is_active(
            None,
            Some(now + ChronoDuration::seconds(1)),
            now,
        ));
        assert!(!refresh_lease_is_active(Some(lease_id), None, now));
    }

    #[test]
    fn refresh_polling_backs_off_to_a_bounded_interval() {
        let mut interval = REFRESH_POLL_INITIAL;
        for _ in 0..16 {
            let next = next_refresh_poll_interval(interval);
            assert!(next >= interval);
            assert!(next <= REFRESH_POLL_MAX);
            interval = next;
        }
        assert_eq!(interval, REFRESH_POLL_MAX);
    }

    #[test]
    fn refresh_coordinator_reuses_live_locks_and_drops_stale_entries() {
        let coordinator = RefreshCoordinator::default();
        let first = coordinator.lock_for("same-session").unwrap();
        let follower = coordinator.lock_for("same-session").unwrap();
        assert!(Arc::ptr_eq(&first, &follower));
        assert_eq!(coordinator.entry_count(), 1);

        drop(first);
        drop(follower);
        coordinator.prune();
        assert_eq!(coordinator.entry_count(), 0);
    }

    #[test]
    fn refresh_coordinator_fails_closed_at_its_live_entry_bound() {
        let coordinator = RefreshCoordinator::default();
        let locks = (0..MAX_REFRESH_COORDINATORS)
            .map(|index| coordinator.lock_for(&format!("session-{index}")).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(coordinator.entry_count(), MAX_REFRESH_COORDINATORS);
        assert!(coordinator.lock_for("overflow").is_none());

        drop(locks);
        coordinator.prune();
        assert_eq!(coordinator.entry_count(), 0);
    }

    #[derive(Clone, Copy)]
    enum LogoutScenario {
        Success,
        UnauthorizedOnce,
        UnavailableOnce,
        InvalidRefresh,
        ChangedSessionId,
    }

    struct RetryAuth {
        user_id: Uuid,
        rotated: AuthTokens,
        expected_refresh_token: String,
        scenario: LogoutScenario,
        refresh_calls: AtomicUsize,
        sign_out_calls: AtomicUsize,
        signed_out_tokens: Mutex<Vec<String>>,
    }

    impl RetryAuth {
        fn new(user_id: Uuid, session_id: Uuid, scenario: LogoutScenario) -> Arc<Self> {
            Arc::new(Self {
                user_id,
                rotated: tokens(
                    user_id,
                    session_id,
                    "rotated-refresh-token",
                    Utc::now() + ChronoDuration::hours(1),
                ),
                expected_refresh_token: "initial-refresh-token".into(),
                scenario,
                refresh_calls: AtomicUsize::new(0),
                sign_out_calls: AtomicUsize::new(0),
                signed_out_tokens: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl AuthProvider for RetryAuth {
        async fn password_sign_in(
            &self,
            _email: &str,
            _password: &str,
        ) -> Result<AuthTokens, AuthProviderError> {
            Err(AuthProviderError::InvalidCredentials)
        }

        async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            if refresh_token != self.expected_refresh_token {
                return Err(AuthProviderError::InvalidCredentials);
            }
            if matches!(self.scenario, LogoutScenario::InvalidRefresh) {
                return Err(AuthProviderError::InvalidCredentials);
            }
            Ok(self.rotated.clone())
        }

        async fn user_for_token(
            &self,
            _access_token: &str,
        ) -> Result<SupabaseUser, AuthProviderError> {
            Ok(SupabaseUser {
                id: self.user_id,
                email: Some("user@example.com".into()),
            })
        }

        async fn sign_out(&self, access_token: &str) -> Result<(), AuthProviderError> {
            self.signed_out_tokens
                .lock()
                .unwrap()
                .push(access_token.to_owned());
            let call = self.sign_out_calls.fetch_add(1, Ordering::SeqCst);
            match (self.scenario, call) {
                (LogoutScenario::UnauthorizedOnce, 0) => Err(AuthProviderError::InvalidCredentials),
                (LogoutScenario::UnavailableOnce, 0) => Err(AuthProviderError::Unavailable),
                _ => Ok(()),
            }
        }
    }

    fn jwt(user_id: Uuid, session_id: Uuid) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "sub": user_id,
                "session_id": session_id,
                "exp": (Utc::now() + ChronoDuration::hours(1)).timestamp()
            })
            .to_string(),
        );
        format!("{header}.{payload}.test-signature")
    }

    fn tokens(
        user_id: Uuid,
        session_id: Uuid,
        refresh_token: &str,
        expires_at: chrono::DateTime<Utc>,
    ) -> AuthTokens {
        AuthTokens {
            access_token: jwt(user_id, session_id),
            refresh_token: refresh_token.into(),
            expires_at,
            user: SupabaseUser {
                id: user_id,
                email: Some("user@example.com".into()),
            },
        }
    }

    async fn setup(
        scenario: LogoutScenario,
        access_expires_at: chrono::DateTime<Utc>,
    ) -> (
        DatabaseConnection,
        SessionService,
        Arc<RetryAuth>,
        String,
        Uuid,
        Uuid,
    ) {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let user_id = Uuid::new_v4();
        let original_session_id = Uuid::new_v4();
        let rotated_session_id = if matches!(scenario, LogoutScenario::ChangedSessionId) {
            Uuid::new_v4()
        } else {
            original_session_id
        };
        let auth = RetryAuth::new(user_id, rotated_session_id, scenario);
        let sessions = SessionService::new(
            db.clone(),
            auth.clone(),
            &[7; 32],
            Duration::from_secs(3_600),
        )
        .unwrap();
        let created = sessions
            .create(tokens(
                user_id,
                original_session_id,
                "initial-refresh-token",
                access_expires_at,
            ))
            .await
            .unwrap();
        (
            db,
            sessions,
            auth,
            created.raw_id,
            original_session_id,
            rotated_session_id,
        )
    }

    async fn stored_session(db: &DatabaseConnection, raw_id: &str) -> web_session::Model {
        let id_hash = super::hash_token(raw_id);
        web_session::Entity::find_by_id(id_hash)
            .one(db)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn expired_access_is_refreshed_and_rotation_is_persisted_before_logout() {
        let (db, sessions, auth, raw_id, original_session_id, rotated_session_id) = setup(
            LogoutScenario::Success,
            Utc::now() - ChronoDuration::minutes(1),
        )
        .await;
        assert_eq!(
            stored_session(&db, &raw_id).await.supabase_session_id,
            Some(original_session_id)
        );

        sessions.revoke(&raw_id).await.unwrap();

        let stored = stored_session(&db, &raw_id).await;
        assert_eq!(stored.supabase_session_id, Some(rotated_session_id));
        assert!(stored.upstream_revoked_at.is_some());
        assert!(stored.revocation_pending_at.is_none());
        assert_eq!(stored.revocation_attempts, 1);
        assert_eq!(auth.refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(auth.sign_out_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            sessions.decrypt(&stored.encrypted_refresh_token).unwrap(),
            "rotated-refresh-token"
        );
        assert_eq!(
            sessions.decrypt(&stored.encrypted_access_token).unwrap(),
            auth.rotated.access_token
        );
        let signed_out_tokens = auth.signed_out_tokens.lock().unwrap();
        assert_eq!(signed_out_tokens.len(), 1);
        assert_eq!(signed_out_tokens[0], auth.rotated.access_token);
    }

    #[tokio::test]
    async fn unauthorized_logout_refreshes_and_retries_instead_of_confirming() {
        let (db, sessions, auth, raw_id, _original_session_id, rotated_session_id) = setup(
            LogoutScenario::UnauthorizedOnce,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;

        sessions.revoke(&raw_id).await.unwrap();

        let stored = stored_session(&db, &raw_id).await;
        let calls = auth.signed_out_tokens.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], auth.rotated.access_token);
        assert_eq!(auth.refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stored.supabase_session_id, Some(rotated_session_id));
        assert!(stored.upstream_revoked_at.is_some());
    }

    #[tokio::test]
    async fn failed_logout_is_retried_from_the_durable_queue() {
        let (db, sessions, auth, raw_id, _original_session_id, _rotated_session_id) = setup(
            LogoutScenario::UnavailableOnce,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;

        sessions.revoke(&raw_id).await.unwrap();
        let first_attempt = stored_session(&db, &raw_id).await;
        assert!(first_attempt.upstream_revoked_at.is_none());
        assert!(first_attempt.revocation_pending_at.is_some());
        assert_eq!(first_attempt.revocation_attempts, 1);

        let mut active: web_session::ActiveModel = first_attempt.into();
        active.revocation_next_attempt_at = Set(Some(Utc::now() - ChronoDuration::seconds(1)));
        active.update(&db).await.unwrap();

        // Reconstructing the service exercises the persisted queue rather than
        // relying on any in-memory retry state from the original request.
        let restarted = SessionRevoker::new(db.clone(), auth.clone(), &[7; 32]).unwrap();
        restarted.reconcile_once().await.unwrap();

        let stored = stored_session(&db, &raw_id).await;
        assert!(stored.upstream_revoked_at.is_some());
        assert!(stored.revocation_pending_at.is_none());
        assert_eq!(stored.revocation_attempts, 2);
        assert_eq!(auth.sign_out_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rejected_credentials_are_not_recorded_as_positive_logout() {
        let (db, sessions, _auth, raw_id, _session_id, _rotated_session_id) = setup(
            LogoutScenario::InvalidRefresh,
            Utc::now() - ChronoDuration::minutes(1),
        )
        .await;

        sessions.revoke(&raw_id).await.unwrap();

        let stored = stored_session(&db, &raw_id).await;
        assert!(stored.upstream_revoked_at.is_none());
        assert!(stored.revocation_abandoned_at.is_some());
        assert_eq!(
            stored.revocation_failure_kind.as_deref(),
            Some("credentials_rejected")
        );
        assert!(stored.revocation_pending_at.is_none());
    }

    #[tokio::test]
    async fn locally_revoked_supabase_session_rejects_still_valid_bearer() {
        let (db, sessions, _auth, raw_id, session_id, _rotated_session_id) = setup(
            LogoutScenario::Success,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;
        let user_id = stored_session(&db, &raw_id).await.user_id;
        let access_token = jwt(user_id, session_id);

        sessions.revoke(&raw_id).await.unwrap();

        assert!(matches!(
            sessions.authenticate_bearer(&access_token).await,
            Err(super::SessionError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn legacy_null_session_identity_is_backfilled_before_failed_logout() {
        let (db, sessions, auth, raw_id, session_id, _rotated_session_id) = setup(
            LogoutScenario::UnavailableOnce,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;
        let stored_before_logout = stored_session(&db, &raw_id).await;
        let access_token = sessions
            .decrypt(&stored_before_logout.encrypted_access_token)
            .unwrap();

        // Simulate a row created before supabase_session_id was introduced.
        let mut legacy: web_session::ActiveModel = stored_before_logout.into();
        legacy.supabase_session_id = Set(None);
        legacy.update(&db).await.unwrap();

        sessions.revoke(&raw_id).await.unwrap();

        let stored = stored_session(&db, &raw_id).await;
        assert_eq!(stored.supabase_session_id, Some(session_id));
        assert!(stored.revoked_at.is_some());
        assert!(stored.upstream_revoked_at.is_none());
        assert!(stored.revocation_pending_at.is_some());
        assert_eq!(auth.refresh_calls.load(Ordering::SeqCst), 0);
        assert_eq!(auth.sign_out_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            sessions.authenticate_bearer(&access_token).await,
            Err(super::SessionError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn bearer_heartbeat_revalidation_detects_local_revocation() {
        let (db, sessions, _auth, raw_id, session_id, _rotated_session_id) = setup(
            LogoutScenario::Success,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;
        let stored_before_logout = stored_session(&db, &raw_id).await;
        let access_token = sessions
            .decrypt(&stored_before_logout.encrypted_access_token)
            .unwrap();
        let open_bearer_context = sessions.authenticate_bearer(&access_token).await.unwrap();
        assert_eq!(open_bearer_context.supabase_session_id, Some(session_id));

        sessions.revoke(&raw_id).await.unwrap();

        assert!(matches!(
            sessions.revalidate(&open_bearer_context).await,
            Err(super::SessionError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn refresh_cannot_change_the_supabase_session_identity() {
        let (db, sessions, _auth, raw_id, original_session_id, rotated_session_id) = setup(
            LogoutScenario::ChangedSessionId,
            Utc::now() - ChronoDuration::minutes(1),
        )
        .await;
        assert_ne!(original_session_id, rotated_session_id);

        assert!(matches!(
            sessions.authenticate(&raw_id).await,
            Err(super::SessionError::AuthUpstream)
        ));
        let stored = stored_session(&db, &raw_id).await;
        assert_eq!(stored.supabase_session_id, Some(original_session_id));
        assert!(stored.revoked_at.is_some());
        assert!(stored.revocation_pending_at.is_some());
    }

    #[tokio::test]
    async fn expired_refresh_claim_cannot_overwrite_a_newer_lease_holder() {
        let (db, sessions, _auth, raw_id, _session_id, _rotated_session_id) = setup(
            LogoutScenario::Success,
            Utc::now() - ChronoDuration::minutes(1),
        )
        .await;
        let id_hash = super::hash_token(&raw_id);
        let stale_claim = match sessions.claim_refresh(&id_hash).await.unwrap() {
            RefreshClaimResult::Claimed(claim) => claim,
            _ => panic!("expired session should acquire a refresh claim"),
        };

        // Model a second replica taking over after lease expiry and committing
        // a newer token pair before the delayed first response arrives.
        let stored = stored_session(&db, &raw_id).await;
        let mut active: web_session::ActiveModel = stored.into();
        active.refresh_lease_id = Set(Some(Uuid::new_v4()));
        active.refresh_lease_expires_at = Set(Some(Utc::now() + ChronoDuration::seconds(30)));
        active.encrypted_access_token = Set(sessions.encrypt("newer-access-token").unwrap());
        active.encrypted_refresh_token = Set(sessions.encrypt("newer-refresh-token").unwrap());
        active.access_expires_at = Set(Utc::now() + ChronoDuration::hours(1));
        active.update(&db).await.unwrap();

        assert!(matches!(
            sessions.execute_refresh_claim(stale_claim).await,
            Err(super::SessionError::AuthUpstream)
        ));
        let stored = stored_session(&db, &raw_id).await;
        assert_eq!(
            sessions.decrypt(&stored.encrypted_refresh_token).unwrap(),
            "newer-refresh-token"
        );
        assert_eq!(
            sessions.decrypt(&stored.encrypted_access_token).unwrap(),
            "newer-access-token"
        );
    }
}
