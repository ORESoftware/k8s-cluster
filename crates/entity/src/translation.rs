//! An LLM translation: one row per translate call, whichever provider ran it.

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "translations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(column_type = "Text")]
    pub source_text: String,
    #[sea_orm(column_type = "Text")]
    pub translated_text: String,
    pub source_lang: Option<String>,
    pub target_lang: String,
    /// "openai" | "gemini" | "anthropic".
    pub provider: String,
    pub model: String,
    pub latency_ms: i64,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
