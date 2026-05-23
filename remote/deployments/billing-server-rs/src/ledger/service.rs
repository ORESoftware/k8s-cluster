use chrono::Utc;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::money::Currency;
use crate::shard::{Region, ShardKey};

use super::types::*;

#[derive(Clone)]
pub struct LedgerService {
    pool: PgPool,
}

impl LedgerService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        if let Some(acct) = sqlx::query(
            r#"
            SELECT id, tenant_id, shard_key, user_id, kind,
                   normal_side, code, currency,
                   metadata, created_at
            FROM accounts
            WHERE tenant_id = $1 AND code = $2 AND currency = $3
            "#,
        )
        .bind(tenant_id)
        .bind(code)
        .bind(&currency_str)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(row_to_account(&acct)?);
        }

        let row = sqlx::query(
            r#"
            INSERT INTO accounts
                (tenant_id, shard_key, user_id, kind, normal_side, code, currency)
            VALUES ($1, $2, $3, $4::account_kind, $5::account_normal_side, $6, $7)
            ON CONFLICT (tenant_id, code, currency) DO UPDATE SET code = EXCLUDED.code
            RETURNING id, tenant_id, shard_key, user_id, kind, normal_side, code,
                      currency, metadata, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(user_id)
        .bind(kind_to_str(kind))
        .bind(normal_side_to_str(normal_side))
        .bind(code)
        .bind(&currency_str)
        .fetch_one(&self.pool)
        .await?;

        row_to_account(&row)
    }

    /// Post a draft transaction. Atomic, idempotent, and zero-sum-checked.
    ///
    /// * The DB's deferred constraint trigger enforces the zero-sum invariant
    ///   per currency at COMMIT time.
    /// * Idempotency: a repeat with the same `(tenant_id, idempotency_key)`
    ///   returns the existing transaction id without writing again.
    pub async fn post_transaction(
        &self,
        draft: &DraftTransaction,
        region: Region,
    ) -> AppResult<Uuid> {
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

        let shard = ShardKey::derive(draft.tenant_id, region).0;

        let mut tx = self.pool.begin().await?;

        // Idempotency short-circuit
        if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
            r#"SELECT id FROM transactions
               WHERE tenant_id = $1 AND idempotency_key = $2"#,
        )
        .bind(draft.tenant_id)
        .bind(&draft.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(existing);
        }

        let proposed_tx_id = Uuid::new_v4();
        let inserted_tx_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO transactions
                (id, tenant_id, shard_key, kind, idempotency_key, description, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(proposed_tx_id)
        .bind(draft.tenant_id)
        .bind(shard)
        .bind(&draft.kind)
        .bind(&draft.idempotency_key)
        .bind(&draft.description)
        .bind(&draft.metadata)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(tx_id) = inserted_tx_id else {
            let existing: Uuid = sqlx::query_scalar(
                r#"SELECT id FROM transactions
                   WHERE tenant_id = $1 AND idempotency_key = $2"#,
            )
            .bind(draft.tenant_id)
            .bind(&draft.idempotency_key)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(existing);
        };

        for p in &draft.postings {
            // Resolve account by code (per-tenant, per-currency unique).
            let acct_id: Uuid = sqlx::query_scalar(
                r#"SELECT id FROM accounts
                   WHERE tenant_id = $1 AND code = $2 AND currency = $3"#,
            )
            .bind(draft.tenant_id)
            .bind(&p.account_code)
            .bind(&p.currency)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "account not found for code={} currency={}",
                    p.account_code, p.currency
                ))
            })?;

            // Bind amount as text and cast to NUMERIC in SQL — avoids requiring
            // sqlx's rust_decimal/bigdecimal feature for i128 transport.
            let amount_text = p.amount_minor.to_string();

            sqlx::query(
                r#"
                INSERT INTO postings
                    (transaction_id, tenant_id, shard_key, account_id, direction,
                     amount_minor, currency, source, source_event_id, metadata)
                VALUES ($1, $2, $3, $4, $5::posting_direction,
                        ($6)::NUMERIC(38, 0), $7, $8, $9, $10)
                "#,
            )
            .bind(tx_id)
            .bind(draft.tenant_id)
            .bind(shard)
            .bind(acct_id)
            .bind(direction_to_str(p.direction))
            .bind(&amount_text)
            .bind(&p.currency)
            .bind(&p.source)
            .bind(&p.source_event_id)
            .bind(&p.metadata)
            .execute(&mut *tx)
            .await
            .map_err(map_pg_constraint_err)?;
        }

        tx.commit().await?;
        Ok(tx_id)
    }

    pub async fn account_balance(
        &self,
        tenant_id: Uuid,
        account_code: &str,
        currency: Currency,
    ) -> AppResult<AccountBalance> {
        let cur = currency.as_str().to_string();

        let row = sqlx::query(
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
        )
        .bind(tenant_id)
        .bind(account_code)
        .bind(&cur)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("account {account_code}/{cur}")))?;

        let account_id: Uuid = row.try_get("id")?;
        let balance_text: String = row.try_get("balance_text")?;
        let balance_minor: i128 = balance_text.parse().unwrap_or(0);

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
        let row = sqlx::query(
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
        )
        .bind(tenant_id)
        .bind(account_code_like)
        .bind(&cur)
        .fetch_one(&self.pool)
        .await?;

        let net_text: String = row.try_get("net")?;
        Ok(net_text.parse().unwrap_or(0))
    }
}

fn row_to_account(row: &sqlx::postgres::PgRow) -> AppResult<Account> {
    let currency_str: String = row.try_get("currency")?;
    let metadata: serde_json::Value = row.try_get("metadata")?;

    Ok(Account {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        shard_key: row.try_get("shard_key")?,
        user_id: row.try_get("user_id")?,
        kind: row.try_get("kind")?,
        normal_side: row.try_get("normal_side")?,
        code: row.try_get("code")?,
        currency: Currency::new(&currency_str).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        metadata,
        created_at: row.try_get("created_at")?,
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

fn map_pg_constraint_err(e: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(ref db_err) = e {
        if let Some(code) = db_err.code() {
            if code == "23505" {
                return AppError::Conflict(format!(
                    "posting already exists (idempotent replay): {}",
                    db_err.message()
                ));
            }
        }
    }
    AppError::Database(e)
}

#[allow(dead_code)]
fn _silence_unused_jsonvalue(_: JsonValue) {}
