//! Typed Meshy provider client for Daedalus image-to-3D workflows.
//!
//! Meshy is an upstream geometry generator. A successful provider task is
//! represented as a review-blocked candidate; it is never promoted directly to
//! Daedalus machine-ready release.

use std::{collections::BTreeMap, env, error::Error, fmt, time::Duration};

use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER},
    Method, StatusCode, Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

pub mod cli;

pub const DEFAULT_BASE_URL: &str = "https://api.meshy.ai";
pub const API_KEY_ENV: &str = "MESHY_API_KEY";
pub const API_BASE_URL_ENV: &str = "MESHY_API_BASE_URL";

const API_PREFIX: [&str; 2] = ["openapi", "v1"];
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_PAGE_SIZE: u16 = 50;

#[derive(Clone)]
pub struct MeshyClient {
    http: reqwest::Client,
    base_url: Url,
}

impl fmt::Debug for MeshyClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeshyClient")
            .field("base_url", &self.base_url)
            .field("authorization", &"Bearer <redacted>")
            .finish()
    }
}

impl MeshyClient {
    pub fn from_env() -> Result<Self, MeshyError> {
        let api_key = env::var(API_KEY_ENV)
            .map_err(|_| MeshyError::Configuration(format!("{API_KEY_ENV} is required")))?;
        let base_url = env::var(API_BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        Self::with_base_url(api_key, base_url)
    }

    pub fn new(api_key: impl Into<String>) -> Result<Self, MeshyError> {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl AsRef<str>,
    ) -> Result<Self, MeshyError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(MeshyError::Configuration(
                "Meshy API key must not be empty".to_string(),
            ));
        }

        let mut base_url = Url::parse(base_url.as_ref()).map_err(|error| {
            MeshyError::Configuration(format!("invalid Meshy API base URL: {error}"))
        })?;
        validate_base_url(&base_url)?;
        base_url.set_query(None);
        base_url.set_fragment(None);

        let mut headers = HeaderMap::new();
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", api_key.trim()))
            .map_err(|_| {
                MeshyError::Configuration(
                    "Meshy API key contains characters that are invalid in an HTTP header"
                        .to_string(),
                )
            })?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(DEFAULT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(MeshyError::Transport)?;

        Ok(Self { http, base_url })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub async fn create_image_task(
        &self,
        request: &ImageTo3dRequest,
    ) -> Result<CreateTaskResponse, MeshyError> {
        request.validate()?;
        self.send_json(
            Method::POST,
            self.endpoint(TaskKind::ImageTo3d, None)?,
            Some(request),
        )
        .await
    }

    pub async fn create_multi_image_task(
        &self,
        request: &MultiImageTo3dRequest,
    ) -> Result<CreateTaskResponse, MeshyError> {
        request.validate()?;
        self.send_json(
            Method::POST,
            self.endpoint(TaskKind::MultiImageTo3d, None)?,
            Some(request),
        )
        .await
    }

    pub async fn get_task(&self, kind: TaskKind, task_id: &str) -> Result<MeshyTask, MeshyError> {
        validate_task_id(task_id)?;
        self.send_json::<(), MeshyTask>(Method::GET, self.endpoint(kind, Some(task_id))?, None)
            .await
    }

    pub async fn list_tasks(
        &self,
        kind: TaskKind,
        query: ListTasksQuery,
    ) -> Result<Vec<MeshyTask>, MeshyError> {
        query.validate()?;
        let mut url = self.endpoint(kind, None)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("page_num", &query.page_num.to_string());
            pairs.append_pair("page_size", &query.page_size.to_string());
            pairs.append_pair("sort_by", query.sort_by.as_api_value());
        }
        self.send_json::<(), Vec<MeshyTask>>(Method::GET, url, None)
            .await
    }

    pub async fn delete_task(&self, kind: TaskKind, task_id: &str) -> Result<(), MeshyError> {
        validate_task_id(task_id)?;
        self.send_empty(Method::DELETE, self.endpoint(kind, Some(task_id))?)
            .await
    }

