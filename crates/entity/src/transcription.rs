//! A speech-to-text result: one row per transcribed clip.

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "transcriptions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Where the audio came from: "upload" | "pipeline" | "vapi".
    pub source: String,
    pub provider: String,
    pub model: String,
    #[sea_orm(column_type = "Text")]
    pub text: String,
    pub language: Option<String>,
    pub sample_rate: Option<i32>,
    pub duration_ms: Option<i64>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
