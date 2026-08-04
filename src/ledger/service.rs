use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::customer_locks::{
    CustomerLockBroker, CustomerLockGuard, customer_lock_targets_from_account_code,
};
use crate::db::{decode_enum, require_row, stmt};
use crate::entity::accounts;
use crate::error::{AppError, AppResult};
use crate::events::EventBus;
use crate::financial_audit::{
    AuditedLedgerPost, FinancialOperationContext, load_ledger_post, record_ledger_post,
};
use crate::money::Currency;
use crate::shard::{Region, ShardKey};

use super::fingerprint::{
    LedgerIntent, LedgerPostingIntent, ReplayFingerprint, classify_replay, intent_fingerprint,
};
use super::types::*;

#[derive(Clone)]
pub struct LedgerService {
    pool: DatabaseConnection,
    customer_locks: CustomerLockBroker,
    events: Arc<EventBus>,
}

impl LedgerService {
    pub fn new(
        pool: DatabaseConnection,
        customer_locks: CustomerLockBroker,
        events: Arc<EventBus>,
    ) -> Self {
        Self {
            pool,
            customer_locks,
            events,
        }
    }

    /// Ensure a "system" account exists for the tenant, e.g. `clearing/stripe`,
    /// `cash/chase_main`, `revenue/saas`, `fees/stripe`. Idempotent.
    pub async fn ensure_account(
        &self,
        tenant_id: Uuid,
        region: Region,
        user_id: Option<Uuid>,
        kind: AccountKind,
        code: &str,
        currency: Currency,
    ) -> AppResult<Account> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let normal_side = kind.normal_side();
        let currency_str = currency.as_str().to_string();

        // Try fetch first; create if missing.
        if let Some(acct) = accounts::Entity::find()
            .filter(accounts::Column::TenantId.eq(tenant_id))
            .filter(accounts::Column::Code.eq(code))
            .filter(accounts::Column::Currency.eq(currency_str.clone()))
            .one(&self.pool)
            .await?
        {
            return model_to_account(acct);
        }

        // INSERT ... ON CONFLICT (tenant_id, code, currency)
        //        DO UPDATE SET code = EXCLUDED.code
        // RETURNING <all columns>, matching the previous hand-written upsert.
        // The entity's save_as casts write the enum labels through
        // `::account_kind` / `::account_normal_side` exactly as before.
        let model = accounts::Entity::insert(accounts::ActiveModel {
            tenant_id: Set(tenant_id),
            shard_key: Set(shard),
            user_id: Set(user_id),
            kind: Set(kind_to_str(kind).to_string()),
            normal_side: Set(normal_side_to_str(normal_side).to_string()),
            code: Set(code.to_string()),
            currency: Set(currency_str),
            ..Default::default()
        })
        .on_conflict(
            OnConflict::columns([
                accounts::Column::TenantId,
                accounts::Column::Code,
                accounts::Column::Currency,
            ])
            .update_column(accounts::Column::Code)
            .to_owned(),
        )
        .exec_with_returning(&self.pool)
        .await?;