    pub async fn wait_for_task(
        &self,
        kind: TaskKind,
        task_id: &str,
        options: WaitOptions,
    ) -> Result<MeshyTask, MeshyError> {
        options.validate()?;
        let deadline = tokio::time::Instant::now() + options.timeout;
        loop {
            let task = self.get_task(kind, task_id).await?;
            if task.status.is_terminal() {
                return Ok(task);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(MeshyError::Timeout {
                    task_id: task_id.to_string(),
                    timeout: options.timeout,
                });
            }
            tokio::time::sleep(options.poll_interval).await;
        }
    }

    fn endpoint(&self, kind: TaskKind, task_id: Option<&str>) -> Result<Url, MeshyError> {
        let mut url = self.base_url.clone();
        let mut segments = url.path_segments_mut().map_err(|_| {
            MeshyError::Configuration("Meshy API base URL cannot be used as a base".to_string())
        })?;
        segments.pop_if_empty();
        segments.extend(API_PREFIX);
        segments.push(kind.as_path());
        if let Some(task_id) = task_id {
            segments.push(task_id);
        }
        drop(segments);
        Ok(url)
    }

    async fn send_json<B, R>(
        &self,
        method: Method,
        url: Url,
        body: Option<&B>,
    ) -> Result<R, MeshyError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(MeshyError::Transport)?;
        let status = response.status();
        let retry_after = retry_after_seconds(response.headers());
        let bytes = response.bytes().await.map_err(MeshyError::Transport)?;
        if !status.is_success() {
            return Err(api_error(status, retry_after, &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|error| MeshyError::Decode {
            status: status.as_u16(),
            message: error.to_string(),
        })
    }

    async fn send_empty(&self, method: Method, url: Url) -> Result<(), MeshyError> {
        let response = self
            .http
            .request(method, url)
            .send()
            .await
            .map_err(MeshyError::Transport)?;
        let status = response.status();
        let retry_after = retry_after_seconds(response.headers());
        let bytes = response.bytes().await.map_err(MeshyError::Transport)?;
        if status.is_success() {
            Ok(())
        } else {
            Err(api_error(status, retry_after, &bytes))
        }
    }
}

fn validate_base_url(url: &Url) -> Result<(), MeshyError> {
    if url.scheme() == "https" {
        return Ok(());
    }
    let local_http = url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        );
    if local_http {
        return Ok(());
    }
    Err(MeshyError::Configuration(
        "Meshy API base URL must use HTTPS; plain HTTP is accepted only for localhost tests"
            .to_string(),
    ))
}

