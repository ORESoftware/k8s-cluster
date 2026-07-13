use super::{
    DraftNote, Mutation, MutationAction, MutationBatch, MutationBatchResponse, MutationResult,
    MutationStatus, PullQuery, PullResponse, RecordKey, RecordSnapshot, DRAFT_NOTE_KIND,
    PROTOCOL_VERSION,
};
use crate::{
    auth::AuthContext,
    db::{
        begin_user_transaction,
        entity::{sync_change, sync_clock, sync_receipt, sync_record},
    },
    error::AppError,
    AppState,
};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_BATCH_SIZE: usize = 50;
const DEFAULT_PAGE_SIZE: u64 = 200;
const MAX_PAGE_SIZE: u64 = 500;
const MAX_PAGE_CANDIDATES: u64 = 16;
const MAX_PAGE_ENCODED_BYTES: usize = 1024 * 1024;

pub async fn push_mutations(
    state: &AppState,
    actor: &AuthContext,
    batch: MutationBatch,
) -> Result<MutationBatchResponse, AppError> {
    if batch.protocol_version != PROTOCOL_VERSION {
        return Err(AppError::BadRequest(
            "unsupported sync protocol version".into(),
        ));
    }
    if batch.operations.is_empty() || batch.operations.len() > MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(format!(
            "operations must contain between 1 and {MAX_BATCH_SIZE} items"
        )));
    }

    let mut results = Vec::with_capacity(batch.operations.len());
    for mutation in batch.operations {
        let (result, cursor) =
            apply_mutation(state, actor.user_id, batch.client_id, mutation).await?;
        if let Some(cursor) = cursor {
            state.hub.invalidate(actor.user_id, cursor);
        }
        results.push(result);
    }
    Ok(MutationBatchResponse { results })
}

async fn apply_mutation(
    state: &AppState,
    owner_id: Uuid,
    client_id: Uuid,
    mutation: Mutation,
) -> Result<(MutationResult, Option<i64>), AppError> {
    let request_hash = URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&mutation)?));
    let transaction = begin_user_transaction(&state.db, owner_id).await?;

    // The per-owner clock is both a commit-ordered cursor and a narrow write
    // serialization point. This avoids the commit-order gap possible with a
    // plain PostgreSQL sequence.
    sync_clock::Entity::insert(sync_clock::ActiveModel {
        owner_id: Set(owner_id),
        cursor: Set(0),
    })
    .on_conflict(
        OnConflict::column(sync_clock::Column::OwnerId)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(&transaction)
    .await?;
    let clock = sync_clock::Entity::find_by_id(owner_id)
        .lock_exclusive()
        .one(&transaction)
        .await?
        .ok_or_else(|| AppError::BadRequest("sync clock was not initialized".into()))?;

    if let Some(receipt) =
        sync_receipt::Entity::find_by_id((owner_id, client_id, mutation.mutation_id))
            .one(&transaction)
            .await?
    {
        let result = if receipt.request_hash == request_hash {
            serde_json::from_value(receipt.result)?
        } else {
            MutationResult {
                mutation_id: mutation.mutation_id,
                status: MutationStatus::IdempotencyKeyReused,
                record: None,
                message: Some("mutation ID was already used for different content".into()),
            }
        };
        transaction.commit().await?;
        return Ok((result, None));
    }

    if let Some(result) = validate_mutation(&mutation) {
        store_receipt(
            &transaction,
            owner_id,
            client_id,
            mutation.mutation_id,
            request_hash,
            &result,
        )
        .await?;
        transaction.commit().await?;
        return Ok((result, None));
    }

    let current =
        sync_record::Entity::find_by_id((owner_id, mutation.key.kind.clone(), mutation.key.id))
            .lock_exclusive()
            .one(&transaction)
            .await?;
    let base_version = parse_base_version(&mutation)?;

    let mut applied_cursor = None;
    let result = match (current, mutation.action) {
        (None, MutationAction::Delete) => MutationResult {
            mutation_id: mutation.mutation_id,
            status: MutationStatus::Gone,
            record: None,
            message: Some("record does not exist".into()),
        },
        (None, MutationAction::Put) if base_version.is_some() => MutationResult {
            mutation_id: mutation.mutation_id,
            status: MutationStatus::Conflict,
            record: None,
            message: Some("record was removed or never existed".into()),
        },
        (None, MutationAction::Put) => {
            let value = mutation.value.clone().expect("validated put value");
            let now = Utc::now();
            let model = sync_record::ActiveModel {
                owner_id: Set(owner_id),
                collection: Set(mutation.key.kind.clone()),
                record_id: Set(mutation.key.id),
                version: Set(1),
                payload: Set(serde_json::to_value(&value)?),
                deleted_at: Set(None),
                updated_at: Set(now),
            }
            .insert(&transaction)
            .await?;
            let snapshot = snapshot_from_record(&model)?;
            let cursor = write_change(&transaction, owner_id, clock.cursor + 1, &snapshot).await?;
            applied_cursor = Some(cursor);
            MutationResult {
                mutation_id: mutation.mutation_id,
                status: MutationStatus::Applied,
                record: Some(snapshot),
                message: None,
            }
        }
        (Some(model), _) if model.deleted_at.is_some() => MutationResult {
            mutation_id: mutation.mutation_id,
            status: MutationStatus::Gone,
            record: Some(snapshot_from_record(&model)?),
            message: Some("deleted record IDs cannot be reused".into()),
        },
        (Some(model), _) if Some(model.version) != base_version => MutationResult {
            mutation_id: mutation.mutation_id,
            status: MutationStatus::Conflict,
            record: Some(snapshot_from_record(&model)?),
            message: Some("base version is stale".into()),
        },
        (Some(model), action) => {
            let now = Utc::now();
            let mut active: sync_record::ActiveModel = model.into();
            active.version = Set(active.version.as_ref() + 1);
            active.updated_at = Set(now);
            match action {
                MutationAction::Put => {
                    active.payload = Set(serde_json::to_value(
                        mutation.value.as_ref().expect("validated put value"),
                    )?);
                    active.deleted_at = Set(None);
                }
                MutationAction::Delete => active.deleted_at = Set(Some(now)),
            }
            let model = active.update(&transaction).await?;
            let snapshot = snapshot_from_record(&model)?;
            let cursor = write_change(&transaction, owner_id, clock.cursor + 1, &snapshot).await?;
            applied_cursor = Some(cursor);
            MutationResult {
                mutation_id: mutation.mutation_id,
                status: MutationStatus::Applied,
                record: Some(snapshot),
                message: None,
            }
        }
    };

    if let Some(cursor) = applied_cursor {
        state
            .hub
            .enqueue_postgres_invalidation(&transaction, owner_id, cursor)
            .await?;
    }

    store_receipt(
        &transaction,
        owner_id,
        client_id,
        mutation.mutation_id,
        request_hash,
        &result,
    )
    .await?;
    transaction.commit().await?;
    Ok((result, applied_cursor))
}