        model_to_account(model)
    }

    /// Post a draft transaction. Atomic, idempotent, zero-sum-checked, and
    /// transactionally attributed to the authenticated financial actor.
    ///
    /// * The DB's deferred constraint trigger enforces the zero-sum invariant
    ///   per currency at COMMIT time.
    /// * Idempotency: a repeat with the same `(tenant_id, idempotency_key)`
    ///   returns the existing transaction id only when its canonical business
    ///   intent matches. Reusing the key for different or unverifiable legacy
    ///   intent fails closed.
    /// * The accepted-operation audit row is inserted on the same SeaORM
    ///   transaction as the header and postings. Attribution failure rolls the
    ///   entire financial mutation back.
    pub async fn post_transaction(
        &self,
        draft: &DraftTransaction,
        region: Region,
        operation_context: &FinancialOperationContext,
    ) -> AppResult<AuditedLedgerPost> {
        let customer_lock_targets = customer_lock_targets_from_draft(draft);
        let mut customer_lock_guard = self
            .customer_locks
            .acquire_customers(
                draft.tenant_id,
                customer_lock_targets,
                "ledger.post_transaction",
            )
            .await?;

        let result = self
            .post_transaction_locked(
                draft,
                region,
                &mut customer_lock_guard,
                operation_context,
            )
            .await;
        if let Err(e) = customer_lock_guard.release().await {
            tracing::warn!(error = %e, "failed to release customer ledger-write lock");
        }

        // Publish the domain event AFTER releasing the distributed customer
        // lock. async-nats `publish().await` writes to a bounded in-process
        // channel that can suspend under broker backpressure; doing it under
        // the lock would hold a cross-pod live-mutex resource across a network
        // op (and risk TTL expiry mid-hold). The event is best-effort and not
        // part of correctness, so it belongs outside the critical section.
        let posted = result?;
        if !posted.replayed {
            self.publish_posting_event(draft, posted.transaction_id, region)
                .await;
        }
        Ok(posted)
    }

    /// Build the redacted per-currency debit totals and publish the committed
    /// transaction event. Best-effort; never called for idempotent replays.
    async fn publish_posting_event(&self, draft: &DraftTransaction, tx_id: Uuid, region: Region) {
        let mut debit_totals: HashMap<String, i128> = HashMap::new();
        for p in &draft.postings {
            if matches!(p.direction, Direction::Debit) {
                *debit_totals.entry(p.currency.clone()).or_insert(0) += p.amount_minor;
            }
        }
        let totals = serde_json::Value::Object(
            debit_totals
                .into_iter()
                .map(|(cur, amt)| (cur, serde_json::Value::String(amt.to_string())))
                .collect(),
        );
        self.events
            .publish_ledger_posting(
                draft.tenant_id,
                tx_id,
                &draft.kind,
                &draft.idempotency_key,
                draft.postings.len(),
                totals,
                region.tag(),
            )
            .await;
    }

    /// Returns the committed transaction plus its original durable audit
    /// identity. Idempotent replays never append a second accepted event.
    async fn post_transaction_locked(
        &self,
        draft: &DraftTransaction,
        region: Region,
        customer_lock_guard: &mut CustomerLockGuard,
        operation_context: &FinancialOperationContext,
    ) -> AppResult<AuditedLedgerPost> {
        if draft.postings.len() < 2 {
            return Err(AppError::LedgerInvariant(
                "transaction must contain at least 2 postings".into(),
            ));
        }

        // Pre-flight: zero-sum per currency, in-memory, before we hit the DB.
        let mut net_by_currency: HashMap<String, i128> = HashMap::new();
        for p in &draft.postings {
            let signed = match p.direction {
                Direction::Debit => p.amount_minor,
                Direction::Credit => -p.amount_minor,
            };
            *net_by_currency.entry(p.currency.clone()).or_insert(0) += signed;
        }
        for (cur, net) in &net_by_currency {
            if *net != 0 {
                return Err(AppError::LedgerInvariant(format!(
                    "transaction not balanced in {cur}: net={net}"
                )));
            }
        }

        let incoming_fingerprint = intent_fingerprint(&intent_from_draft(draft));
        let shard = ShardKey::derive(draft.tenant_id, region).0;

        let tx = self.pool.begin().await?;

        // Serialize concurrent calls with the same (tenant_id,
        // idempotency_key). Two callers racing this code path used to
        // be able to both see "no existing row", both insert (one wins
        // via ON CONFLICT), but the loser could return the winning
        // tx_id *before* the winner had finished inserting postings —
        // so a subsequent read would see a transaction with no
        // postings. Holding a transaction-scoped advisory lock keyed
        // on (tenant_id, idempotency_key) means the second caller
        // waits until the first commits, at which point either it
        // also commits or returns the existing id.
        //
        // Use the two-int form `pg_advisory_xact_lock(tenant_lo,
        // tenant_hi XOR key_hash)` so the lock space is partitioned
        // by tenant — keeps the global lock count linear in tenants
        // and prevents one noisy tenant from starving others.
        //
        // SeaORM note: this MUST stay a raw Statement executed on the
        // transaction handle — the advisory lock is scoped to the
        // transaction's connection.
        let key_hash = idempotency_lock_hash(&draft.idempotency_key);
        let tenant_lo = (draft.tenant_id.as_u128() & 0xFFFF_FFFF) as i32;
        let tenant_hi = ((draft.tenant_id.as_u128() >> 32) & 0xFFFF_FFFF) as i32;
        tx.execute(stmt(
            "SELECT pg_advisory_xact_lock($1, $2)",
            [(tenant_lo ^ tenant_hi).into(), key_hash.into()],
        ))
        .await?;

        // Idempotency short-circuit. With the advisory lock above, the winner
        // has fully committed (or rolled back) before we read. Compare the
        // canonical intent while still holding that same lock: a key identifies
        // one business operation, not merely whichever row arrived first.
        if let Some(row) = tx
            .query_one(stmt(
                r#"SELECT id, intent_fingerprint FROM transactions
               WHERE tenant_id = $1 AND idempotency_key = $2"#,
                [draft.tenant_id.into(), draft.idempotency_key.clone().into()],
            ))
            .await?
        {
            let existing: Uuid = row.try_get("", "id")?;
            let existing_fingerprint: String = row.try_get("", "intent_fingerprint")?;
            require_matching_replay(
                &draft.idempotency_key,
                &existing_fingerprint,
                &incoming_fingerprint,
            )?;
            let audit = load_ledger_post(&tx, draft.tenant_id, existing).await?;
            customer_lock_guard.fence(&tx, draft.tenant_id).await?;
            tx.commit().await?;
            return Ok(AuditedLedgerPost {
                transaction_id: existing,
                audit,
                replayed: true,
            });
        }

        let proposed_tx_id = Uuid::new_v4();
        let inserted_tx_id: Option<Uuid> = tx
            .query_one(stmt(
                r#"
            INSERT INTO transactions
                (id, tenant_id, shard_key, kind, idempotency_key,
                 intent_fingerprint, description, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
            RETURNING id
            "#,
                [
                    proposed_tx_id.into(),
                    draft.tenant_id.into(),
                    shard.into(),
                    draft.kind.clone().into(),
                    draft.idempotency_key.clone().into(),
                    incoming_fingerprint.clone().into(),
                    draft.description.clone().into(),
                    draft.metadata.clone().into(),
                ],
            ))
            .await?
            .map(|row| row.try_get("", "id"))
            .transpose()?;

        let Some(tx_id) = inserted_tx_id else {
            let row = require_row(
                tx.query_one(stmt(
                    r#"SELECT id, intent_fingerprint FROM transactions
                   WHERE tenant_id = $1 AND idempotency_key = $2"#,
                    [draft.tenant_id.into(), draft.idempotency_key.clone().into()],
                ))
                .await?,
            )?;
            let existing: Uuid = row.try_get("", "id")?;
            let existing_fingerprint: String = row.try_get("", "intent_fingerprint")?;
            require_matching_replay(
                &draft.idempotency_key,
                &existing_fingerprint,
                &incoming_fingerprint,
            )?;
            let audit = load_ledger_post(&tx, draft.tenant_id, existing).await?;
            customer_lock_guard.fence(&tx, draft.tenant_id).await?;
            tx.commit().await?;
            return Ok(AuditedLedgerPost {
                transaction_id: existing,
                audit,
                replayed: true,
            });
        };

        for p in &draft.postings {
            // Resolve account by code (per-tenant, per-currency unique).
            let acct_id: Uuid = tx
                .query_one(stmt(
                    r#"SELECT id FROM accounts
                   WHERE tenant_id = $1 AND code = $2 AND currency = $3"#,
                    [
                        draft.tenant_id.into(),
                        p.account_code.clone().into(),
                        p.currency.clone().into(),
                    ],
                ))
                .await?
                .map(|row| row.try_get("", "id"))
                .transpose()?
                .ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "account not found for code={} currency={}",
                        p.account_code, p.currency
                    ))
                })?;

            // Bind amount as text and cast to NUMERIC in SQL — i128 covers the
            // full numeric(38, 0) domain, which rust_decimal's Decimal cannot.
            let amount_text = p.amount_minor.to_string();

            tx.execute(stmt(
                r#"
                INSERT INTO postings
                    (transaction_id, tenant_id, shard_key, account_id, direction,
                     amount_minor, currency, source, source_event_id, metadata)
                VALUES ($1, $2, $3, $4, $5::posting_direction,
                        ($6)::NUMERIC(38, 0), $7, $8, $9, $10)
                "#,
                [
                    tx_id.into(),
                    draft.tenant_id.into(),
                    shard.into(),
                    acct_id.into(),
                    direction_to_str(p.direction).into(),
                    amount_text.into(),
                    p.currency.clone().into(),
                    p.source.clone().into(),
                    p.source_event_id.clone().into(),
                    p.metadata.clone().into(),
                ],
            ))
            .await
            .map_err(map_pg_constraint_err)?;
        }

        let audit = record_ledger_post(
            &tx,
            draft.tenant_id,
            tx_id,
            operation_context,
            &draft.idempotency_key,
        )
        .await?;

        customer_lock_guard.fence(&tx, draft.tenant_id).await?;
        tx.commit().await?;

        // Genuinely-new commit (the two idempotent short-circuits above
        // returned early). The caller publishes the event AFTER releasing the
        // customer lock — see `post_transaction`.
        Ok(AuditedLedgerPost {
            transaction_id: tx_id,
            audit,
            replayed: false,
        })
    }

    pub async fn account_balance(
        &self,
        tenant_id: Uuid,
        account_code: &str,
        currency: Currency,
    ) -> AppResult<AccountBalance> {
        let cur = currency.as_str().to_string();

        let row = self
            .pool
            .query_one(stmt(
                r#"
            SELECT a.id,
                   a.normal_side,
                   COALESCE(SUM(
                       CASE
                           WHEN a.normal_side = 'debit'  AND p.direction = 'debit'  THEN  p.amount_minor
                           WHEN a.normal_side = 'debit'  AND p.direction = 'credit' THEN -p.amount_minor
                           WHEN a.normal_side = 'credit' AND p.direction = 'credit' THEN  p.amount_minor
                           WHEN a.normal_side = 'credit' AND p.direction = 'debit'  THEN -p.amount_minor
                       END
                   ), 0)::TEXT AS balance_text
            FROM accounts a
            LEFT JOIN postings p ON p.account_id = a.id
            WHERE a.tenant_id = $1 AND a.code = $2 AND a.currency = $3
            GROUP BY a.id, a.normal_side
            "#,
                [tenant_id.into(), account_code.into(), cur.clone().into()],
            ))
            .await?
            .ok_or_else(|| AppError::NotFound(format!("account {account_code}/{cur}")))?;

        let account_id: Uuid = row.try_get("", "id")?;
        let balance_text: String = row.try_get("", "balance_text")?;
        // A balance we cannot parse is an integrity failure, not a zero
        // balance: silently reporting 0 in a financial read path could mask a
        // real position. Surface it instead of defaulting.
        let balance_minor: i128 = balance_text.parse().map_err(|e| {
            AppError::LedgerInvariant(format!(
                "account {account_code}/{cur} balance {balance_text:?} is not a valid integer: {e}"
            ))
        })?;

        Ok(AccountBalance {
            account_id,
            account_code: account_code.to_string(),
            currency,
            balance_minor,
            as_of: Utc::now(),
        })
    }

    /// Sum balance across a set of account codes for a single tenant + currency.
    /// Used by AR aging, AP aging, and per-user roll-ups.
    pub async fn sum_balances(
        &self,
        tenant_id: Uuid,
        account_code_like: &str,
        currency: Currency,
    ) -> AppResult<i128> {
        let cur = currency.as_str().to_string();
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            SELECT COALESCE(SUM(
                CASE
                    WHEN a.normal_side = 'debit'  AND p.direction = 'debit'  THEN  p.amount_minor
                    WHEN a.normal_side = 'debit'  AND p.direction = 'credit' THEN -p.amount_minor
                    WHEN a.normal_side = 'credit' AND p.direction = 'credit' THEN  p.amount_minor
                    WHEN a.normal_side = 'credit' AND p.direction = 'debit'  THEN -p.amount_minor
                END
            ), 0)::TEXT AS net
            FROM accounts a
            JOIN postings p ON p.account_id = a.id
            WHERE a.tenant_id = $1 AND a.code LIKE $2 AND a.currency = $3
            "#,
                    [tenant_id.into(), account_code_like.into(), cur.into()],
                ))
                .await?,
        )?;

        let net_text: String = row.try_get("", "net")?;
        // As in `account_balance`: propagate a parse failure rather than
        // silently reporting a zero roll-up.
        net_text.parse().map_err(|e| {
            AppError::LedgerInvariant(format!(
                "summed balance {net_text:?} is not a valid integer: {e}"
            ))
        })
    }
}