fn validate_task_id(task_id: &str) -> Result<(), MeshyError> {
    if task_id.trim().is_empty() {
        return Err(MeshyError::Validation(
            "Meshy task id must not be empty".to_string(),
        ));
    }
    if task_id.len() > 256 {
        return Err(MeshyError::Validation(
            "Meshy task id must not exceed 256 bytes".to_string(),
        ));
    }
    Ok(())
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn api_error(status: StatusCode, retry_after_seconds: Option<u64>, bytes: &[u8]) -> MeshyError {
    let bounded = &bytes[..bytes.len().min(MAX_ERROR_BODY_BYTES)];
    let parsed = serde_json::from_slice::<Value>(bounded).ok();
    let code = parsed.as_ref().and_then(|value| {
        value
            .get("code")
            .or_else(|| value.get("error_code"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let message = parsed
        .as_ref()
        .and_then(extract_error_message)
        .or_else(|| String::from_utf8(bounded.to_vec()).ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("Meshy API error")
                .to_string()
        });
    MeshyError::Api {
        status: status.as_u16(),
        code,
        message,
        retry_after_seconds,
    }
}

fn extract_error_message(value: &Value) -> Option<String> {
    value
        .get("message")
        .or_else(|| value.get("detail"))
        .or_else(|| value.get("error"))
        .and_then(|value| match value {
            Value::String(message) => Some(message.clone()),
            Value::Object(map) => map
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
}

#[derive(Debug)]
pub enum MeshyError {
    Configuration(String),
    Validation(String),
    Transport(reqwest::Error),
    Api {
        status: u16,
        code: Option<String>,
        message: String,
        retry_after_seconds: Option<u64>,
    },
    Decode {
        status: u16,
        message: String,
    },
    Timeout {
        task_id: String,
        timeout: Duration,
    },
}

impl fmt::Display for MeshyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => write!(f, "Meshy configuration error: {message}"),
            Self::Validation(message) => write!(f, "Meshy request validation error: {message}"),
            Self::Transport(error) => write!(f, "Meshy transport error: {error}"),
            Self::Api {
                status,
                code,
                message,
                retry_after_seconds,
            } => {
                write!(f, "Meshy API returned HTTP {status}")?;
                if let Some(code) = code {
                    write!(f, " ({code})")?;
                }
                write!(f, ": {message}")?;
                if let Some(seconds) = retry_after_seconds {
                    write!(f, " [retry after {seconds}s]")?;
                }
                Ok(())
            }
            Self::Decode { status, message } => {
                write!(
                    f,
                    "Meshy response decode failed after HTTP {status}: {message}"
                )
            }
            Self::Timeout { task_id, timeout } => write!(
                f,
                "Meshy task {task_id} did not reach a terminal state within {} seconds",
                timeout.as_secs()
            ),
        }
    }
}

impl Error for MeshyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    ImageTo3d,
    MultiImageTo3d,
}

impl TaskKind {
    pub const fn as_path(self) -> &'static str {
        match self {
            Self::ImageTo3d => "image-to-3d",
            Self::MultiImageTo3d => "multi-image-to-3d",
        }
    }
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_path())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageTo3dRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_type: Option<ModelType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_model: Option<AiModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose_mode: Option<PoseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_enhancement: Option<bool>,
    #[serde(flatten)]
    pub options: GenerationOptions,
}

