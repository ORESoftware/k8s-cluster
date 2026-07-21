//! The RDS identity mirror (SeaORM).
//!
//! A `users` table in AWS RDS (`shared_auth` schema) that mirrors the Supabase
//! identities this server has seen — a shortcut/parallel index so downstream
//! services resolve a stable OreSoftware user id without a Supabase round-trip.
//! A mirror of *identity*, never of credentials.
//!
//! The schema is owned declaratively by pg-defs (`db/schema.sql`, applied via
//! dpm). This process connects with `search_path=shared_auth` and runs **no
//! DDL** — only upserts and reads.

mod entity;
mod users;

pub use users::{MirroredUser, UserStore};
