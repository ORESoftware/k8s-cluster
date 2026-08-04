//! billing-server-rs entrypoint.
//!
//! HTTP-only billing platform. Model A (observer/recorder) — we do not move
//! money on our own license. Postgres is the ledger source of truth; Solana
//! is the tamper-evidence notary; provider data flows in via OAuth /
//! webhook ingestors (mostly stubbed in this scaffold).

mod admin;
mod api;
mod cdc;
mod config;
mod crypto;
mod customer_locks;
mod customers;
mod db;
mod entity;
mod error;
mod events;
mod fiducia;
mod financial_audit;
mod jobs;
mod ledger;
mod locks;
mod memberships;
mod money;
mod notifications;
mod providers;
mod scheduler;
mod server;
mod shard;
mod shared_auth;
mod shared_auth_startup;
mod solana;
mod state;
mod supabase_auth;
mod sync;
mod tenants;
mod users;
mod vendors;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::run().await
}