impl ImageTo3dRequest {
    pub fn validate(&self) -> Result<(), MeshyError> {
        validate_exactly_one_input(
            self.input_task_id.as_deref(),
            self.image_url.as_deref(),
            "image_url",
        )?;
        if let Some(image_url) = self.image_url.as_deref() {
            validate_image_reference(image_url, "image_url")?;
        }
        validate_model_selection(self.model_type, self.ai_model, &self.options)?;
        self.options.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultiImageTo3dRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_urls: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_model: Option<AiModel>,
    #[serde(flatten)]
    pub options: GenerationOptions,
}

impl MultiImageTo3dRequest {
    pub fn validate(&self) -> Result<(), MeshyError> {
        let has_task = self
            .input_task_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_images = self
            .image_urls
            .as_ref()
            .is_some_and(|values| !values.is_empty());
        if has_task == has_images {
            return Err(MeshyError::Validation(
                "provide exactly one of input_task_id or image_urls".to_string(),
            ));
        }
        if let Some(images) = &self.image_urls {
            if !(1..=4).contains(&images.len()) {
                return Err(MeshyError::Validation(
                    "image_urls must contain between 1 and 4 images".to_string(),
                ));
            }
            for (index, image) in images.iter().enumerate() {
                validate_image_reference(image, &format!("image_urls[{index}]"))?;
            }
        }
        if matches!(self.ai_model, Some(AiModel::MeshyT1 | AiModel::MeshyT2)) {
            return Err(MeshyError::Validation(
                "multi-image generation supports meshy-5, meshy-6, or latest; smart-topology models are single-image only"
                    .to_string(),
            ));
        }
        self.options.validate()?;
        Ok(())
    }
}

fn validate_exactly_one_input(
    input_task_id: Option<&str>,
    direct_input: Option<&str>,
    direct_input_name: &str,
) -> Result<(), MeshyError> {
    let has_task = input_task_id.is_some_and(|value| !value.trim().is_empty());
    let has_direct = direct_input.is_some_and(|value| !value.trim().is_empty());
    if has_task == has_direct {
        return Err(MeshyError::Validation(format!(
            "provide exactly one of input_task_id or {direct_input_name}"
        )));
    }
    Ok(())
}

fn validate_image_reference(value: &str, field: &str) -> Result<(), MeshyError> {
    let value = value.trim();
    if value.starts_with("data:image/jpeg;base64,")
        || value.starts_with("data:image/jpg;base64,")
        || value.starts_with("data:image/png;base64,")
    {
        return Ok(());
    }
    let url = Url::parse(value).map_err(|_| {
        MeshyError::Validation(format!(
            "{field} must be an HTTP(S) URL or a JPEG/PNG base64 data URI"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(MeshyError::Validation(format!(
            "{field} must use HTTP(S) or be a JPEG/PNG base64 data URI"
        )));
    }
    Ok(())
}

fn validate_model_selection(
    model_type: Option<ModelType>,
    ai_model: Option<AiModel>,
    options: &GenerationOptions,
) -> Result<(), MeshyError> {
    match model_type.unwrap_or(ModelType::Standard) {
        ModelType::Standard => {
            if matches!(ai_model, Some(AiModel::MeshyT1 | AiModel::MeshyT2)) {
                return Err(MeshyError::Validation(
                    "standard image generation supports meshy-5, meshy-6, or latest".to_string(),
                ));
            }
        }
        ModelType::SmartTopology => {
            if matches!(
                ai_model,
                Some(AiModel::Latest | AiModel::Meshy5 | AiModel::Meshy6)
            ) {
                return Err(MeshyError::Validation(
                    "smart-topology requires meshy-t1 or meshy-t2".to_string(),
                ));
            }
            if options.topology.is_some()
                || options.should_remesh.is_some()
                || options.save_pre_remeshed_model.is_some()
                || options.decimation_mode.is_some()
            {
                return Err(MeshyError::Validation(
                    "smart-topology ignores topology, remesh, pre-remesh, and decimation settings; remove them to keep request intent unambiguous"
                        .to_string(),
                ));
            }
            if ai_model == Some(AiModel::MeshyT1) && options.target_polycount.is_some() {
                return Err(MeshyError::Validation(
                    "meshy-t1 does not support target_polycount; use meshy-t2".to_string(),
                ));
            }
            if ai_model.unwrap_or(AiModel::MeshyT2) == AiModel::MeshyT2 {
                if let Some(polycount) = options.target_polycount {
                    if !(100..=15_000).contains(&polycount) {
                        return Err(MeshyError::Validation(
                            "smart-topology meshy-t2 target_polycount must be between 100 and 15000"
                                .to_string(),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenerationOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub should_texture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_pbr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_resolution: Option<TextureResolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub should_remesh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology: Option<Topology>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_polycount: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimation_mode: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_pre_remeshed_model: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_lighting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_formats: Option<Vec<TargetFormat>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_size: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_thumbnail: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_view_thumbnails: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_at: Option<OriginAt>,
}

impl GenerationOptions {
    fn validate(&self) -> Result<(), MeshyError> {
        if self.enable_pbr == Some(true) && self.should_texture == Some(false) {
            return Err(MeshyError::Validation(
                "enable_pbr requires should_texture=true".to_string(),
            ));
        }
        if self.should_texture == Some(false)
            && (self.texture_resolution.is_some()
                || self.texture_prompt.is_some()
                || self.texture_image_url.is_some()
                || self.remove_lighting.is_some())
        {
            return Err(MeshyError::Validation(
                "texture settings cannot be supplied when should_texture=false".to_string(),
            ));
        }
        if self.texture_prompt.is_some() && self.texture_image_url.is_some() {
            return Err(MeshyError::Validation(
                "provide at most one of texture_prompt or texture_image_url".to_string(),
            ));
        }
        if let Some(prompt) = &self.texture_prompt {
            if prompt.chars().count() > 600 {
                return Err(MeshyError::Validation(
                    "texture_prompt must not exceed 600 characters".to_string(),
                ));
            }
        }
        if let Some(image) = self.texture_image_url.as_deref() {
            validate_image_reference(image, "texture_image_url")?;
        }
        if let Some(polycount) = self.target_polycount {
            if !(100..=300_000).contains(&polycount) {
                return Err(MeshyError::Validation(
                    "target_polycount must be between 100 and 300000".to_string(),
                ));
            }
        }
        if let Some(mode) = self.decimation_mode {
            if !(1..=4).contains(&mode) {
                return Err(MeshyError::Validation(
                    "decimation_mode must be between 1 and 4".to_string(),
                ));
            }
            if self.target_polycount.is_some() {
                return Err(MeshyError::Validation(
                    "decimation_mode and target_polycount are mutually exclusive".to_string(),
                ));
            }
        }
        if self.target_formats.as_ref().is_some_and(Vec::is_empty) {
            return Err(MeshyError::Validation(
                "target_formats must not be an empty array".to_string(),
            ));
        }
        if self.origin_at.is_some() && self.auto_size != Some(true) {
            return Err(MeshyError::Validation(
                "origin_at applies only when auto_size=true".to_string(),
            ));
        }
        if self.multi_view_thumbnails == Some(true) && self.auto_size != Some(true) {
            return Err(MeshyError::Validation(
                "multi_view_thumbnails applies only when auto_size=true".to_string(),
            ));
        }
        if self.texture_resolution == Some(TextureResolution::EightK)
            && self.topology == Some(Topology::Quad)
        {
            return Err(MeshyError::Validation(
                "8k textures support triangle topology only".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ModelType {
    Standard,
    SmartTopology,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AiModel {
    #[serde(rename = "latest")]
    Latest,
    #[serde(rename = "meshy-5")]
    Meshy5,
    #[serde(rename = "meshy-6")]
    Meshy6,
    #[serde(rename = "meshy-t1")]
    MeshyT1,
    #[serde(rename = "meshy-t2")]
    MeshyT2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PoseMode {
    APose,
    TPose,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Topology {
    Triangle,
    Quad,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextureResolution {
    #[serde(rename = "2k")]
    TwoK,
    #[serde(rename = "4k")]
    FourK,
    #[serde(rename = "8k")]
    EightK,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OriginAt {
    Bottom,
    Center,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetFormat {
    #[serde(rename = "glb")]
    Glb,
    #[serde(rename = "obj")]
    Obj,
    #[serde(rename = "fbx")]
    Fbx,
    #[serde(rename = "stl")]
    Stl,
    #[serde(rename = "usdz")]
    Usdz,
    #[serde(rename = "3mf")]
    ThreeMf,
}

impl TargetFormat {
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::Glb => "glb",
            Self::Obj => "obj",
            Self::Fbx => "fbx",
            Self::Stl => "stl",
            Self::Usdz => "usdz",
            Self::ThreeMf => "3mf",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTaskResponse {
    pub result: String,
}

impl CreateTaskResponse {
    pub fn daedalus_receipt(&self, kind: TaskKind) -> DaedalusTaskReceipt {
        DaedalusTaskReceipt::new(kind, self.result.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshyTask {
    pub id: String,
    #[serde(rename = "type", default)]
    pub task_type: String,
    #[serde(default)]
    pub model_urls: ModelUrls,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub alpha_thumbnail_url: Option<String>,
    #[serde(default)]
    pub thumbnail_urls: ThumbnailUrls,
    #[serde(default)]
    pub texture_prompt: Option<String>,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
    pub status: TaskStatus,
    #[serde(default)]
    pub texture_urls: Vec<TextureUrls>,
    #[serde(default)]
    pub preceding_tasks: Option<u32>,
    #[serde(default)]
    pub task_error: Option<TaskError>,
    #[serde(default)]
    pub consumed_credits: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MeshyTask {
    pub fn daedalus_candidate(&self, kind: TaskKind) -> DaedalusGeometryCandidate {
        DaedalusGeometryCandidate::from_task(kind, self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelUrls {
    #[serde(default)]
    pub glb: Option<String>,
    #[serde(default)]
    pub obj: Option<String>,
    #[serde(default)]
    pub fbx: Option<String>,
    #[serde(default)]
    pub stl: Option<String>,
    #[serde(default)]
    pub usdz: Option<String>,
    #[serde(rename = "3mf", default)]
    pub three_mf: Option<String>,
    #[serde(default)]
    pub pre_remeshed_glb: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ModelUrls {
    pub fn requested_artifacts(&self) -> BTreeMap<String, String> {
        let mut artifacts = BTreeMap::new();
        for (format, value) in [
            ("glb", self.glb.as_ref()),
            ("obj", self.obj.as_ref()),
            ("fbx", self.fbx.as_ref()),
            ("stl", self.stl.as_ref()),
            ("usdz", self.usdz.as_ref()),
            ("3mf", self.three_mf.as_ref()),
            ("pre_remeshed_glb", self.pre_remeshed_glb.as_ref()),
        ] {
            if let Some(value) = value {
                artifacts.insert(format.to_string(), value.clone());
            }
        }
        artifacts
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThumbnailUrls {
    #[serde(default)]
    pub front: Option<String>,
    #[serde(default)]
    pub right: Option<String>,
    #[serde(default)]
    pub back: Option<String>,
    #[serde(default)]
    pub left: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextureUrls {
    #[serde(default)]
    pub base_color: Option<String>,
    #[serde(default)]
    pub metallic: Option<String>,
    #[serde(default)]
    pub normal: Option<String>,
    #[serde(default)]
    pub roughness: Option<String>,
    #[serde(default)]
    pub emission: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskError {
    #[serde(default)]
    pub message: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    Canceled,
    Cancelled,
    #[serde(other)]
    Unknown,
}

impl TaskStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Canceled | Self::Cancelled
        )
    }
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSort {
    CreatedAscending,
    CreatedDescending,
}

impl TaskSort {
    const fn as_api_value(self) -> &'static str {
        match self {
            Self::CreatedAscending => "+created_at",
            Self::CreatedDescending => "-created_at",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ListTasksQuery {
    pub page_num: u32,
    pub page_size: u16,
    pub sort_by: TaskSort,
}

impl Default for ListTasksQuery {
    fn default() -> Self {
        Self {
            page_num: 1,
            page_size: 10,
            sort_by: TaskSort::CreatedDescending,
        }
    }
}

impl ListTasksQuery {
    fn validate(self) -> Result<(), MeshyError> {
        if self.page_num == 0 {
            return Err(MeshyError::Validation("page_num starts at 1".to_string()));
        }
        if self.page_size == 0 || self.page_size > MAX_PAGE_SIZE {
            return Err(MeshyError::Validation(format!(
                "page_size must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WaitOptions {
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl Default for WaitOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            timeout: Duration::from_secs(30 * 60),
        }
    }
}

impl WaitOptions {
    fn validate(self) -> Result<(), MeshyError> {
        if self.poll_interval.is_zero() {
            return Err(MeshyError::Validation(
                "poll interval must be greater than zero".to_string(),
            ));
        }
        if self.timeout < self.poll_interval {
            return Err(MeshyError::Validation(
                "wait timeout must be at least one poll interval".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaedalusTaskReceipt {
    pub schema_version: &'static str,
    pub provider: &'static str,
    pub provider_task_kind: String,
    pub provider_task_id: String,
    pub release_state: &'static str,
    pub machine_ready: bool,
    pub required_reviews: Vec<&'static str>,
}

impl DaedalusTaskReceipt {
    fn new(kind: TaskKind, provider_task_id: String) -> Self {
        Self {
            schema_version: "dd.fabrication.external-generation-task.v1",
            provider: "meshy",
            provider_task_kind: kind.to_string(),
            provider_task_id,
            release_state: "draft",
            machine_ready: false,
            required_reviews: release_reviews(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaedalusGeometryCandidate {
    pub schema_version: &'static str,
    pub provider: &'static str,
    pub source_system: &'static str,
    pub provider_task_kind: String,
    pub provider_task_id: String,
    pub provider_status: TaskStatus,
    pub provider_progress: u8,
    pub release_state: &'static str,
    pub machine_ready: bool,
    pub model_urls: BTreeMap<String, String>,
    pub provider_expires_at: Option<i64>,
    pub consumed_credits: Option<u64>,
    pub blockers: Vec<&'static str>,
    pub required_evidence: Vec<&'static str>,
    pub provider_error: Option<String>,
}

impl DaedalusGeometryCandidate {
    fn from_task(kind: TaskKind, task: &MeshyTask) -> Self {
        let provider_error = task
            .task_error
            .as_ref()
            .map(|error| error.message.trim().to_string())
            .filter(|message| !message.is_empty());
        Self {
            schema_version: "dd.fabrication.external-geometry-candidate.v1",
            provider: "meshy",
            source_system: "meshy",
            provider_task_kind: kind.to_string(),
            provider_task_id: task.id.clone(),
            provider_status: task.status,
            provider_progress: task.progress,
            release_state: "draft",
            machine_ready: false,
            model_urls: task.model_urls.requested_artifacts(),
            provider_expires_at: task.expires_at,
            consumed_credits: task.consumed_credits,
            blockers: vec![
                "external AI geometry is candidate input, not certified fabrication geometry",
                "provider model URLs must be copied into Daedalus-controlled storage and hashed",
                "scale, units, coordinate frame, topology, normals, watertightness, and wall thickness require independent review",
                "manufacturing route, simulation, inspection, and release authorization remain unresolved",
            ],
            required_evidence: release_reviews(),
            provider_error,
        }
    }
}

fn release_reviews() -> Vec<&'static str> {
    vec![
        "source image checksums and provider request provenance",
        "durable GLB/STL/3MF artifact checksums",
        "scale, units, and coordinate-frame evidence",
        "mesh repair, manifold, normals, and wall-thickness review",
        "orientation, support, material, slicer, and machine-profile review",
        "simulation or dry-run evidence and operator or automation signoff",
    ]
}

pub fn capability_document() -> Value {
    json!({
        "schemaVersion": "dd.fabrication.external-generation-provider.v1",
        "provider": "meshy",
        "taskKinds": [TaskKind::ImageTo3d.as_path(), TaskKind::MultiImageTo3d.as_path()],
        "inputImageFormats": ["jpg", "jpeg", "png"],
        "multiImageCount": {"minimum": 1, "maximum": 4},
        "outputFormats": ["glb", "obj", "fbx", "stl", "usdz", "3mf"],
        "authentication": {"type": "bearer", "environmentVariable": API_KEY_ENV},
        "releaseBoundary": {
            "releaseState": "draft",
            "machineReady": false,
            "requiredReviews": release_reviews(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
    };

    async fn mock_server(status: u16, response_body: &'static str) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let observed = Arc::new(Mutex::new(String::new()));
        let observed_task = Arc::clone(&observed);
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = stream.read(&mut chunk).await.expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if complete_http_request(&request) {
                    break;
                }
            }
            *observed_task.lock().await = String::from_utf8_lossy(&request).to_string();
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{address}"), observed)
    }

    fn complete_http_request(bytes: &[u8]) -> bool {
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        bytes.len() >= header_end + 4 + content_length
    }

    fn image_request() -> ImageTo3dRequest {
        ImageTo3dRequest {
            image_url: Some("https://example.com/object.png".to_string()),
            ai_model: Some(AiModel::Meshy6),
            options: GenerationOptions {
                target_formats: Some(vec![
                    TargetFormat::Glb,
                    TargetFormat::Stl,
                    TargetFormat::ThreeMf,
                ]),
                moderation: Some(true),
                ..GenerationOptions::default()
            },
            ..ImageTo3dRequest::default()
        }
    }

    #[tokio::test]
    async fn create_image_task_uses_bearer_auth_and_expected_endpoint() {
        let (base_url, observed) = mock_server(200, r#"{"result":"task-123"}"#).await;
        let client = MeshyClient::with_base_url("secret-test-key", &base_url).expect("client");
        let created = client
            .create_image_task(&image_request())
            .await
            .expect("create task");
        assert_eq!(created.result, "task-123");

        let request = observed.lock().await.clone();
        assert!(request.starts_with("POST /openapi/v1/image-to-3d HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer secret-test-key"));
        assert!(request.contains("\"target_formats\":[\"glb\",\"stl\",\"3mf\"]"));
    }

    #[tokio::test]
    async fn api_errors_do_not_echo_the_api_key() {
        let (base_url, _) = mock_server(
            401,
            r#"{"code":"InvalidCredentials","message":"invalid credentials"}"#,
        )
        .await;
        let api_key = "secret-that-must-not-leak";
        let client = MeshyClient::with_base_url(api_key, base_url).expect("client");
        let error = client
            .create_image_task(&image_request())
            .await
            .expect_err("request must fail");
        let rendered = format!("{error:?} {error} {client:?}");
        assert!(!rendered.contains(api_key));
        assert!(rendered.contains("InvalidCredentials"));
    }

    #[test]
    fn single_image_request_rejects_ambiguous_inputs() {
        let mut request = image_request();
        request.input_task_id = Some("upstream-task".to_string());
        assert!(request.validate().is_err());
    }

    #[test]
    fn multi_image_request_enforces_one_to_four_views() {
        let request = MultiImageTo3dRequest {
            image_urls: Some(vec![
                "https://example.com/1.png".to_string(),
                "https://example.com/2.png".to_string(),
                "https://example.com/3.png".to_string(),
                "https://example.com/4.png".to_string(),
                "https://example.com/5.png".to_string(),
            ]),
            ..MultiImageTo3dRequest::default()
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn smart_topology_polycount_is_bounded_to_provider_contract() {
        let request = ImageTo3dRequest {
            image_url: Some("https://example.com/object.png".to_string()),
            model_type: Some(ModelType::SmartTopology),
            ai_model: Some(AiModel::MeshyT2),
            options: GenerationOptions {
                target_polycount: Some(15_001),
                ..GenerationOptions::default()
            },
            ..ImageTo3dRequest::default()
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn succeeded_provider_task_is_still_review_blocked() {
        let task: MeshyTask = serde_json::from_value(json!({
            "id": "task-123",
            "type": "image-to-3d",
            "status": "SUCCEEDED",
            "progress": 100,
            "model_urls": {
                "glb": "https://assets.meshy.ai/model.glb",
                "stl": "https://assets.meshy.ai/model.stl",
                "3mf": "https://assets.meshy.ai/model.3mf"
            }
        }))
        .expect("task response");
        let candidate = task.daedalus_candidate(TaskKind::ImageTo3d);
        assert!(!candidate.machine_ready);
        assert_eq!(candidate.release_state, "draft");
        assert_eq!(candidate.model_urls.len(), 3);
        assert!(!candidate.blockers.is_empty());
    }

    #[test]
    fn non_local_plain_http_is_refused() {
        let error = MeshyClient::with_base_url("secret", "http://mesh-provider.invalid")
            .expect_err("plain HTTP must be rejected");
        assert!(error.to_string().contains("HTTPS"));
    }
}
