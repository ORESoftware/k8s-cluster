use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use uuid::Uuid;

use crate::money::{Currency, Money};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "account_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Asset,
    Liability,
    Income,
    Expense,
    Receivable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "account_normal_side", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum NormalSide {
    Debit,
    Credit,
}

impl AccountKind {
    pub fn normal_side(self) -> NormalSide {
        match self {
            Self::Asset | Self::Expense | Self::Receivable => NormalSide::Debit,
            Self::Liability | Self::Income => NormalSide::Credit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "posting_direction", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Debit,
    Credit,
}

impl Direction {
    pub fn opposite(self) -> Self {
        match self {
            Self::Debit => Self::Credit,
            Self::Credit => Self::Debit,
        }
    }

    pub fn signed_for(self, side: NormalSide) -> i128 {
        // +1 if the posting increases the account's normal balance, -1 if it decreases it
        match (side, self) {
            (NormalSide::Debit, Self::Debit) => 1,
            (NormalSide::Debit, Self::Credit) => -1,
            (NormalSide::Credit, Self::Credit) => 1,
            (NormalSide::Credit, Self::Debit) => -1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub user_id: Option<Uuid>,
    pub kind: AccountKind,
    pub normal_side: NormalSide,
    pub code: String,
    pub currency: Currency,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub kind: String,
    pub idempotency_key: String,
    pub description: Option<String>,
    pub metadata: serde_json::Value,
    pub posted_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Posting {
    pub id: i64,
    pub transaction_id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub account_id: Uuid,
    pub direction: Direction,
    pub amount: Money,
    pub source: String,
    pub source_event_id: String,
    pub posted_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

/// Caller-facing transaction draft. The service validates zero-sum and
/// idempotency, then commits all postings in a single DB transaction.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftTransaction {
    pub tenant_id: Uuid,
    pub kind: String,
    pub idempotency_key: String,
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub postings: Vec<DraftPosting>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftPosting {
    pub account_code: String,
    pub direction: Direction,
    pub amount_minor: i128,
    pub currency: String,
    pub source: String,
    pub source_event_id: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountBalance {
    pub account_id: Uuid,
    pub account_code: String,
    pub currency: Currency,
    pub balance_minor: i128,
    pub as_of: DateTime<Utc>,
}

// Helper newtype so sqlx can decode JSONB into serde_json::Value via Json<T>.
pub(crate) type JsonValue = Json<serde_json::Value>;