fn model_to_account(m: accounts::Model) -> AppResult<Account> {
    Ok(Account {
        id: m.id,
        tenant_id: m.tenant_id,
        shard_key: m.shard_key,
        user_id: m.user_id,
        kind: decode_enum("kind", &m.kind)?,
        normal_side: decode_enum("normal_side", &m.normal_side)?,
        code: m.code,
        currency: Currency::new(&m.currency).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        metadata: m.metadata,
        created_at: m.created_at.with_timezone(&Utc),
    })
}

fn kind_to_str(k: AccountKind) -> &'static str {
    match k {
        AccountKind::Asset => "asset",
        AccountKind::Liability => "liability",
        AccountKind::Income => "income",
        AccountKind::Expense => "expense",
        AccountKind::Receivable => "receivable",
    }
}

fn normal_side_to_str(s: NormalSide) -> &'static str {
    match s {
        NormalSide::Debit => "debit",
        NormalSide::Credit => "credit",
    }
}

fn direction_to_str(d: Direction) -> &'static str {
    match d {
        Direction::Debit => "debit",
        Direction::Credit => "credit",
    }
}

fn intent_from_draft(draft: &DraftTransaction) -> LedgerIntent {
    LedgerIntent {
        tenant_id: draft.tenant_id,
        kind: draft.kind.clone(),
        description: draft.description.clone(),
        metadata: draft.metadata.clone(),
        postings: draft
            .postings
            .iter()
            .map(|posting| LedgerPostingIntent {
                account_code: posting.account_code.clone(),
                direction: direction_to_str(posting.direction).into(),
                amount_minor: posting.amount_minor,
                currency: posting.currency.clone(),
                source: posting.source.clone(),
                source_event_id: posting.source_event_id.clone(),
                metadata: posting.metadata.clone(),
            })
            .collect(),
    }
}

