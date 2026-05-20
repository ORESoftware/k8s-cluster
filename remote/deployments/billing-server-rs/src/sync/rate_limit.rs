use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::AppResult;
use crate::providers::ProviderKind;

#[derive(Clone)]
pub struct ProviderRateLimiter {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderBudget {
    pub window_seconds: i32,
    pub request_limit: i32,
}

#[derive(Clone, Debug)]
pub struct RateLimitReservation {
    pub allowed: bool,
    pub retry_after_seconds: i64,
    pub remaining: i32,
    pub window_start: DateTime<Utc>,
}

impl ProviderRateLimiter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn reserve(
        &self,
        tenant_id: Uuid,
        provider: ProviderKind,
    ) -> AppResult<RateLimitReservation> {
        let budget = provider_budget(provider);
        let now = Utc::now();
        let window_start = floor_to_window(now, budget.window_seconds);

        let row = sqlx::query(
            r#"
            INSERT INTO provider_rate_limit_buckets
                (tenant_id, provider, window_start, window_seconds,
                 request_limit, requests_used)
            VALUES ($1, $2::provider_kind, $3, $4, $5, 1)
            ON CONFLICT (tenant_id, provider, window_start, window_seconds)
            DO UPDATE SET
                requests_used = provider_rate_limit_buckets.requests_used + 1,
                request_limit = EXCLUDED.request_limit,
                updated_at = now()
            RETURNING requests_used, request_limit
            "#,
        )
        .bind(tenant_id)
        .bind(provider.tag())
        .bind(window_start)
        .bind(budget.window_seconds)
        .bind(budget.request_limit)
        .fetch_one(&self.pool)
        .await?;

        let used: i32 = row.try_get("requests_used")?;
        let limit: i32 = row.try_get("request_limit")?;
        let allowed = used <= limit;
        let remaining = (limit - used).max(0);
        let window_end = window_start + chrono::Duration::seconds(budget.window_seconds as i64);
        let retry_after_seconds = (window_end - now).num_seconds().max(1);

        Ok(RateLimitReservation {
            allowed,
            retry_after_seconds,
            remaining,
            window_start,
        })
    }
}

fn provider_budget(provider: ProviderKind) -> ProviderBudget {
    match provider {
        // Conservative shared budgets. Provider HTTP implementations can add
        // endpoint-specific budgets before they call more expensive APIs.
        ProviderKind::Stripe => ProviderBudget { window_seconds: 60, request_limit: 1_200 },
        ProviderKind::Paypal => ProviderBudget { window_seconds: 60, request_limit: 120 },
        ProviderKind::Braintree => ProviderBudget { window_seconds: 60, request_limit: 120 },
        ProviderKind::CoinbaseCommerce | ProviderKind::CoinbasePrime => {
            ProviderBudget { window_seconds: 60, request_limit: 120 }
        }
        ProviderKind::Coinflow => ProviderBudget { window_seconds: 60, request_limit: 120 },
        ProviderKind::PlaidBank => ProviderBudget { window_seconds: 60, request_limit: 120 },
        ProviderKind::SwiftWire | ProviderKind::AchDirect | ProviderKind::Wise => {
            ProviderBudget { window_seconds: 60, request_limit: 60 }
        }
        ProviderKind::SolanaWallet => ProviderBudget { window_seconds: 60, request_limit: 240 },
    }
}

fn floor_to_window(now: DateTime<Utc>, window_seconds: i32) -> DateTime<Utc> {
    let window = window_seconds.max(1) as i64;
    let ts = now.timestamp();
    let floored = ts - ts.rem_euclid(window);
    DateTime::<Utc>::from_timestamp(floored, 0).unwrap_or(now)
}
