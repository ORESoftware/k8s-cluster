//! Resumable Meshy job stages for Daedalus fabrication workers.
//!
//! The crate separates slow provider and artifact work from the durable RDS
//! state machine. Callers persist every returned checkpoint through
//! `ClaimedJobLease` before advancing. A provider create is guarded by a
//! persisted submission intent: if the process dies after the billable request
//! but before its task id is checkpointed, a restarted worker refuses to submit
//! again and requires explicit reconciliation instead of risking duplicate
//! spend.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use dd_meshy_client::{
    AiModel, GenerationOptions, ImageTo3dRequest, MeshyClient, MeshyError, ModelType,
    MultiImageTo3dRequest, TargetFormat, TaskKind, TaskStatus,
};
use futures_util::StreamExt;
use reqwest::{header::CONTENT_TYPE, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

pub const JOB_KIND: &str = "meshy_image_to_3d";
pub const JOB_SCHEMA: &str = "daedalus.meshy-job.v1";
pub const CHECKPOINT_SCHEMA: &str = "daedalus.meshy-job-checkpoint.v1";
pub const RESULT_SCHEMA: &str = "daedalus.meshy-job-result.v1";

const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MIN_MAX_ARTIFACT_BYTES: u64 = 1024 * 1024;
const MAX_MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_PURPOSE_BYTES: usize = 256;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_PROVIDER_ERROR_CHARS: usize = 500;
const DEFAULT_ARTIFACT_TIMEOUT_SECS: u64 = 30 * 60;

pub type MeshyJobResultValue<T> = Result<T, MeshyJobError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshyJobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub submission_ambiguous: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl MeshyJobError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: truncate_chars(&message.into(), MAX_PROVIDER_ERROR_CHARS),
            retryable,
            submission_ambiguous: false,
            retry_after_seconds: None,
        }
    }

    fn ambiguous(mut self) -> Self {
        self.submission_ambiguous = true;
        self
    }

    fn retry_after(mut self, seconds: Option<u64>) -> Self {
        self.retry_after_seconds = seconds;
        self
    }
}

impl fmt::Display for MeshyJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)?;
        if self.submission_ambiguous {
            write!(formatter, " [provider submission outcome is ambiguous]")?;
        }
        if let Some(seconds) = self.retry_after_seconds {
            write!(formatter, " [retry after {seconds}s]")?;
        }
        Ok(())
    }
}