fn require_matching_replay(idempotency_key: &str, existing: &str, incoming: &str) -> AppResult<()> {
    match classify_replay(existing, incoming) {
        ReplayFingerprint::Exact => Ok(()),
        ReplayFingerprint::Legacy => Err(AppError::Conflict(format!(
            "idempotency key {idempotency_key:?} belongs to a legacy transaction whose intent \
             cannot be verified; submit the operation with a new key"
        ))),
        ReplayFingerprint::Mismatch => Err(AppError::Conflict(format!(
            "idempotency key {idempotency_key:?} was reused with different ledger intent"
        ))),
    }
}

fn map_pg_constraint_err(e: sea_orm::DbErr) -> AppError {
    if let Some(sea_orm::SqlErr::UniqueConstraintViolation(message)) = e.sql_err() {
        return AppError::Conflict(format!(
            "posting already exists (idempotent replay): {message}"
        ));
    }
    AppError::Database(e)
}

/// Stable 32-bit hash of the idempotency key, used as the second
/// argument to `pg_advisory_xact_lock(tenant_part, key_part)`.
///
/// We use the FNV-1a variant because:
///   * it's allocation-free and tiny (no extra dep)
///   * Postgres advisory locks take `int4`, so we explicitly want a
///     32-bit output rather than a SipHash 64-bit value we'd have to
///     truncate
///   * collisions on this hash just mean two unrelated idempotency
///     keys for the same tenant block each other — annoying but not
///     correctness-breaking
fn idempotency_lock_hash(key: &str) -> i32 {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut h: u32 = FNV_OFFSET;
    for b in key.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h as i32
}

