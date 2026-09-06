//! A text-to-speech job: the audio itself is returned to the caller (and not
//! stored); the row records what was synthesized.

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "syntheses")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(column_type = "Text")]
    pub text: String,
    pub voice: String,
    pub provider: String,
    pub model: String,
    /// Output container format: "wav" | "mp3".
    pub format: String,
    pub audio_bytes: i64,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