impl Error for MeshyJobError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceImage {
    pub asset_id: String,
    pub url: String,
    /// Lowercase SHA-256 of the source bytes owned by Daedalus.
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationMode {
    ImageTo3d,
    MultiImageTo3d,
}

impl GenerationMode {
    pub const fn task_kind(self) -> TaskKind {
        match self {
            Self::ImageTo3d => TaskKind::ImageTo3d,
            Self::MultiImageTo3d => TaskKind::MultiImageTo3d,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExistingProviderTask {
    pub task_id: String,
    pub mode: GenerationMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshyJobRequest {
    #[serde(default = "default_job_schema")]
    pub schema_version: String,
    pub images: Vec<SourceImage>,
    #[serde(default = "default_target_formats")]
    pub target_formats: Vec<TargetFormat>,
    #[serde(default = "default_true")]
    pub should_texture: bool,
    #[serde(default)]
    pub should_remesh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_model: Option<AiModel>,
    #[serde(default = "default_purpose")]
    pub purpose: String,
    #[serde(default = "default_archive_prefix")]
    pub archive_prefix: String,
    #[serde(default = "default_max_artifact_bytes")]
    pub max_artifact_bytes: u64,
    /// Manual reconciliation path for a task whose create response could not be
    /// checkpointed. New automatic jobs leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_provider_task: Option<ExistingProviderTask>,
}

fn default_job_schema() -> String {
    JOB_SCHEMA.to_string()
}

fn default_target_formats() -> Vec<TargetFormat> {
    vec![TargetFormat::Glb, TargetFormat::Stl, TargetFormat::ThreeMf]
}

fn default_true() -> bool {
    true
}

fn default_purpose() -> String {
    "additive-prototype".to_string()
}

fn default_archive_prefix() -> String {
    "meshy".to_string()
}

fn default_max_artifact_bytes() -> u64 {
    DEFAULT_MAX_ARTIFACT_BYTES
}

impl MeshyJobRequest {
    pub fn validate(&self) -> MeshyJobResultValue<()> {
        if self.schema_version != JOB_SCHEMA {
            return Err(MeshyJobError::new(
                "unsupported_job_schema",
                format!(
                    "expected schema_version {JOB_SCHEMA:?}, got {:?}",
                    self.schema_version
                ),
                false,
            ));
        }
        if self.images.is_empty() || self.images.len() > 4 {
            return Err(MeshyJobError::new(
                "invalid_source_images",
                "images must contain between one and four entries",
                false,
            ));
        }
        let mut asset_ids = BTreeSet::new();
        for image in &self.images {
            validate_identifier("asset_id", &image.asset_id)?;
            if !asset_ids.insert(image.asset_id.trim().to_string()) {
                return Err(MeshyJobError::new(
                    "duplicate_asset_id",
                    "source image asset_id values must be unique",
                    false,
                ));
            }
            validate_https_url(&image.url, "source image url")?;
            validate_sha256("source image sha256", &image.sha256)?;
        }

        if self.target_formats.is_empty() || self.target_formats.len() > 6 {
            return Err(MeshyJobError::new(
                "invalid_target_formats",
                "target_formats must contain between one and six entries",
                false,
            ));
        }
        let mut formats = BTreeSet::new();
        for format in &self.target_formats {
            if !formats.insert(*format) {
                return Err(MeshyJobError::new(
                    "duplicate_target_format",
                    "target_formats must not contain duplicates",
                    false,
                ));
            }
        }

        if self.purpose.trim().is_empty() || self.purpose.len() > MAX_PURPOSE_BYTES {
            return Err(MeshyJobError::new(
                "invalid_purpose",
                format!("purpose must be 1-{MAX_PURPOSE_BYTES} bytes"),
                false,
            ));
        }
        if self.purpose.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(MeshyJobError::new(
                "invalid_purpose",
                "purpose must not contain control characters",
                false,
            ));
        }
        validate_object_key(&self.archive_prefix)?;
        if !(MIN_MAX_ARTIFACT_BYTES..=MAX_MAX_ARTIFACT_BYTES).contains(&self.max_artifact_bytes) {
            return Err(MeshyJobError::new(
                "invalid_artifact_limit",
                format!(
                    "max_artifact_bytes must be between {MIN_MAX_ARTIFACT_BYTES} and {MAX_MAX_ARTIFACT_BYTES}"
                ),
                false,
            ));
        }

        let mode = self.mode();
        let smart_topology = matches!(self.ai_model, Some(AiModel::MeshyT1 | AiModel::MeshyT2));
        if mode == GenerationMode::MultiImageTo3d && smart_topology {
            return Err(MeshyJobError::new(
                "invalid_model_for_multi_image",
                "meshy-t1 and meshy-t2 are single-image models",
                false,
            ));
        }
        if smart_topology && self.should_remesh {
            return Err(MeshyJobError::new(
                "ambiguous_smart_topology_options",
                "smart-topology models cannot be combined with should_remesh=true",
                false,
            ));
        }
        if let Some(existing) = &self.existing_provider_task {
            validate_task_id(&existing.task_id)?;
            if existing.mode != mode {
                return Err(MeshyJobError::new(
                    "provider_task_mode_mismatch",
                    "existing_provider_task.mode must match the number of source images",
                    false,
                ));
            }
        }
        Ok(())
    }

    pub fn mode(&self) -> GenerationMode {
        if self.images.len() == 1 {
            GenerationMode::ImageTo3d
        } else {
            GenerationMode::MultiImageTo3d
        }
    }

    pub fn fingerprint(&self) -> MeshyJobResultValue<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            MeshyJobError::new(
                "request_fingerprint_failed",
                format!("could not serialize Meshy job request: {error}"),
                false,
            )
        })?;
        Ok(sha256_hex(&bytes))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobIdentity {
    pub tenant_id: String,
    pub job_id: String,
}

impl JobIdentity {
    pub fn validate(&self) -> MeshyJobResultValue<()> {
        validate_identifier("tenant_id", &self.tenant_id)?;
        Uuid::parse_str(self.job_id.trim()).map_err(|error| {
            MeshyJobError::new(
                "invalid_job_id",
                format!("job_id must be a UUID: {error}"),
                false,
            )
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubmissionCheckpoint {
    Prepared {
        attempt_id: String,
        prepared_at_epoch_ms: i64,
    },
    Submitted {
        task_id: String,
        adopted: bool,
        submitted_at_epoch_ms: i64,
    },
    Ambiguous {
        attempt_id: String,
        reason: String,
        observed_at_epoch_ms: i64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTaskStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    Canceled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderTaskSnapshot {
    pub id: String,
    pub status: ProviderTaskStatus,
    pub progress: u8,
    #[serde(default)]
    pub artifacts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_credits: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchivedArtifact {
    pub object_key: String,
    pub archive_uri: String,
    pub sha256: String,
    pub byte_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshyJobCheckpoint {
    pub schema_version: String,
    pub request_fingerprint: String,
    pub mode: GenerationMode,
    pub submission: SubmissionCheckpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderTaskSnapshot>,
    #[serde(default)]
    pub archived_artifacts: BTreeMap<String, ArchivedArtifact>,
}

impl MeshyJobCheckpoint {
    pub fn from_value(value: &JsonValue) -> MeshyJobResultValue<Option<Self>> {
        if value.is_null() || value.as_object().is_some_and(|map| map.is_empty()) {
            return Ok(None);
        }
        let checkpoint: Self = serde_json::from_value(value.clone()).map_err(|error| {
            MeshyJobError::new(
                "invalid_checkpoint",
                format!("could not decode Meshy checkpoint: {error}"),
                false,
            )
        })?;
        if checkpoint.schema_version != CHECKPOINT_SCHEMA {
            return Err(MeshyJobError::new(
                "unsupported_checkpoint_schema",
                format!(
                    "expected checkpoint schema {CHECKPOINT_SCHEMA:?}, got {:?}",
                    checkpoint.schema_version
                ),
                false,
            ));
        }
        Ok(Some(checkpoint))
    }

    pub fn to_value(&self) -> MeshyJobResultValue<JsonValue> {
        serde_json::to_value(self).map_err(|error| {
            MeshyJobError::new(
                "checkpoint_serialization_failed",
                format!("could not serialize Meshy checkpoint: {error}"),
                false,
            )
        })
    }

    pub fn provider_task_id(&self) -> Option<&str> {
        match &self.submission {
            SubmissionCheckpoint::Submitted { task_id, .. } => Some(task_id),
            _ => None,
        }
    }

    fn validate_for_request(&self, request: &MeshyJobRequest) -> MeshyJobResultValue<()> {
        if self.request_fingerprint != request.fingerprint()? {
            return Err(MeshyJobError::new(
                "checkpoint_request_mismatch",
                "persisted Meshy checkpoint belongs to a different request payload",
                false,
            ));
        }
        if self.mode != request.mode() {
            return Err(MeshyJobError::new(
                "checkpoint_mode_mismatch",
                "persisted provider mode does not match the request",
                false,
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct SubmissionPermit {
    attempt_id: String,
    request_fingerprint: String,
}

#[derive(Debug)]
pub enum Preparation {
    Submit {
        checkpoint: MeshyJobCheckpoint,
        permit: SubmissionPermit,
    },
    Adopted {
        checkpoint: MeshyJobCheckpoint,
    },
}

#[derive(Debug)]
pub enum SubmissionOutcome {
    Submitted {
        checkpoint: MeshyJobCheckpoint,
    },
    Ambiguous {
        checkpoint: MeshyJobCheckpoint,
        error: MeshyJobError,
    },
}

#[derive(Debug)]
pub enum PollOutcome {
    Waiting { checkpoint: MeshyJobCheckpoint },
    Succeeded { checkpoint: MeshyJobCheckpoint },
}

#[derive(Debug)]
pub enum ArchiveOutcome {
    Archived {
        format: String,
        artifact: ArchivedArtifact,
        checkpoint: MeshyJobCheckpoint,
    },
    Complete {
        result: MeshyJobResult,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshyJobResult {
    pub schema_version: String,
    pub provider: String,
    pub provider_task_id: String,
    pub provider_task_status: ProviderTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_credits: Option<u64>,
    pub source_images: Vec<SourceImage>,
    pub artifacts: BTreeMap<String, ArchivedArtifact>,
    pub purpose: String,
    pub review_state: String,
    pub release_state: String,
    pub machine_ready: bool,
    pub required_evidence: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MeshyJobEngine;

impl MeshyJobEngine {
    pub fn prepare(
        &self,
        identity: &JobIdentity,
        request: &MeshyJobRequest,
        now_epoch_ms: i64,
    ) -> MeshyJobResultValue<Preparation> {
        identity.validate()?;
        request.validate()?;
        let fingerprint = request.fingerprint()?;
        if let Some(existing) = &request.existing_provider_task {
            let checkpoint = MeshyJobCheckpoint {
                schema_version: CHECKPOINT_SCHEMA.to_string(),
                request_fingerprint: fingerprint,
                mode: existing.mode,
                submission: SubmissionCheckpoint::Submitted {
                    task_id: existing.task_id.trim().to_string(),
                    adopted: true,
                    submitted_at_epoch_ms: now_epoch_ms,
                },
                provider: None,
                archived_artifacts: BTreeMap::new(),
            };
            return Ok(Preparation::Adopted { checkpoint });
        }

        let attempt_id = Uuid::new_v4().to_string();
        let checkpoint = MeshyJobCheckpoint {
            schema_version: CHECKPOINT_SCHEMA.to_string(),
            request_fingerprint: fingerprint.clone(),
            mode: request.mode(),
            submission: SubmissionCheckpoint::Prepared {
                attempt_id: attempt_id.clone(),
                prepared_at_epoch_ms: now_epoch_ms,
            },
            provider: None,
            archived_artifacts: BTreeMap::new(),
        };
        Ok(Preparation::Submit {
            checkpoint,
            permit: SubmissionPermit {
                attempt_id,
                request_fingerprint: fingerprint,
            },
        })
    }

    pub async fn submit_prepared<P: GeometryProvider + ?Sized>(
        &self,
        provider: &P,
        request: &MeshyJobRequest,
        mut checkpoint: MeshyJobCheckpoint,
        permit: SubmissionPermit,
        now_epoch_ms: i64,
    ) -> MeshyJobResultValue<SubmissionOutcome> {
        checkpoint.validate_for_request(request)?;
        let attempt_id = match &checkpoint.submission {
            SubmissionCheckpoint::Prepared { attempt_id, .. } => attempt_id,
            SubmissionCheckpoint::Submitted { .. } => {
                return Err(MeshyJobError::new(
                    "provider_task_already_submitted",
                    "provider task is already durably identified",
                    false,
                ))
            }
            SubmissionCheckpoint::Ambiguous { .. } => {
                return Err(MeshyJobError::new(
                    "provider_submission_requires_reconciliation",
                    "ambiguous provider submission cannot be retried automatically",
                    false,
                ))
            }
        };
        if attempt_id != &permit.attempt_id
            || checkpoint.request_fingerprint != permit.request_fingerprint
        {
            return Err(MeshyJobError::new(
                "invalid_submission_permit",
                "submission permit does not match the persisted intent",
                false,
            ));
        }

        match provider.submit(request).await {
            Ok(task_id) => {
                validate_task_id(&task_id)?;
                checkpoint.submission = SubmissionCheckpoint::Submitted {
                    task_id: task_id.trim().to_string(),
                    adopted: false,
                    submitted_at_epoch_ms: now_epoch_ms,
                };
                Ok(SubmissionOutcome::Submitted { checkpoint })
            }
            Err(error) if error.submission_ambiguous => {
                checkpoint.submission = SubmissionCheckpoint::Ambiguous {
                    attempt_id: permit.attempt_id,
                    reason: truncate_chars(&error.message, MAX_PROVIDER_ERROR_CHARS),
                    observed_at_epoch_ms: now_epoch_ms,
                };
                Ok(SubmissionOutcome::Ambiguous { checkpoint, error })
            }
            Err(error) => Err(error),
        }
    }

    pub async fn poll<P: GeometryProvider + ?Sized>(
        &self,
        provider: &P,
        request: &MeshyJobRequest,
        mut checkpoint: MeshyJobCheckpoint,
    ) -> MeshyJobResultValue<PollOutcome> {
        checkpoint.validate_for_request(request)?;
        let task_id = match &checkpoint.submission {
            SubmissionCheckpoint::Submitted { task_id, .. } => task_id.clone(),
            SubmissionCheckpoint::Prepared { .. } => {
                return Err(MeshyJobError::new(
                    "provider_submission_unreconciled",
                    "a persisted submission intent has no task id; do not resubmit automatically",
                    false,
                )
                .ambiguous())
            }
            SubmissionCheckpoint::Ambiguous { .. } => {
                return Err(MeshyJobError::new(
                    "provider_submission_requires_reconciliation",
                    "provider submission outcome is ambiguous; enqueue a reconciled request with existing_provider_task",
                    false,
                )
                .ambiguous())
            }
        };
        let mut snapshot = provider.get(checkpoint.mode, &task_id).await?;
        if snapshot.id != task_id {
            return Err(MeshyJobError::new(
                "provider_task_identity_mismatch",
                "provider returned a task whose id does not match the durable checkpoint",
                false,
            ));
        }
        snapshot.progress = snapshot.progress.min(100);
        snapshot.error = snapshot
            .error
            .as_deref()
            .map(|message| truncate_chars(message.trim(), MAX_PROVIDER_ERROR_CHARS))
            .filter(|message| !message.is_empty());

        let requested: BTreeSet<&str> = request
            .target_formats
            .iter()
            .map(|format| format.as_key())
            .collect();
        snapshot
            .artifacts
            .retain(|format, _| requested.contains(format.as_str()));
        for (format, url) in &snapshot.artifacts {
            validate_https_url(url, &format!("provider artifact {format}"))?;
        }

        match snapshot.status {
            ProviderTaskStatus::Pending | ProviderTaskStatus::InProgress => {
                checkpoint.provider = Some(snapshot);
                Ok(PollOutcome::Waiting { checkpoint })
            }
            ProviderTaskStatus::Succeeded => {
                let missing: Vec<&str> = request
                    .target_formats
                    .iter()
                    .map(|format| format.as_key())
                    .filter(|format| !snapshot.artifacts.contains_key(*format))
                    .collect();
                if !missing.is_empty() {
                    return Err(MeshyJobError::new(
                        "provider_outputs_missing",
                        format!(
                            "provider succeeded without requested outputs: {}",
                            missing.join(", ")
                        ),
                        false,
                    ));
                }
                checkpoint.provider = Some(snapshot);
                Ok(PollOutcome::Succeeded { checkpoint })
            }
            ProviderTaskStatus::Failed => Err(MeshyJobError::new(
                "provider_task_failed",
                snapshot
                    .error
                    .unwrap_or_else(|| "Meshy generation failed".to_string()),
                false,
            )),
            ProviderTaskStatus::Canceled => Err(MeshyJobError::new(
                "provider_task_canceled",
                "Meshy generation was canceled",
                false,
            )),
            ProviderTaskStatus::Unknown => Err(MeshyJobError::new(
                "provider_task_status_unknown",
                "Meshy returned an unknown task status",
                true,
            )),
        }
    }

    pub async fn archive_next<F, A>(
        &self,
        fetcher: &F,
        archive: &A,
        identity: &JobIdentity,
        request: &MeshyJobRequest,
        mut checkpoint: MeshyJobCheckpoint,
    ) -> MeshyJobResultValue<ArchiveOutcome>
    where
        F: ArtifactFetcher + ?Sized,
        A: ArtifactArchive + ?Sized,
    {
        identity.validate()?;
        checkpoint.validate_for_request(request)?;
        let provider = checkpoint.provider.as_ref().ok_or_else(|| {
            MeshyJobError::new(
                "provider_snapshot_missing",
                "provider must succeed before artifacts can be archived",
                false,
            )
        })?;
        if provider.status != ProviderTaskStatus::Succeeded {
            return Err(MeshyJobError::new(
                "provider_not_succeeded",
                "provider must succeed before artifacts can be archived",
                false,
            ));
        }

        let next = request
            .target_formats
            .iter()
            .find(|format| !checkpoint.archived_artifacts.contains_key(format.as_key()));
        let Some(format) = next.copied() else {
            return Ok(ArchiveOutcome::Complete {
                result: self.result(identity, request, &checkpoint)?,
            });
        };
        let format_key = format.as_key();
        let source_url = provider.artifacts.get(format_key).ok_or_else(|| {
            MeshyJobError::new(
                "provider_artifact_missing",
                format!("provider artifact {format_key} is missing"),
                false,
            )
        })?;
        let task_id = checkpoint.provider_task_id().ok_or_else(|| {
            MeshyJobError::new(
                "provider_task_id_missing",
                "durable provider task id is missing",
                false,
            )
        })?;
        let object_key = archive_object_key(identity, request, task_id, format)?;
        let fetched = fetcher
            .fetch(FetchRequest {
                source_url: source_url.clone(),
                staging_key: object_key.clone(),
                max_bytes: request.max_artifact_bytes,
                format,
            })
            .await?;
        validate_fetched_artifact(&fetched, format).await?;
        let artifact = archive.put_verified(&object_key, &fetched).await?;
        if artifact.sha256 != fetched.sha256 || artifact.byte_count != fetched.byte_count {
            return Err(MeshyJobError::new(
                "archive_receipt_mismatch",
                "artifact archive returned a digest or byte count that differs from the downloaded bytes",
                false,
            ));
        }
        checkpoint
            .archived_artifacts
            .insert(format_key.to_string(), artifact.clone());
        Ok(ArchiveOutcome::Archived {
            format: format_key.to_string(),
            artifact,
            checkpoint,
        })
    }

    pub fn result(
        &self,
        identity: &JobIdentity,
        request: &MeshyJobRequest,
        checkpoint: &MeshyJobCheckpoint,
    ) -> MeshyJobResultValue<MeshyJobResult> {
        identity.validate()?;
        checkpoint.validate_for_request(request)?;
        let provider = checkpoint.provider.as_ref().ok_or_else(|| {
            MeshyJobError::new(
                "provider_snapshot_missing",
                "provider snapshot is missing",
                false,
            )
        })?;
        if provider.status != ProviderTaskStatus::Succeeded {
            return Err(MeshyJobError::new(
                "provider_not_succeeded",
                "provider task is not successful",
                false,
            ));
        }
        for format in &request.target_formats {
            if !checkpoint.archived_artifacts.contains_key(format.as_key()) {
                return Err(MeshyJobError::new(
                    "artifact_archive_incomplete",
                    format!("artifact {} has not been archived", format.as_key()),
                    false,
                ));
            }
        }
        let task_id = checkpoint.provider_task_id().ok_or_else(|| {
            MeshyJobError::new(
                "provider_task_id_missing",
                "durable provider task id is missing",
                false,
            )
        })?;
        Ok(MeshyJobResult {
            schema_version: RESULT_SCHEMA.to_string(),
            provider: "meshy".to_string(),
            provider_task_id: task_id.to_string(),
            provider_task_status: provider.status,
            provider_expires_at: provider.expires_at,
            consumed_credits: provider.consumed_credits,
            source_images: request.images.clone(),
            artifacts: checkpoint.archived_artifacts.clone(),
            purpose: request.purpose.trim().to_string(),
            review_state: "needs_review".to_string(),
            release_state: "blocked".to_string(),
            machine_ready: false,
            required_evidence: required_evidence(),
        })
    }
}

#[async_trait]
pub trait GeometryProvider: Send + Sync {
    async fn submit(&self, request: &MeshyJobRequest) -> MeshyJobResultValue<String>;

    async fn get(
        &self,
        mode: GenerationMode,
        task_id: &str,
    ) -> MeshyJobResultValue<ProviderTaskSnapshot>;
}

#[derive(Clone)]
pub struct MeshyGeometryProvider {
    client: MeshyClient,
}

impl fmt::Debug for MeshyGeometryProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MeshyGeometryProvider")
            .field("client", &self.client)
            .finish()
    }
}

impl MeshyGeometryProvider {
    pub fn from_env() -> MeshyJobResultValue<Self> {
        let client = MeshyClient::from_env().map_err(|error| map_meshy_error(error, false))?;
        Ok(Self { client })
    }

    pub fn new(client: MeshyClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl GeometryProvider for MeshyGeometryProvider {
    async fn submit(&self, request: &MeshyJobRequest) -> MeshyJobResultValue<String> {
        request.validate()?;
        let response = match request.mode() {
            GenerationMode::ImageTo3d => {
                self.client
                    .create_image_task(&image_request(request)?)
                    .await
            }
            GenerationMode::MultiImageTo3d => {
                self.client
                    .create_multi_image_task(&multi_image_request(request)?)
                    .await
            }
        }
        .map_err(|error| map_meshy_error(error, true))?;
        validate_task_id(&response.result)?;
        Ok(response.result.trim().to_string())
    }

    async fn get(
        &self,
        mode: GenerationMode,
        task_id: &str,
    ) -> MeshyJobResultValue<ProviderTaskSnapshot> {
        validate_task_id(task_id)?;
        let task = self
            .client
            .get_task(mode.task_kind(), task_id)
            .await
            .map_err(|error| map_meshy_error(error, false))?;
        let status = match task.status {
            TaskStatus::Pending => ProviderTaskStatus::Pending,
            TaskStatus::InProgress => ProviderTaskStatus::InProgress,
            TaskStatus::Succeeded => ProviderTaskStatus::Succeeded,
            TaskStatus::Failed => ProviderTaskStatus::Failed,
            TaskStatus::Canceled | TaskStatus::Cancelled => ProviderTaskStatus::Canceled,
            TaskStatus::Unknown => ProviderTaskStatus::Unknown,
        };
        Ok(ProviderTaskSnapshot {
            id: task.id,
            status,
            progress: task.progress,
            artifacts: task.model_urls.requested_artifacts(),
            thumbnail_url: task.thumbnail_url,
            expires_at: task.expires_at,
            error: task
                .task_error
                .map(|error| truncate_chars(error.message.trim(), MAX_PROVIDER_ERROR_CHARS))
                .filter(|message| !message.is_empty()),
            consumed_credits: task.consumed_credits,
        })
    }
}

fn image_request(request: &MeshyJobRequest) -> MeshyJobResultValue<ImageTo3dRequest> {
    request.validate()?;
    let smart_topology = matches!(request.ai_model, Some(AiModel::MeshyT1 | AiModel::MeshyT2));
    Ok(ImageTo3dRequest {
        image_url: Some(request.images[0].url.trim().to_string()),
        model_type: smart_topology.then_some(ModelType::SmartTopology),
        ai_model: request.ai_model,
        options: generation_options(request, smart_topology),
        ..Default::default()
    })
}

fn multi_image_request(request: &MeshyJobRequest) -> MeshyJobResultValue<MultiImageTo3dRequest> {
    request.validate()?;
    Ok(MultiImageTo3dRequest {
        image_urls: Some(
            request
                .images
                .iter()
                .map(|image| image.url.trim().to_string())
                .collect(),
        ),
        ai_model: request.ai_model,
        options: generation_options(request, false),
        ..Default::default()
    })
}

fn generation_options(request: &MeshyJobRequest, smart_topology: bool) -> GenerationOptions {
    GenerationOptions {
        should_texture: Some(request.should_texture),
        should_remesh: if smart_topology {
            None
        } else {
            Some(request.should_remesh)
        },
        target_formats: Some(request.target_formats.clone()),
        ..Default::default()
    }
}

fn map_meshy_error(error: MeshyError, during_submit: bool) -> MeshyJobError {
    match error {
        MeshyError::Configuration(_) => MeshyJobError::new(
            "meshy_configuration_error",
            "Meshy provider configuration is invalid",
            false,
        ),
        MeshyError::Validation(_) => MeshyJobError::new(
            "meshy_request_validation_error",
            "Meshy rejected the request before it was sent",
            false,
        ),
        MeshyError::Transport(_) => {
            let error = MeshyJobError::new("meshy_transport_error", "Meshy transport failed", true);
            if during_submit {
                error.ambiguous()
            } else {
                error
            }
        }
        MeshyError::Api {
            status,
            code,
            retry_after_seconds,
            ..
        } => {
            let retryable = status == 408 || status == 425 || status == 429 || status >= 500;
            let message = match code {
                Some(code) => format!("Meshy returned HTTP {status} ({code})"),
                None => format!("Meshy returned HTTP {status}"),
            };
            let error = MeshyJobError::new("meshy_api_error", message, retryable)
                .retry_after(retry_after_seconds);
            if during_submit && (status == 408 || status >= 500) {
                error.ambiguous()
            } else {
                error
            }
        }
        MeshyError::Decode { status, .. } => {
            let error = MeshyJobError::new(
                "meshy_response_decode_error",
                format!("Meshy returned an unreadable HTTP {status} response"),
                true,
            );
            if during_submit {
                error.ambiguous()
            } else {
                error
            }
        }
        MeshyError::Timeout { .. } => {
            let error = MeshyJobError::new("meshy_timeout", "Meshy task operation timed out", true);
            if during_submit {
                error.ambiguous()
            } else {
                error
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub source_url: String,
    pub staging_key: String,
    pub max_bytes: u64,
    pub format: TargetFormat,
}

#[derive(Debug, Clone)]
pub struct FetchedArtifact {
    pub path: PathBuf,
    pub sha256: String,
    pub byte_count: u64,
    pub media_type: Option<String>,
}

#[async_trait]
pub trait ArtifactFetcher: Send + Sync {
    async fn fetch(&self, request: FetchRequest) -> MeshyJobResultValue<FetchedArtifact>;
}

#[derive(Debug, Clone)]
pub struct ArtifactUrlPolicy {
    allowed_host_suffixes: Vec<String>,
}

impl ArtifactUrlPolicy {
    pub fn new<I, S>(allowed_host_suffixes: I) -> MeshyJobResultValue<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut suffixes = BTreeSet::new();
        for suffix in allowed_host_suffixes {
            let suffix = suffix
                .into()
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if suffix.is_empty()
                || suffix.len() > 253
                || suffix
                    .bytes()
                    .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.')))
            {
                return Err(MeshyJobError::new(
                    "invalid_artifact_host_policy",
                    "artifact host suffixes must be valid DNS suffixes",
                    false,
                ));
            }
            suffixes.insert(suffix);
        }
        if suffixes.is_empty() {
            return Err(MeshyJobError::new(
                "artifact_host_policy_required",
                "at least one Meshy artifact host suffix is required",
                false,
            ));
        }
        Ok(Self {
            allowed_host_suffixes: suffixes.into_iter().collect(),
        })
    }

    pub fn from_env() -> MeshyJobResultValue<Self> {
        let value = env::var("MESHY_ARTIFACT_ALLOWED_HOSTS").map_err(|_| {
            MeshyJobError::new(
                "artifact_host_policy_required",
                "MESHY_ARTIFACT_ALLOWED_HOSTS is required",
                false,
            )
        })?;
        Self::new(value.split(',').map(str::to_string))
    }

    pub fn validate(&self, value: &str) -> MeshyJobResultValue<Url> {
        let url = Url::parse(value).map_err(|_| {
            MeshyJobError::new(
                "invalid_artifact_url",
                "provider artifact URL is invalid",
                false,
            )
        })?;
        if url.scheme() != "https" {
            return Err(MeshyJobError::new(
                "insecure_artifact_url",
                "provider artifact URL must use HTTPS",
                false,
            ));
        }
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(MeshyJobError::new(
                "invalid_artifact_url",
                "provider artifact URL must not contain credentials or a fragment",
                false,
            ));
        }
        if url.port_or_known_default() != Some(443) {
            return Err(MeshyJobError::new(
                "invalid_artifact_port",
                "provider artifact URL must use HTTPS port 443",
                false,
            ));
        }
        let host = url.host_str().ok_or_else(|| {
            MeshyJobError::new(
                "invalid_artifact_host",
                "provider artifact URL has no host",
                false,
            )
        })?;
        if host.parse::<IpAddr>().is_ok() {
            return Err(MeshyJobError::new(
                "artifact_ip_literal_rejected",
                "provider artifact URLs must use an allowlisted DNS host, not an IP literal",
                false,
            ));
        }
        let host = host.to_ascii_lowercase();
        let allowed = self
            .allowed_host_suffixes
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
        if !allowed {
            return Err(MeshyJobError::new(
                "artifact_host_not_allowed",
                format!("provider artifact host {host:?} is not allowlisted"),
                false,
            ));
        }
        Ok(url)
    }
}

#[derive(Clone)]
pub struct BoundedHttpFetcher {
    client: reqwest::Client,
    staging_dir: PathBuf,
    policy: ArtifactUrlPolicy,
}

impl fmt::Debug for BoundedHttpFetcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedHttpFetcher")
            .field("staging_dir", &self.staging_dir)
            .field("policy", &self.policy)
            .finish()
    }
}

impl BoundedHttpFetcher {
    pub fn new(
        staging_dir: impl Into<PathBuf>,
        policy: ArtifactUrlPolicy,
        timeout: Duration,
    ) -> MeshyJobResultValue<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(24 * 60 * 60) {
            return Err(MeshyJobError::new(
                "invalid_artifact_timeout",
                "artifact timeout must be between 1 second and 24 hours",
                false,
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("dd-meshy-job/artifact-fetcher")
            .build()
            .map_err(|_| {
                MeshyJobError::new(
                    "artifact_client_configuration_failed",
                    "could not configure artifact HTTP client",
                    false,
                )
            })?;
        Ok(Self {
            client,
            staging_dir: staging_dir.into(),
            policy,
        })
    }

    pub fn from_env() -> MeshyJobResultValue<Self> {
        let staging_dir = env::var("MESHY_ARTIFACT_STAGING_DIR")
            .unwrap_or_else(|_| "/var/lib/daedalus/meshy-staging".to_string());
        let timeout_secs = env::var("MESHY_ARTIFACT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_ARTIFACT_TIMEOUT_SECS);
        Self::new(
            staging_dir,
            ArtifactUrlPolicy::from_env()?,
            Duration::from_secs(timeout_secs),
        )
    }
}

#[async_trait]
impl ArtifactFetcher for BoundedHttpFetcher {
    async fn fetch(&self, request: FetchRequest) -> MeshyJobResultValue<FetchedArtifact> {
        if !(MIN_MAX_ARTIFACT_BYTES..=MAX_MAX_ARTIFACT_BYTES).contains(&request.max_bytes) {
            return Err(MeshyJobError::new(
                "invalid_artifact_limit",
                "artifact byte limit is outside the supported range",
                false,
            ));
        }
        validate_object_key(&request.staging_key)?;
        let url = self.policy.validate(&request.source_url)?;
        tokio::fs::create_dir_all(&self.staging_dir)
            .await
            .map_err(|_| {
                MeshyJobError::new(
                    "artifact_staging_unavailable",
                    "could not create artifact staging directory",
                    true,
                )
            })?;

        let response = self.client.get(url).send().await.map_err(|_| {
            MeshyJobError::new(
                "artifact_download_transport_error",
                "artifact download transport failed",
                true,
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(MeshyJobError::new(
                "artifact_download_http_error",
                format!("artifact download returned HTTP {}", status.as_u16()),
                status.as_u16() == 408 || status.as_u16() == 429 || status.is_server_error(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > request.max_bytes)
        {
            return Err(MeshyJobError::new(
                "artifact_too_large",
                "artifact Content-Length exceeds the configured byte limit",
                false,
            ));
        }
        let media_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .to_ascii_lowercase()
            })
            .filter(|value| !value.is_empty());

        let staging_name = format!(
            "{}.{}.part",
            sha256_hex(request.staging_key.as_bytes()),
            request.format.as_key()
        );
        let path = self.staging_dir.join(staging_name);
        let mut file = tokio::fs::File::create(&path).await.map_err(|_| {
            MeshyJobError::new(
                "artifact_staging_unavailable",
                "could not open artifact staging file",
                true,
            )
        })?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        let mut byte_count = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                MeshyJobError::new(
                    "artifact_download_transport_error",
                    "artifact download stream failed",
                    true,
                )
            })?;
            byte_count = byte_count.checked_add(chunk.len() as u64).ok_or_else(|| {
                MeshyJobError::new(
                    "artifact_size_overflow",
                    "artifact byte count overflowed",
                    false,
                )
            })?;
            if byte_count > request.max_bytes {
                return Err(MeshyJobError::new(
                    "artifact_too_large",
                    "artifact exceeded the configured byte limit while streaming",
                    false,
                ));
            }
            file.write_all(&chunk).await.map_err(|_| {
                MeshyJobError::new(
                    "artifact_staging_write_failed",
                    "could not write artifact staging file",
                    true,
                )
            })?;
            hasher.update(&chunk);
        }
        file.flush().await.map_err(|_| {
            MeshyJobError::new(
                "artifact_staging_write_failed",
                "could not flush artifact staging file",
                true,
            )
        })?;
        file.sync_all().await.map_err(|_| {
            MeshyJobError::new(
                "artifact_staging_sync_failed",
                "could not durably sync artifact staging file",
                true,
            )
        })?;
        if byte_count == 0 {
            return Err(MeshyJobError::new(
                "empty_artifact",
                "provider artifact is empty",
                false,
            ));
        }
        Ok(FetchedArtifact {
            path,
            sha256: hex::encode(hasher.finalize()),
            byte_count,
            media_type,
        })
    }
}

#[async_trait]
pub trait ArtifactArchive: Send + Sync {
    async fn put_verified(
        &self,
        object_key: &str,
        artifact: &FetchedArtifact,
    ) -> MeshyJobResultValue<ArchivedArtifact>;
}

#[derive(Debug, Clone)]
pub struct DirectoryArchive {
    root: PathBuf,
    uri_prefix: String,
}

impl DirectoryArchive {
    pub fn new(
        root: impl Into<PathBuf>,
        uri_prefix: impl Into<String>,
    ) -> MeshyJobResultValue<Self> {
        let uri_prefix = uri_prefix.into();
        if uri_prefix.trim().is_empty() || uri_prefix.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(MeshyJobError::new(
                "invalid_archive_uri_prefix",
                "archive URI prefix must not be empty or contain control characters",
                false,
            ));
        }
        Ok(Self {
            root: root.into(),
            uri_prefix: uri_prefix.trim_end_matches('/').to_string(),
        })
    }

    pub fn from_env() -> MeshyJobResultValue<Self> {
        let root = env::var("DAEDALUS_ARTIFACT_ARCHIVE_DIR")
            .unwrap_or_else(|_| "/var/lib/daedalus/artifacts".to_string());
        let uri_prefix = env::var("DAEDALUS_ARTIFACT_URI_PREFIX")
            .unwrap_or_else(|_| "daedalus-artifact://archive".to_string());
        Self::new(root, uri_prefix)
    }

    fn receipt(&self, object_key: &str, artifact: &FetchedArtifact) -> ArchivedArtifact {
        ArchivedArtifact {
            object_key: object_key.to_string(),
            archive_uri: format!("{}/{}", self.uri_prefix, object_key),
            sha256: artifact.sha256.clone(),
            byte_count: artifact.byte_count,
            media_type: artifact.media_type.clone(),
        }
    }
}

#[async_trait]
impl ArtifactArchive for DirectoryArchive {
    async fn put_verified(
        &self,
        object_key: &str,
        artifact: &FetchedArtifact,
    ) -> MeshyJobResultValue<ArchivedArtifact> {
        validate_object_key(object_key)?;
        validate_sha256("artifact sha256", &artifact.sha256)?;
        let final_path = self.root.join(object_key);
        if let Some(parent) = final_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|_| {
                MeshyJobError::new(
                    "archive_directory_unavailable",
                    "could not create artifact archive directory",
                    true,
                )
            })?;
        }

        if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
            verify_existing_file(&final_path, artifact).await?;
            return Ok(self.receipt(object_key, artifact));
        }

        let file_name = final_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                MeshyJobError::new(
                    "invalid_archive_object_key",
                    "archive object key has no valid file name",
                    false,
                )
            })?;
        let temporary =
            final_path.with_file_name(format!(".{file_name}.{}.partial", &artifact.sha256[..16]));
        tokio::fs::copy(&artifact.path, &temporary)
            .await
            .map_err(|_| {
                MeshyJobError::new(
                    "archive_copy_failed",
                    "could not copy artifact into the archive",
                    true,
                )
            })?;
        let temporary_file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&temporary)
            .await
            .map_err(|_| {
                MeshyJobError::new(
                    "archive_sync_failed",
                    "could not open temporary archive file for sync",
                    true,
                )
            })?;
        temporary_file.sync_all().await.map_err(|_| {
            MeshyJobError::new(
                "archive_sync_failed",
                "could not durably sync temporary archive file",
                true,
            )
        })?;
        verify_existing_file(&temporary, artifact).await?;
        match tokio::fs::rename(&temporary, &final_path).await {
            Ok(()) => {}
            Err(_) if tokio::fs::try_exists(&final_path).await.unwrap_or(false) => {
                verify_existing_file(&final_path, artifact).await?;
            }
            Err(_) => {
                return Err(MeshyJobError::new(
                    "archive_commit_failed",
                    "could not atomically commit artifact into the archive",
                    true,
                ))
            }
        }
        verify_existing_file(&final_path, artifact).await?;
        Ok(self.receipt(object_key, artifact))
    }
}

async fn verify_existing_file(path: &Path, expected: &FetchedArtifact) -> MeshyJobResultValue<()> {
    let (sha256, byte_count) = hash_file(path).await?;
    if sha256 != expected.sha256 || byte_count != expected.byte_count {
        return Err(MeshyJobError::new(
            "archive_object_conflict",
            "archive object key already exists with different bytes",
            false,
        ));
    }
    Ok(())
}

async fn hash_file(path: &Path) -> MeshyJobResultValue<(String, u64)> {
    let mut file = tokio::fs::File::open(path).await.map_err(|_| {
        MeshyJobError::new(
            "archive_read_failed",
            "could not read artifact archive object",
            true,
        )
    })?;
    let mut buffer = vec![0_u8; 128 * 1024];
    let mut hasher = Sha256::new();
    let mut byte_count = 0_u64;
    loop {
        let read = file.read(&mut buffer).await.map_err(|_| {
            MeshyJobError::new(
                "archive_read_failed",
                "could not read artifact archive object",
                true,
            )
        })?;
        if read == 0 {
            break;
        }
        byte_count = byte_count.checked_add(read as u64).ok_or_else(|| {
            MeshyJobError::new(
                "artifact_size_overflow",
                "artifact byte count overflowed",
                false,
            )
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok((hex::encode(hasher.finalize()), byte_count))
}

async fn validate_fetched_artifact(
    artifact: &FetchedArtifact,
    format: TargetFormat,
) -> MeshyJobResultValue<()> {
    validate_sha256("artifact sha256", &artifact.sha256)?;
    if artifact.byte_count == 0 {
        return Err(MeshyJobError::new(
            "empty_artifact",
            "downloaded artifact is empty",
            false,
        ));
    }
    if artifact.media_type.as_deref().is_some_and(|media_type| {
        matches!(
            media_type,
            "text/html" | "application/json" | "text/json" | "application/problem+json"
        )
    }) {
        return Err(MeshyJobError::new(
            "unexpected_artifact_media_type",
            "provider returned an HTML or JSON document instead of a model artifact",
            false,
        ));
    }

    let mut file = tokio::fs::File::open(&artifact.path).await.map_err(|_| {
        MeshyJobError::new(
            "artifact_staging_read_failed",
            "could not inspect downloaded artifact",
            true,
        )
    })?;
    let mut prefix = [0_u8; 32];
    let read = file.read(&mut prefix).await.map_err(|_| {
        MeshyJobError::new(
            "artifact_staging_read_failed",
            "could not inspect downloaded artifact",
            true,
        )
    })?;
    let prefix = &prefix[..read];
    let valid = match format {
        TargetFormat::Glb => prefix.starts_with(b"glTF"),
        TargetFormat::ThreeMf | TargetFormat::Usdz => prefix.starts_with(b"PK"),
        TargetFormat::Stl => artifact.byte_count >= 84 || prefix.starts_with(b"solid"),
        TargetFormat::Obj | TargetFormat::Fbx => !prefix.is_empty(),
    };
    if !valid {
        return Err(MeshyJobError::new(
            "artifact_signature_mismatch",
            format!(
                "downloaded bytes do not resemble the requested {} format",
                format.as_key()
            ),
            false,
        ));
    }
    Ok(())
}

fn archive_object_key(
    identity: &JobIdentity,
    request: &MeshyJobRequest,
    task_id: &str,
    format: TargetFormat,
) -> MeshyJobResultValue<String> {
    identity.validate()?;
    request.validate()?;
    validate_task_id(task_id)?;
    let tenant = &sha256_hex(identity.tenant_id.trim().as_bytes())[..24];
    let task = &sha256_hex(task_id.trim().as_bytes())[..24];
    let key = format!(
        "{}/tenant-{tenant}/{}/task-{task}/{}.{}",
        request.archive_prefix.trim_matches('/'),
        identity.job_id.trim(),
        format.as_key(),
        format.as_key()
    );
    validate_object_key(&key)?;
    Ok(key)
}

fn required_evidence() -> Vec<String> {
    [
        "source image ownership and SHA-256 provenance",
        "durable GLB/STL/3MF archive receipts and SHA-256 checksums",
        "scale, units, and coordinate-frame evidence",
        "mesh repair, manifold, normals, and wall-thickness review",
        "orientation, support, material, slicer, and machine-profile review",
        "simulation or dry-run evidence and explicit release authorization",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn validate_identifier(name: &str, value: &str) -> MeshyJobResultValue<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(MeshyJobError::new(
            "invalid_identifier",
            format!("{name} must be 1-{MAX_IDENTIFIER_BYTES} bytes"),
            false,
        ));
    }
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(MeshyJobError::new(
            "invalid_identifier",
            format!("{name} must not contain control characters"),
            false,
        ));
    }
    Ok(())
}

fn validate_task_id(value: &str) -> MeshyJobResultValue<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(MeshyJobError::new(
            "invalid_provider_task_id",
            "provider task id contains unsupported characters or length",
            false,
        ));
    }
    Ok(())
}

fn validate_sha256(name: &str, value: &str) -> MeshyJobResultValue<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(MeshyJobError::new(
            "invalid_sha256",
            format!("{name} must be 64 lowercase hexadecimal characters"),
            false,
        ));
    }
    Ok(())
}

fn validate_https_url(value: &str, field: &str) -> MeshyJobResultValue<()> {
    if value.len() > MAX_URL_BYTES || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(MeshyJobError::new(
            "invalid_https_url",
            format!("{field} is empty, oversized, or contains control characters"),
            false,
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        MeshyJobError::new(
            "invalid_https_url",
            format!("{field} is not a valid URL"),
            false,
        )
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(MeshyJobError::new(
            "invalid_https_url",
            format!("{field} must be an HTTPS URL without credentials or a fragment"),
            false,
        ));
    }
    Ok(())
}

fn validate_object_key(value: &str) -> MeshyJobResultValue<()> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || value.len() > 1024
        || path.is_absolute()
        || value.contains('\\')
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(MeshyJobError::new(
            "invalid_archive_object_key",
            "archive object key must be a bounded relative slash-separated path",
            false,
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(value) if !value.is_empty() => {}
            _ => {
                return Err(MeshyJobError::new(
                    "invalid_archive_object_key",
                    "archive object key must not contain '.', '..', roots, or prefixes",
                    false,
                ))
            }
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    use tokio::sync::Mutex;

    use super::*;

    fn source_hash() -> String {
        "a".repeat(64)
    }

    fn identity() -> JobIdentity {
        JobIdentity {
            tenant_id: "tenant-acme".to_string(),
            job_id: "c22e13a1-0fc8-430f-a78b-8cb73a687f9d".to_string(),
        }
    }

    fn request() -> MeshyJobRequest {
        MeshyJobRequest {
            schema_version: JOB_SCHEMA.to_string(),
            images: vec![SourceImage {
                asset_id: "front".to_string(),
                url: "https://assets.example/front.png".to_string(),
                sha256: source_hash(),
            }],
            target_formats: default_target_formats(),
            should_texture: true,
            should_remesh: false,
            ai_model: Some(AiModel::Meshy6),
            purpose: "additive-prototype".to_string(),
            archive_prefix: "meshy".to_string(),
            max_artifact_bytes: MIN_MAX_ARTIFACT_BYTES,
            existing_provider_task: None,
        }
    }

    #[derive(Clone)]
    struct MockProvider {
        submits: Arc<AtomicUsize>,
        submit_error: Option<MeshyJobError>,
        snapshot: ProviderTaskSnapshot,
    }

    #[async_trait]
    impl GeometryProvider for MockProvider {
        async fn submit(&self, _request: &MeshyJobRequest) -> MeshyJobResultValue<String> {
            self.submits.fetch_add(1, Ordering::SeqCst);
            match &self.submit_error {
                Some(error) => Err(error.clone()),
                None => Ok("provider-task-1".to_string()),
            }
        }

        async fn get(
            &self,
            _mode: GenerationMode,
            _task_id: &str,
        ) -> MeshyJobResultValue<ProviderTaskSnapshot> {
            Ok(self.snapshot.clone())
        }
    }

    struct MemoryFetcher {
        values: BTreeMap<String, Vec<u8>>,
        root: tempfile::TempDir,
    }

    #[async_trait]
    impl ArtifactFetcher for MemoryFetcher {
        async fn fetch(&self, request: FetchRequest) -> MeshyJobResultValue<FetchedArtifact> {
            let bytes = self.values.get(&request.source_url).ok_or_else(|| {
                MeshyJobError::new("mock_artifact_missing", "mock artifact missing", false)
            })?;
            let path = self
                .root
                .path()
                .join(format!("{}.bin", request.format.as_key()));
            tokio::fs::write(&path, bytes).await.map_err(|error| {
                MeshyJobError::new("mock_artifact_write_failed", error.to_string(), false)
            })?;
            Ok(FetchedArtifact {
                path,
                sha256: sha256_hex(bytes),
                byte_count: bytes.len() as u64,
                media_type: Some("application/octet-stream".to_string()),
            })
        }
    }

    #[derive(Default)]
    struct MemoryArchive {
        values: Mutex<BTreeMap<String, ArchivedArtifact>>,
    }

    #[async_trait]
    impl ArtifactArchive for MemoryArchive {
        async fn put_verified(
            &self,
            object_key: &str,
            artifact: &FetchedArtifact,
        ) -> MeshyJobResultValue<ArchivedArtifact> {
            let receipt = ArchivedArtifact {
                object_key: object_key.to_string(),
                archive_uri: format!("memory://{object_key}"),
                sha256: artifact.sha256.clone(),
                byte_count: artifact.byte_count,
                media_type: artifact.media_type.clone(),
            };
            let mut values = self.values.lock().await;
            if let Some(existing) = values.get(object_key) {
                if existing.sha256 != receipt.sha256 {
                    return Err(MeshyJobError::new(
                        "archive_object_conflict",
                        "memory object conflict",
                        false,
                    ));
                }
                return Ok(existing.clone());
            }
            values.insert(object_key.to_string(), receipt.clone());
            Ok(receipt)
        }
    }

    fn succeeded_snapshot() -> ProviderTaskSnapshot {
        ProviderTaskSnapshot {
            id: "provider-task-1".to_string(),
            status: ProviderTaskStatus::Succeeded,
            progress: 100,
            artifacts: BTreeMap::from([
                (
                    "glb".to_string(),
                    "https://cdn.meshy.example/model.glb".to_string(),
                ),
                (
                    "stl".to_string(),
                    "https://cdn.meshy.example/model.stl".to_string(),
                ),
                (
                    "3mf".to_string(),
                    "https://cdn.meshy.example/model.3mf".to_string(),
                ),
                (
                    "obj".to_string(),
                    "https://cdn.meshy.example/unrequested.obj".to_string(),
                ),
            ]),
            thumbnail_url: None,
            expires_at: Some(1_800_000_000),
            error: None,
            consumed_credits: Some(12),
        }
    }

    #[tokio::test]
    async fn persisted_intent_cannot_be_resubmitted_after_process_loss() {
        let engine = MeshyJobEngine;
        let preparation = engine
            .prepare(&identity(), &request(), 100)
            .expect("prepare");
        let checkpoint = match preparation {
            Preparation::Submit { checkpoint, .. } => checkpoint,
            Preparation::Adopted { .. } => panic!("unexpected adopted task"),
        };
        let serialized = checkpoint.to_value().expect("serialize");
        let resumed = MeshyJobCheckpoint::from_value(&serialized)
            .expect("parse")
            .expect("checkpoint");
        let provider = MockProvider {
            submits: Arc::new(AtomicUsize::new(0)),
            submit_error: None,
            snapshot: succeeded_snapshot(),
        };
        let error = engine
            .poll(&provider, &request(), resumed)
            .await
            .expect_err("unreconciled submission must stop");
        assert_eq!(error.code, "provider_submission_unreconciled");
        assert!(error.submission_ambiguous);
        assert_eq!(provider.submits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ambiguous_create_is_checkpointed_and_never_retried_automatically() {
        let engine = MeshyJobEngine;
        let (checkpoint, permit) = match engine.prepare(&identity(), &request(), 100).unwrap() {
            Preparation::Submit { checkpoint, permit } => (checkpoint, permit),
            Preparation::Adopted { .. } => panic!("unexpected adopted task"),
        };
        let provider = MockProvider {
            submits: Arc::new(AtomicUsize::new(0)),
            submit_error: Some(
                MeshyJobError::new("meshy_transport_error", "transport failed", true).ambiguous(),
            ),
            snapshot: succeeded_snapshot(),
        };
        let outcome = engine
            .submit_prepared(&provider, &request(), checkpoint, permit, 200)
            .await
            .expect("ambiguous outcome is persistable");
        let checkpoint = match outcome {
            SubmissionOutcome::Ambiguous { checkpoint, error } => {
                assert!(error.submission_ambiguous);
                checkpoint
            }
            SubmissionOutcome::Submitted { .. } => panic!("unexpected submission"),
        };
        assert!(matches!(
            checkpoint.submission,
            SubmissionCheckpoint::Ambiguous { .. }
        ));
        let error = engine
            .poll(&provider, &request(), checkpoint)
            .await
            .expect_err("ambiguous checkpoint must require reconciliation");
        assert_eq!(error.code, "provider_submission_requires_reconciliation");
        assert_eq!(provider.submits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn successful_job_archives_only_requested_outputs_and_stays_release_blocked() {
        let engine = MeshyJobEngine;
        let (checkpoint, permit) = match engine.prepare(&identity(), &request(), 100).unwrap() {
            Preparation::Submit { checkpoint, permit } => (checkpoint, permit),
            Preparation::Adopted { .. } => panic!("unexpected adopted task"),
        };
        let provider = MockProvider {
            submits: Arc::new(AtomicUsize::new(0)),
            submit_error: None,
            snapshot: succeeded_snapshot(),
        };
        let checkpoint = match engine
            .submit_prepared(&provider, &request(), checkpoint, permit, 200)
            .await
            .unwrap()
        {
            SubmissionOutcome::Submitted { checkpoint } => checkpoint,
            SubmissionOutcome::Ambiguous { .. } => panic!("unexpected ambiguity"),
        };
        let mut checkpoint = match engine
            .poll(&provider, &request(), checkpoint)
            .await
            .unwrap()
        {
            PollOutcome::Succeeded { checkpoint } => checkpoint,
            PollOutcome::Waiting { .. } => panic!("unexpected waiting"),
        };
        assert_eq!(
            checkpoint.provider.as_ref().unwrap().artifacts.len(),
            3,
            "unrequested OBJ is filtered before checkpointing"
        );

        let mut glb = b"glTF".to_vec();
        glb.extend(vec![0_u8; 128]);
        let mut stl = vec![0_u8; 84];
        stl.extend_from_slice(b"triangle");
        let mut three_mf = b"PK\x03\x04".to_vec();
        three_mf.extend(vec![0_u8; 128]);
        let fetcher = MemoryFetcher {
            values: BTreeMap::from([
                ("https://cdn.meshy.example/model.glb".to_string(), glb),
                ("https://cdn.meshy.example/model.stl".to_string(), stl),
                ("https://cdn.meshy.example/model.3mf".to_string(), three_mf),
            ]),
            root: tempfile::tempdir().unwrap(),
        };
        let archive = MemoryArchive::default();
        let result = loop {
            match engine
                .archive_next(&fetcher, &archive, &identity(), &request(), checkpoint)
                .await
                .unwrap()
            {
                ArchiveOutcome::Archived {
                    checkpoint: next, ..
                } => checkpoint = next,
                ArchiveOutcome::Complete { result } => break result,
            }
        };
        assert_eq!(result.artifacts.len(), 3);
        assert_eq!(result.review_state, "needs_review");
        assert_eq!(result.release_state, "blocked");
        assert!(!result.machine_ready);
        assert_eq!(result.consumed_credits, Some(12));
        assert_eq!(result.source_images[0].sha256, source_hash());
    }

    #[tokio::test]
    async fn directory_archive_is_idempotent_and_rejects_conflicting_bytes() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let source = staging.path().join("artifact.glb");
        let bytes = b"glTF durable artifact";
        tokio::fs::write(&source, bytes).await.unwrap();
        let artifact = FetchedArtifact {
            path: source.clone(),
            sha256: sha256_hex(bytes),
            byte_count: bytes.len() as u64,
            media_type: Some("model/gltf-binary".to_string()),
        };
        let archive = DirectoryArchive::new(root.path(), "daedalus-artifact://test").unwrap();
        let first = archive
            .put_verified("meshy/tenant/job/model.glb", &artifact)
            .await
            .unwrap();
        let second = archive
            .put_verified("meshy/tenant/job/model.glb", &artifact)
            .await
            .unwrap();
        assert_eq!(first, second);

        let different = staging.path().join("different.glb");
        tokio::fs::write(&different, b"glTF different")
            .await
            .unwrap();
        let conflict = FetchedArtifact {
            path: different,
            sha256: sha256_hex(b"glTF different"),
            byte_count: b"glTF different".len() as u64,
            media_type: Some("model/gltf-binary".to_string()),
        };
        let error = archive
            .put_verified("meshy/tenant/job/model.glb", &conflict)
            .await
            .expect_err("different bytes at the same key must fail");
        assert_eq!(error.code, "archive_object_conflict");
    }

    #[test]
    fn artifact_url_policy_rejects_ssrf_shapes() {
        let policy = ArtifactUrlPolicy::new(["meshy.example"]).unwrap();
        assert!(policy
            .validate("https://cdn.meshy.example/model.glb")
            .is_ok());
        assert!(policy
            .validate("http://cdn.meshy.example/model.glb")
            .is_err());
        assert!(policy.validate("https://127.0.0.1/model.glb").is_err());
        assert!(policy
            .validate("https://cdn.meshy.example@127.0.0.1/model.glb")
            .is_err());
        assert!(policy.validate("https://example.com/model.glb").is_err());
    }

    #[test]
    fn request_validation_requires_owned_source_hashes_and_safe_paths() {
        let mut invalid = request();
        invalid.images[0].sha256 = "ABC".to_string();
        assert!(invalid.validate().is_err());
        invalid = request();
        invalid.archive_prefix = "../escape".to_string();
        assert!(invalid.validate().is_err());
        invalid = request();
        invalid.images.push(SourceImage {
            asset_id: "left".to_string(),
            url: "https://assets.example/left.png".to_string(),
            sha256: "b".repeat(64),
        });
        invalid.ai_model = Some(AiModel::MeshyT2);
        assert!(invalid.validate().is_err());
    }
}