fn customer_lock_targets_from_draft(draft: &DraftTransaction) -> Vec<String> {
    let mut targets = draft
        .postings
        .iter()
        .filter_map(|posting| customer_lock_targets_from_account_code(&posting.account_code))
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_lock_hash_is_deterministic() {
        assert_eq!(
            idempotency_lock_hash("stripe:bt_123"),
            idempotency_lock_hash("stripe:bt_123")
        );
        assert_ne!(
            idempotency_lock_hash("stripe:bt_123"),
            idempotency_lock_hash("stripe:bt_124")
        );
    }

    #[test]
    fn idempotency_lock_hash_diverges_quickly() {
        // Two keys that share a long prefix should still hash to
        // different buckets — defends against trivial collision
        // crafting from predictable provider IDs.
        let a = idempotency_lock_hash("stripe:tx:bt_AAAAAAAA0001");
        let b = idempotency_lock_hash("stripe:tx:bt_AAAAAAAA0002");
        assert_ne!(a, b);
    }

    #[test]
    fn idempotency_lock_hash_empty_key() {
        // FNV offset basis (cast through i32) for empty input.
        assert_eq!(idempotency_lock_hash(""), 0x811c_9dc5_u32 as i32);
    }

    #[test]
    fn customer_lock_targets_from_draft_finds_customer_accounts() {
        let tenant_id = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let draft = DraftTransaction {
            tenant_id,
            kind: "test".into(),
            idempotency_key: "k".into(),
            description: None,
            metadata: serde_json::json!({}),
            postings: vec![
                DraftPosting {
                    account_code: format!("ar/{b}"),
                    direction: Direction::Debit,
                    amount_minor: 10,
                    currency: "USD".into(),
                    source: "test".into(),
                    source_event_id: "1".into(),
                    metadata: serde_json::json!({}),
                },
                DraftPosting {
                    account_code: "clearing/stripe/acct_1".into(),
                    direction: Direction::Credit,
                    amount_minor: 10,
                    currency: "USD".into(),
                    source: "test".into(),
                    source_event_id: "2".into(),
                    metadata: serde_json::json!({}),
                },
                DraftPosting {
                    account_code: format!("unallocated_cash/{a}"),
                    direction: Direction::Credit,
                    amount_minor: 10,
                    currency: "USD".into(),
                    source: "test".into(),
                    source_event_id: "3".into(),
                    metadata: serde_json::json!({}),
                },
                DraftPosting {
                    account_code: format!("credit_memo/{a}"),
                    direction: Direction::Debit,
                    amount_minor: 10,
                    currency: "USD".into(),
                    source: "test".into(),
                    source_event_id: "4".into(),
                    metadata: serde_json::json!({}),
                },
            ],
        };

        let mut expected = vec![a.to_string(), b.to_string()];
        expected.sort();

        assert_eq!(customer_lock_targets_from_draft(&draft), expected);
    }
}