async fn store_receipt<C: ConnectionTrait>(
    connection: &C,
    owner_id: Uuid,
    client_id: Uuid,
    mutation_id: Uuid,
    request_hash: String,
    result: &MutationResult,
) -> Result<(), AppError> {
    sync_receipt::ActiveModel {
        owner_id: Set(owner_id),
        client_id: Set(client_id),
        mutation_id: Set(mutation_id),
        request_hash: Set(request_hash),
        result: Set(serde_json::to_value(result)?),
        created_at: Set(Utc::now()),
    }
    .insert(connection)
    .await?;
    Ok(())
}

async fn write_change<C: ConnectionTrait>(
    connection: &C,
    owner_id: Uuid,
    cursor: i64,
    snapshot: &RecordSnapshot,
) -> Result<i64, AppError> {
    sync_clock::ActiveModel {
        owner_id: Set(owner_id),
        cursor: Set(cursor),
    }
    .update(connection)
    .await?;
    sync_change::ActiveModel {
        owner_id: Set(owner_id),
        cursor: Set(cursor),
        collection: Set(snapshot.key.kind.clone()),
        record_id: Set(snapshot.key.id),
        version: Set(snapshot
            .version
            .parse()
            .map_err(|_| AppError::BadRequest("invalid internal record version".into()))?),
        operation: Set(if snapshot.deleted { "delete" } else { "put" }.into()),
        payload: Set(serde_json::to_value(snapshot)?),
        changed_at: Set(Utc::now()),
    }
    .insert(connection)
    .await?;
    Ok(cursor)
}

pub async fn pull_changes(
    state: &AppState,
    actor: &AuthContext,
    query: PullQuery,
) -> Result<PullResponse, AppError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let codec = CursorCodec::new(&state.config.session_encryption_key)?;
    let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
    let current_high_water = sync_clock::Entity::find_by_id(actor.user_id)
        .one(&transaction)
        .await?
        .map(|clock| clock.cursor)
        .unwrap_or(0);

    let (position, high_water) = match query.cursor {
        Some(cursor) => {
            let decoded = codec.decode(&cursor, actor.user_id)?;
            if decoded.position >= decoded.high_water {
                (decoded.position, current_high_water)
            } else {
                (decoded.position, decoded.high_water)
            }
        }
        None => (0, current_high_water),
    };

    let rows = sync_change::Entity::find()
        .filter(sync_change::Column::OwnerId.eq(actor.user_id))
        .filter(sync_change::Column::Cursor.gt(position))
        .filter(sync_change::Column::Cursor.lte(high_water))
        .order_by_asc(sync_change::Column::Cursor)
        .limit(limit.min(MAX_PAGE_CANDIDATES))
        .all(&transaction)
        .await?;
    let mut changes = Vec::with_capacity(rows.len());
    let mut next_position = position;
    // Account for the JSON array brackets and commas. PullResponse adds a
    // small, fixed envelope around this budget.
    let mut encoded_bytes = 2usize;
    for row in rows {
        let snapshot = serde_json::from_value::<RecordSnapshot>(row.payload)?;
        let snapshot_bytes = serde_json::to_vec(&snapshot)?.len();
        let separator_bytes = usize::from(!changes.is_empty());
        let next_bytes = encoded_bytes
            .saturating_add(separator_bytes)
            .saturating_add(snapshot_bytes);
        if !changes.is_empty() && next_bytes > MAX_PAGE_ENCODED_BYTES {
            break;
        }
        if next_bytes > MAX_PAGE_ENCODED_BYTES {
            // Current validation keeps an individual record below the page
            // budget. Still advance if legacy data violates that invariant so
            // one oversized row cannot permanently stall an account's cursor.
            tracing::warn!(
                snapshot_bytes,
                max_page_bytes = MAX_PAGE_ENCODED_BYTES,
                "single sync snapshot exceeds the pull page byte budget"
            );
        }
        encoded_bytes = next_bytes;
        next_position = row.cursor;
        changes.push(snapshot);
    }
    transaction.commit().await?;

    Ok(PullResponse {
        changes,
        next_cursor: codec.encode(actor.user_id, next_position, high_water)?,
        caught_up: next_position >= high_water,
    })
}

