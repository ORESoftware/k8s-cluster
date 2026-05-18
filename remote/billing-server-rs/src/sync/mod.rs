//! Provider sync: on-demand + backstop polling.
//!
//! Design philosophy (intentional, documented in the platform brief):
//!
//!  1. **Webhooks** are the primary mechanism. Most providers (Stripe, PayPal,
//!     Braintree, Plaid, Coinbase) push events in real time.
//!  2. **On-demand sync** is the dominant *poll* path. When a B2B customer
//!     hits "Refresh" in their dashboard, the platform syncs immediately.
//!     This is what users interact with.
//!  3. **Backstop sync** is a low-frequency safety net (default: every ~5
//!     hours / 5x/day) that catches anything webhooks or on-demand syncs
//!     missed. This is *not* the primary mechanism — it exists so the
//!     ledger eventually converges even if everything else fails.
//!
//! All three feed the same `ConnectionSyncJob` handler, which:
//!
//!  - Acquires a tenant-scoped lease on `connection:<id>` (so concurrent
//!    on-demand requests don't double-sync).
//!  - Loads the sealed credential, dispatches to the per-provider sync.
//!  - On success: updates `provider_connections.last_sync_at`, releases lease.
//!  - On failure: marks the connection's `last_error`, releases lease,
//!    returns Err so the scheduler retries per `max_attempts`.
//!
//! The per-provider sync bodies are stubbed in this commit. The contract is
//! frozen here so the API + scheduling layer is real today; each provider's
//! `sync_balance_transactions` lands incrementally.

pub mod handler;

pub use handler::ConnectionSyncJob;
