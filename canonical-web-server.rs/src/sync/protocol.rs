use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u8 = 1;
pub const DRAFT_NOTE_KIND: &str = "draft_note";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationBatch {
    pub protocol_version: u8,
    pub client_id: Uuid,
    pub operations: Vec<Mutation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutation {
    pub mutation_id: Uuid,
    pub key: RecordKey,
    pub action: MutationAction,
    pub base_version: Option<String>,
    pub schema_version: u8,
    pub value: Option<DraftNote>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordKey {
    pub kind: String,
    pub id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationAction {
    Put,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftNote {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSnapshot {
    pub key: RecordKey,
    pub version: String,
    pub schema_version: u8,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<DraftNote>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MutationStatus {
    Applied,
    Conflict,
    Gone,
    Invalid,
    IdempotencyKeyReused,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult {
    pub mutation_id: Uuid,
    pub status: MutationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<RecordSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MutationBatchResponse {
    pub results: Vec<MutationResult>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullQuery {
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResponse {
    pub changes: Vec<RecordSnapshot>,
    pub next_cursor: String,
    pub caught_up: bool,
}