fn validate_mutation(mutation: &Mutation) -> Option<MutationResult> {
    let message = if mutation.key.kind != DRAFT_NOTE_KIND {
        Some("unsupported record kind")
    } else if mutation.schema_version != 1 {
        Some("unsupported draft_note schema version")
    } else if !valid_base_version(mutation.base_version.as_deref()) {
        Some("baseVersion must be a canonical non-negative 64-bit decimal string")
    } else {
        match (&mutation.action, &mutation.value) {
            (MutationAction::Put, Some(note))
                if note.title.chars().count() <= 200 && note.body.chars().count() <= 100_000 =>
            {
                None
            }
            (MutationAction::Put, Some(_)) => Some("draft note exceeds its size limit"),
            (MutationAction::Put, None) => Some("put requires a value"),
            (MutationAction::Delete, Some(_)) => Some("delete must not include a value"),
            (MutationAction::Delete, None) => None,
        }
    };
    message.map(|message| MutationResult {
        mutation_id: mutation.mutation_id,
        status: MutationStatus::Invalid,
        record: None,
        message: Some(message.into()),
    })
}

fn valid_base_version(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    if value == "0" {
        return true;
    }
    value
        .as_bytes()
        .first()
        .is_some_and(|first| first.is_ascii_digit() && *first != b'0')
        && value.as_bytes().iter().all(u8::is_ascii_digit)
        && value.parse::<i64>().is_ok()
}

fn parse_base_version(mutation: &Mutation) -> Result<Option<i64>, AppError> {
    mutation
        .base_version
        .as_deref()
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                AppError::BadRequest("baseVersion must be a signed 64-bit decimal string".into())
            })
        })
        .transpose()
}

fn snapshot_from_record(model: &sync_record::Model) -> Result<RecordSnapshot, AppError> {
    Ok(RecordSnapshot {
        key: RecordKey {
            kind: model.collection.clone(),
            id: model.record_id,
        },
        version: model.version.to_string(),
        schema_version: 1,
        deleted: model.deleted_at.is_some(),
        value: if model.deleted_at.is_some() {
            None
        } else {
            Some(serde_json::from_value::<DraftNote>(model.payload.clone())?)
        },
    })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload {
    owner_id: Uuid,
    position: i64,
    high_water: i64,
}

struct CursorCodec(Aes256Gcm);

impl CursorCodec {
    fn new(key: &[u8]) -> Result<Self, AppError> {
        Ok(Self(
            Aes256Gcm::new_from_slice(key).map_err(|_| AppError::Crypto)?,
        ))
    }

    fn encode(&self, owner_id: Uuid, position: i64, high_water: i64) -> Result<String, AppError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AppError::Crypto)?;
        let plaintext = serde_json::to_vec(&CursorPayload {
            owner_id,
            position,
            high_water,
        })?;
        let ciphertext = self
            .0
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|_| AppError::Crypto)?;
        let mut bytes = nonce_bytes.to_vec();
        bytes.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn decode(&self, cursor: &str, expected_owner: Uuid) -> Result<CursorPayload, AppError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| AppError::InvalidSyncCursor)?;
        let (nonce_bytes, ciphertext) = bytes
            .split_at_checked(12)
            .ok_or(AppError::InvalidSyncCursor)?;
        let nonce = nonce_bytes
            .try_into()
            .map_err(|_| AppError::InvalidSyncCursor)?;
        let plaintext = self
            .0
            .decrypt(nonce, ciphertext)
            .map_err(|_| AppError::InvalidSyncCursor)?;
        let decoded: CursorPayload =
            serde_json::from_slice(&plaintext).map_err(|_| AppError::InvalidSyncCursor)?;
        if decoded.owner_id != expected_owner
            || decoded.position < 0
            || decoded.high_water < decoded.position
        {
            return Err(AppError::InvalidSyncCursor);
        }
        Ok(decoded)
    }
}
