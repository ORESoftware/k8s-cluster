//! Container-pool image config + build/test orchestration.
//!
//! Backs the `/container-pool/config` page rendered by `dd-remote-web-home`:
//!   - GET    /api/container-pool/images                       — image catalog + latest status
//!   - GET    /api/container-pool/images/:slug                 — detail for one image
//!   - GET    /api/container-pool/images/:slug/dockerfile      — Dockerfile text (disk default or saved revision)
//!   - PUT    /api/container-pool/images/:slug/dockerfile      — save as a new revision
//!   - GET    /api/container-pool/images/:slug/revisions       — full revision history
//!   - GET    /api/container-pool/images/:slug/builds          — build/test history
//!   - POST   /api/container-pool/images/:slug/build-test      — kick off a build + smoke test
//!   - GET    /api/container-pool/builds/:build_id             — single build run detail
//!
//! Builds reuse the same hostPath/nerdctl mounts that today power
//! `LAMBDA_IMAGE_BUILD_*` (see `dd-remote-rest-api.deployment.yaml`), so no
//! new privileged surface is added. Each build is scoped to its own
//! `/var/lib/dd-container-pool-images/<build_id>` temp dir; the Dockerfile
//! revision text is materialised there and passed to nerdctl via `-f`, while
//! the unmodified repo build context (the real, working-copy directory) is
//! passed as the build context root. That keeps build outputs identical to
//! a `nerdctl build` invoked by hand from the repo.
//!
//! Auth: protected at the gateway via `dd_gateway_auth_ok` (mirrors
//! `/api/lambdas/`) and again in this service with `X-Server-Auth`, so
//! direct in-cluster callers cannot start privileged image builds unless
//! they have the service secret.

use std::{
    env, fs,
    path::{Component, Path as FsPath, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path, Query},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{process::Command as TokioCommand, time::timeout};

use super::{connect_postgres, env_bool, row_opt_string, row_string};

// ─────────────────────────────────────────────────────────────────────────────
// Image catalog (the canonical list — kept in lock-step with the seed at
// remote/databases/pg/seeds/container-pool-app-config.sql). Adding a new
// runtime image is a one-line addition here + an entry in the seed.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct CatalogEntry {
    slug: &'static str,
    display_name: &'static str,
    image_ref: &'static str,
    dockerfile_path: &'static str,
    build_context: &'static str,
    default_test_command: &'static str,
    notes: &'static str,
}

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        slug: "nodejs",
        display_name: "Node.js warm runtime",
        image_ref: "docker.io/library/dd-container-pool-nodejs-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/nodejs.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "node --version && echo ok",
        notes: "Container-pool default Node.js runtime image (lambda + handler).",
    },
    CatalogEntry {
        slug: "nodejs-chat-claude",
        display_name: "Node.js chat/Claude worker (dd-dev-server)",
        image_ref: "docker.io/library/dd-dev-server:dev",
        dockerfile_path: "remote/deployments/dev-server/Dockerfile",
        build_context: "remote/deployments/dev-server",
        // dev-server's default entrypoint needs DD_REPO_URL etc, so the
        // smoke test overrides entrypoint and just proves the image can spawn
        // node and Tini-style init.
        default_test_command: "node --version && which node && echo ok",
        notes: "Repo-scoped warm worker container for agent threads.",
    },
    CatalogEntry {
        slug: "rust",
        display_name: "Rust warm runtime",
        image_ref: "docker.io/library/dd-container-pool-rust-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/rust.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "rustc --version || cargo --version || echo ok",
        notes: "Container-pool Rust runtime image.",
    },
    CatalogEntry {
        slug: "golang",
        display_name: "Go warm runtime",
        image_ref: "docker.io/library/dd-container-pool-golang-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/golang.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "go version && echo ok",
        notes: "Container-pool Go runtime image.",
    },
    CatalogEntry {
        slug: "python3",
        display_name: "Python 3 warm runtime",
        image_ref: "docker.io/library/dd-container-pool-python3-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/python3.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "python3 --version && echo ok",
        notes: "Container-pool Python 3 runtime image.",
    },
    CatalogEntry {
        slug: "dart",
        display_name: "Dart warm runtime",
        image_ref: "docker.io/library/dd-container-pool-dart-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/dart.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "dart --version 2>&1 && echo ok",
        notes: "Container-pool Dart runtime image.",
    },
    CatalogEntry {
        slug: "gleamlang",
        display_name: "Gleam warm runtime",
        image_ref: "docker.io/library/dd-container-pool-gleamlang-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/gleamlang.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "gleam --version && echo ok",
        notes: "Container-pool Gleam/BEAM runtime image.",
    },
    CatalogEntry {
        slug: "erlang",
        display_name: "Erlang warm runtime",
        image_ref: "docker.io/library/dd-container-pool-erlang-runtime:dev",
        dockerfile_path: "remote/deployments/container-pool-rs/runtime-images/erlang.Dockerfile",
        build_context: "remote/deployments/container-pool-rs",
        default_test_command: "erl -version 2>&1 && echo ok",
        notes: "Container-pool Erlang/OTP runtime image.",
    },
];

fn catalog_entry(slug: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.slug == slug)
}

// ─────────────────────────────────────────────────────────────────────────────
// Filesystem + nerdctl wiring (reuses the same env vars as lambda builds so
// the rest-api deployment doesn't need new volumes).
// ─────────────────────────────────────────────────────────────────────────────

fn image_repo_root() -> PathBuf {
    PathBuf::from(
        env::var("CONTAINER_POOL_IMAGE_REPO_ROOT")
            .or_else(|_| env::var("LAMBDA_IMAGE_REPO_ROOT"))
            .unwrap_or_else(|_| "/opt/dd-next-1".to_string()),
    )
}

fn build_root() -> PathBuf {
    PathBuf::from(
        env::var("CONTAINER_POOL_IMAGE_BUILD_ROOT")
            .unwrap_or_else(|_| "/var/lib/dd-container-pool-images".to_string()),
    )
}

fn nerdctl_binary() -> String {
    env::var("CONTAINER_POOL_IMAGE_BUILD_NERDCTL")
        .or_else(|_| env::var("LAMBDA_IMAGE_BUILD_NERDCTL"))
        .unwrap_or_else(|_| "/usr/local/bin/nerdctl".to_string())
}

fn build_namespace() -> String {
    env::var("CONTAINER_POOL_IMAGE_BUILD_NAMESPACE").unwrap_or_else(|_| "dd-pool".to_string())
}

fn build_timeout() -> Duration {
    let seconds = env::var("CONTAINER_POOL_IMAGE_BUILD_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1200);
    Duration::from_secs(seconds.max(60))
}

fn test_timeout() -> Duration {
    let seconds = env::var("CONTAINER_POOL_IMAGE_TEST_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(120);
    Duration::from_secs(seconds.max(10))
}

fn builds_enabled() -> bool {
    env_bool("CONTAINER_POOL_IMAGE_BUILDS_ENABLED", false)
        || env_bool("LAMBDA_IMAGE_BUILD_ENABLED", false)
}

fn api_auth_required() -> bool {
    env_bool("CONTAINER_POOL_IMAGE_API_AUTH_REQUIRED", true)
}

fn custom_test_commands_enabled() -> bool {
    env_bool("CONTAINER_POOL_IMAGE_CUSTOM_TEST_COMMANDS_ENABLED", false)
}

fn image_build_delegate_url() -> Option<String> {
    env::var("IMAGE_BUILD_DELEGATE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
}

fn image_build_delegate_timeout() -> Duration {
    let seconds = env::var("IMAGE_BUILD_DELEGATE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);
    Duration::from_secs(seconds.clamp(5, 1800))
}

fn operator_api_secret() -> Option<String> {
    env::var("CONTAINER_POOL_IMAGE_API_SECRET")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("REMOTE_DEV_SERVER_SECRET")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            env::var("SERVER_AUTH_SECRET")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn request_has_operator_auth(headers: &HeaderMap) -> bool {
    if !api_auth_required() {
        return true;
    }
    let Some(expected) = operator_api_secret() else {
        return false;
    };
    headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "ok": false,
            "error": "unauthorized",
        })),
    )
        .into_response()
}

async fn require_operator_auth(req: Request<Body>, next: Next) -> Response {
    if request_has_operator_auth(req.headers()) {
        next.run(req).await
    } else {
        unauthorized_response()
    }
}

fn validate_path_under_root(path: &FsPath) -> Result<(), String> {
    if path.is_absolute() {
        return Err("path must be relative to the repo root".to_string());
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("path must not contain `..` or absolute components".to_string());
    }
    Ok(())
}

fn resolve_repo_path(rel: &str) -> Result<PathBuf, String> {
    let rel_path = FsPath::new(rel);
    validate_path_under_root(rel_path)?;
    Ok(image_repo_root().join(rel_path))
}

fn read_disk_dockerfile(entry: &CatalogEntry) -> Result<String, String> {
    let path = resolve_repo_path(entry.dockerfile_path)?;
    fs::read_to_string(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn truncate_log(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let marker = format!(
        "\n\n... [truncated {} bytes] ...\n\n",
        value.len().saturating_sub(max)
    );
    let budget = max.saturating_sub(marker.len());
    if budget == 0 {
        return marker.trim().to_string();
    }
    let head_budget = budget / 2;
    let tail_budget = budget - head_budget;
    let head = floor_char_boundary(value, head_budget);
    let tail = ceil_char_boundary(value, value.len().saturating_sub(tail_budget));
    format!("{}{}{}", &value[..head], marker, &value[tail..])
}

fn floor_char_boundary(value: &str, preferred: usize) -> usize {
    let mut idx = preferred.min(value.len());
    while idx > 0 && !value.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(value: &str, preferred: usize) -> usize {
    let mut idx = preferred.min(value.len());
    while idx < value.len() && !value.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

// ─────────────────────────────────────────────────────────────────────────────
// Public payload shapes (serialised back to the page JS).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ImageSummary {
    slug: String,
    display_name: String,
    image_ref: String,
    dockerfile_path: String,
    build_context: String,
    default_test_command: String,
    notes: String,
    disk_default_sha256: Option<String>,
    latest_revision: Option<RevisionRow>,
    latest_build: Option<BuildRunRow>,
    revision_count: i64,
    build_count: i64,
}

#[derive(Serialize, Clone)]
struct RevisionRow {
    id: String,
    image_slug: String,
    image_ref: String,
    dockerfile_path: String,
    build_context: String,
    dockerfile_sha256: String,
    source: String,
    status: String,
    notes: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    dockerfile_text: Option<String>,
}

#[derive(Serialize, Clone)]
struct BuildRunRow {
    id: String,
    image_slug: String,
    revision_id: String,
    image_ref: String,
    candidate_tag: String,
    build_status: String,
    test_status: String,
    overall_status: String,
    test_command: String,
    build_started_at: Option<String>,
    build_finished_at: Option<String>,
    test_started_at: Option<String>,
    test_finished_at: Option<String>,
    error_message: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    build_log_excerpt: Option<String>,
    test_log_excerpt: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Postgres readers / writers (inline SQL — schema.sql is the contract).
// ─────────────────────────────────────────────────────────────────────────────

fn row_to_revision(row: &tokio_postgres::Row, include_text: bool) -> RevisionRow {
    RevisionRow {
        id: row_string(row, "id"),
        image_slug: row_string(row, "image_slug"),
        image_ref: row_string(row, "image_ref"),
        dockerfile_path: row_string(row, "dockerfile_path"),
        build_context: row_string(row, "build_context"),
        dockerfile_sha256: row_string(row, "dockerfile_sha256"),
        source: row_string(row, "source"),
        status: row_string(row, "status"),
        notes: row_string(row, "notes"),
        created_at: row_opt_string(row, "created_at"),
        updated_at: row_opt_string(row, "updated_at"),
        dockerfile_text: if include_text {
            Some(row_string(row, "dockerfile_text"))
        } else {
            None
        },
    }
}

fn row_to_build(row: &tokio_postgres::Row, include_logs: bool) -> BuildRunRow {
    BuildRunRow {
        id: row_string(row, "id"),
        image_slug: row_string(row, "image_slug"),
        revision_id: row_string(row, "revision_id"),
        image_ref: row_string(row, "image_ref"),
        candidate_tag: row_string(row, "candidate_tag"),
        build_status: row_string(row, "build_status"),
        test_status: row_string(row, "test_status"),
        overall_status: row_string(row, "overall_status"),
        test_command: row_string(row, "test_command"),
        build_started_at: row_opt_string(row, "build_started_at"),
        build_finished_at: row_opt_string(row, "build_finished_at"),
        test_started_at: row_opt_string(row, "test_started_at"),
        test_finished_at: row_opt_string(row, "test_finished_at"),
        error_message: row_opt_string(row, "error_message"),
        created_at: row_opt_string(row, "created_at"),
        updated_at: row_opt_string(row, "updated_at"),
        build_log_excerpt: if include_logs {
            Some(row_string(row, "build_log_excerpt"))
        } else {
            None
        },
        test_log_excerpt: if include_logs {
            Some(row_string(row, "test_log_excerpt"))
        } else {
            None
        },
    }
}

const REVISION_COLS: &str =
    "id::text, image_slug, image_ref, dockerfile_path, build_context, dockerfile_sha256, \
     source, status, notes, created_at::text, updated_at::text";

const REVISION_COLS_FULL: &str =
    "id::text, image_slug, image_ref, dockerfile_path, build_context, dockerfile_sha256, \
     source, status, notes, dockerfile_text, created_at::text, updated_at::text";

const BUILD_COLS: &str =
    "id::text, image_slug, revision_id::text, image_ref, candidate_tag, build_status, \
     test_status, overall_status, test_command, build_started_at::text, \
     build_finished_at::text, test_started_at::text, test_finished_at::text, \
     error_message, created_at::text, updated_at::text";

const BUILD_COLS_FULL: &str =
    "id::text, image_slug, revision_id::text, image_ref, candidate_tag, build_status, \
     test_status, overall_status, test_command, build_started_at::text, \
     build_finished_at::text, test_started_at::text, test_finished_at::text, \
     error_message, build_log_excerpt, test_log_excerpt, created_at::text, updated_at::text";

async fn latest_revision_for(
    client: &tokio_postgres::Client,
    slug: &str,
) -> Result<Option<RevisionRow>, String> {
    let sql = format!(
        "select {cols} from container_pool_image_revisions \
         where image_slug = $1 and is_soft_deleted = false \
         order by created_at desc limit 1",
        cols = REVISION_COLS,
    );
    let row = client
        .query_opt(sql.as_str(), &[&slug])
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.as_ref().map(|r| row_to_revision(r, false)))
}

async fn revision_count_for(client: &tokio_postgres::Client, slug: &str) -> Result<i64, String> {
    let row = client
        .query_one(
            "select count(*)::bigint as n from container_pool_image_revisions \
             where image_slug = $1 and is_soft_deleted = false",
            &[&slug],
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.try_get::<_, i64>("n").unwrap_or(0))
}

async fn latest_build_for(
    client: &tokio_postgres::Client,
    slug: &str,
) -> Result<Option<BuildRunRow>, String> {
    let sql = format!(
        "select {cols} from container_pool_build_runs \
         where image_slug = $1 and is_soft_deleted = false \
         order by created_at desc limit 1",
        cols = BUILD_COLS,
    );
    let row = client
        .query_opt(sql.as_str(), &[&slug])
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.as_ref().map(|r| row_to_build(r, false)))
}

async fn build_count_for(client: &tokio_postgres::Client, slug: &str) -> Result<i64, String> {
    let row = client
        .query_one(
            "select count(*)::bigint as n from container_pool_build_runs \
             where image_slug = $1 and is_soft_deleted = false",
            &[&slug],
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.try_get::<_, i64>("n").unwrap_or(0))
}

async fn fetch_revision_by_id(
    client: &tokio_postgres::Client,
    revision_id: &str,
) -> Result<Option<RevisionRow>, String> {
    let sql = format!(
        "select {cols} from container_pool_image_revisions \
         where id = $1::text::uuid and is_soft_deleted = false",
        cols = REVISION_COLS_FULL,
    );
    let row = client
        .query_opt(sql.as_str(), &[&revision_id])
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.as_ref().map(|r| row_to_revision(r, true)))
}

async fn fetch_revision_by_sha(
    client: &tokio_postgres::Client,
    slug: &str,
    sha: &str,
) -> Result<Option<RevisionRow>, String> {
    let sql = format!(
        "select {cols} from container_pool_image_revisions \
         where image_slug = $1 and dockerfile_sha256 = $2 and is_soft_deleted = false \
         order by created_at desc limit 1",
        cols = REVISION_COLS_FULL,
    );
    let row = client
        .query_opt(sql.as_str(), &[&slug, &sha])
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.as_ref().map(|r| row_to_revision(r, true)))
}

async fn fetch_build_by_id(
    client: &tokio_postgres::Client,
    build_id: &str,
) -> Result<Option<BuildRunRow>, String> {
    let sql = format!(
        "select {cols} from container_pool_build_runs \
         where id = $1::text::uuid and is_soft_deleted = false",
        cols = BUILD_COLS_FULL,
    );
    let row = client
        .query_opt(sql.as_str(), &[&build_id])
        .await
        .map_err(|err| err.to_string())?;
    Ok(row.as_ref().map(|r| row_to_build(r, true)))
}

async fn insert_revision(
    client: &tokio_postgres::Client,
    entry: &CatalogEntry,
    text: &str,
    source: &str,
    notes: &str,
) -> Result<RevisionRow, String> {
    let sha = sha256_hex(text);
    if let Some(existing) = fetch_revision_by_sha(client, entry.slug, &sha).await? {
        return Ok(existing);
    }
    let sql = format!(
        "insert into container_pool_image_revisions \
         (image_slug, image_ref, dockerfile_path, build_context, dockerfile_text, \
          dockerfile_sha256, source, notes) \
         values ($1, $2, $3, $4, $5, $6, $7, $8) \
         returning {cols}",
        cols = REVISION_COLS_FULL,
    );
    let row = client
        .query_one(
            sql.as_str(),
            &[
                &entry.slug,
                &entry.image_ref,
                &entry.dockerfile_path,
                &entry.build_context,
                &text,
                &sha,
                &source,
                &notes,
            ],
        )
        .await
        .map_err(|err| format!("insert revision failed: {err}"))?;
    Ok(row_to_revision(&row, true))
}

/// Returns the active "current" revision for a slug:
///   1. Latest user-saved revision, if any.
///   2. Otherwise, the synthetic disk-default revision (inserted on demand).
async fn current_revision(
    client: &tokio_postgres::Client,
    entry: &CatalogEntry,
) -> Result<RevisionRow, String> {
    if let Some(latest) = latest_revision_for(client, entry.slug).await? {
        if let Some(full) = fetch_revision_by_id(client, &latest.id).await? {
            return Ok(full);
        }
    }
    let text = read_disk_dockerfile(entry)?;
    insert_revision(
        client,
        entry,
        &text,
        "disk-default",
        "Loaded from on-disk Dockerfile.",
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP responses
// ─────────────────────────────────────────────────────────────────────────────

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = json!({ "ok": false, "error": message.into() });
    (status, Json(body)).into_response()
}

fn ok_response(value: Value) -> Response {
    let mut body = value;
    if let Some(obj) = body.as_object_mut() {
        obj.entry("ok".to_string()).or_insert(json!(true));
    }
    (StatusCode::OK, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

async fn list_images() -> Response {
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    let mut images: Vec<ImageSummary> = Vec::with_capacity(CATALOG.len());
    for entry in CATALOG.iter() {
        let disk_sha = read_disk_dockerfile(entry)
            .ok()
            .map(|text| sha256_hex(&text));
        let latest_revision = latest_revision_for(&client, entry.slug)
            .await
            .ok()
            .flatten();
        let latest_build = latest_build_for(&client, entry.slug).await.ok().flatten();
        let revision_count = revision_count_for(&client, entry.slug).await.unwrap_or(0);
        let build_count = build_count_for(&client, entry.slug).await.unwrap_or(0);
        images.push(ImageSummary {
            slug: entry.slug.to_string(),
            display_name: entry.display_name.to_string(),
            image_ref: entry.image_ref.to_string(),
            dockerfile_path: entry.dockerfile_path.to_string(),
            build_context: entry.build_context.to_string(),
            default_test_command: entry.default_test_command.to_string(),
            notes: entry.notes.to_string(),
            disk_default_sha256: disk_sha,
            latest_revision,
            latest_build,
            revision_count,
            build_count,
        });
    }
    ok_response(json!({
        "images": images,
        "buildsEnabled": builds_enabled(),
        "namespace": build_namespace(),
        "repoRoot": image_repo_root().display().to_string(),
    }))
}

async fn get_image(Path(slug): Path<String>) -> Response {
    let Some(entry) = catalog_entry(&slug) else {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    };
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    let current = match current_revision(&client, entry).await {
        Ok(r) => r,
        Err(err) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("revision: {err}"),
            )
        }
    };
    let latest_build = latest_build_for(&client, entry.slug).await.ok().flatten();
    let disk_default = read_disk_dockerfile(entry).ok();
    let disk_default_sha = disk_default.as_ref().map(|text| sha256_hex(text));
    ok_response(json!({
        "image": {
            "slug": entry.slug,
            "displayName": entry.display_name,
            "imageRef": entry.image_ref,
            "dockerfilePath": entry.dockerfile_path,
            "buildContext": entry.build_context,
            "defaultTestCommand": entry.default_test_command,
            "notes": entry.notes,
        },
        "currentRevision": current,
        "latestBuild": latest_build,
        "diskDefault": disk_default,
        "diskDefaultSha256": disk_default_sha,
        "buildsEnabled": builds_enabled(),
        "namespace": build_namespace(),
    }))
}

#[derive(Deserialize, Default)]
struct DockerfileQuery {
    #[serde(rename = "revisionId")]
    revision_id: Option<String>,
    source: Option<String>,
}

async fn get_dockerfile(
    Path(slug): Path<String>,
    Query(query): Query<DockerfileQuery>,
) -> Response {
    let Some(entry) = catalog_entry(&slug) else {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    };
    if matches!(query.source.as_deref(), Some("disk-default")) {
        return match read_disk_dockerfile(entry) {
            Ok(text) => {
                let sha = sha256_hex(&text);
                ok_response(json!({
                    "slug": entry.slug,
                    "source": "disk-default",
                    "dockerfilePath": entry.dockerfile_path,
                    "buildContext": entry.build_context,
                    "dockerfileText": text,
                    "dockerfileSha256": sha,
                }))
            }
            Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err),
        };
    }
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    if let Some(revision_id) = query.revision_id.as_deref() {
        return match fetch_revision_by_id(&client, revision_id).await {
            Ok(Some(revision)) if revision.image_slug == slug => ok_response(json!({
                "slug": entry.slug,
                "source": "revision",
                "revision": revision,
            })),
            Ok(_) => error_response(StatusCode::NOT_FOUND, "revision not found"),
            Err(err) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("revision: {err}"),
            ),
        };
    }
    match current_revision(&client, entry).await {
        Ok(revision) => ok_response(json!({
            "slug": entry.slug,
            "source": revision.source,
            "revision": revision,
        })),
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("revision: {err}"),
        ),
    }
}

#[derive(Deserialize)]
struct PutDockerfileBody {
    #[serde(rename = "dockerfileText")]
    dockerfile_text: String,
    #[serde(default)]
    notes: Option<String>,
}

async fn put_dockerfile(Path(slug): Path<String>, Json(body): Json<PutDockerfileBody>) -> Response {
    let Some(entry) = catalog_entry(&slug) else {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    };
    let text = body.dockerfile_text;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "dockerfileText must not be empty");
    }
    if text.len() > 65_536 {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "dockerfileText must be <= 65536 bytes",
        );
    }
    if !text.contains("FROM ") && !text.contains("FROM\t") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "dockerfileText must contain a FROM instruction",
        );
    }
    let notes = body
        .notes
        .unwrap_or_default()
        .chars()
        .take(8_000)
        .collect::<String>();
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    match insert_revision(&client, entry, &text, "user", &notes).await {
        Ok(revision) => ok_response(json!({
            "slug": entry.slug,
            "revision": revision,
        })),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("insert: {err}")),
    }
}

#[derive(Deserialize, Default)]
struct LimitQuery {
    limit: Option<i64>,
}

fn clamp_limit(value: Option<i64>, default: i64, max: i64) -> i64 {
    let v = value.unwrap_or(default);
    v.clamp(1, max)
}

async fn list_revisions(Path(slug): Path<String>, Query(query): Query<LimitQuery>) -> Response {
    if catalog_entry(&slug).is_none() {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    }
    let limit = clamp_limit(query.limit, 25, 100);
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    let sql = format!(
        "select {cols} from container_pool_image_revisions \
         where image_slug = $1 and is_soft_deleted = false \
         order by created_at desc limit $2",
        cols = REVISION_COLS,
    );
    let rows = match client.query(sql.as_str(), &[&slug, &limit]).await {
        Ok(r) => r,
        Err(err) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("query: {err}"))
        }
    };
    let revisions: Vec<RevisionRow> = rows.iter().map(|r| row_to_revision(r, false)).collect();
    ok_response(json!({ "slug": slug, "revisions": revisions }))
}

async fn list_builds(Path(slug): Path<String>, Query(query): Query<LimitQuery>) -> Response {
    if catalog_entry(&slug).is_none() {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    }
    let limit = clamp_limit(query.limit, 25, 100);
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    let sql = format!(
        "select {cols} from container_pool_build_runs \
         where image_slug = $1 and is_soft_deleted = false \
         order by created_at desc limit $2",
        cols = BUILD_COLS,
    );
    let rows = match client.query(sql.as_str(), &[&slug, &limit]).await {
        Ok(r) => r,
        Err(err) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("query: {err}"))
        }
    };
    let builds: Vec<BuildRunRow> = rows.iter().map(|r| row_to_build(r, false)).collect();
    ok_response(json!({ "slug": slug, "builds": builds }))
}

async fn get_build(Path(build_id): Path<String>) -> Response {
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    match fetch_build_by_id(&client, &build_id).await {
        Ok(Some(build)) => ok_response(json!({ "build": build })),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "build not found"),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("build: {err}")),
    }
}

#[derive(Clone, Deserialize, Serialize, Default)]
struct BuildTestBody {
    #[serde(rename = "revisionId")]
    revision_id: Option<String>,
    #[serde(rename = "dockerfileText")]
    dockerfile_text: Option<String>,
    #[serde(rename = "testCommand")]
    test_command: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

async fn delegate_build_test(slug: &str, body: &BuildTestBody) -> Result<Response, String> {
    let base_url = image_build_delegate_url()
        .ok_or_else(|| "IMAGE_BUILD_DELEGATE_URL is not configured".to_string())?;
    let secret = operator_api_secret()
        .ok_or_else(|| "image builder delegation auth is not configured".to_string())?;
    let url = format!("{base_url}/api/container-pool/images/{slug}/build-test");
    tracing::info!(image_slug = %slug, delegate = %base_url, "delegating container image build");
    let response = reqwest::Client::builder()
        .timeout(image_build_delegate_timeout())
        .build()
        .map_err(|error| format!("failed to create image builder client: {error}"))?
        .post(url)
        .header("x-server-auth", secret)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("image builder request failed: {error}"))?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|error| format!("image builder returned an invalid status: {error}"))?;
    let payload = response
        .json::<Value>()
        .await
        .map_err(|error| format!("image builder returned invalid JSON: {error}"))?;
    tracing::info!(image_slug = %slug, http_status = status.as_u16(), "container image build delegation accepted");
    Ok((status, Json(payload)).into_response())
}

async fn trigger_build_test(Path(slug): Path<String>, Json(body): Json<BuildTestBody>) -> Response {
    let Some(entry) = catalog_entry(&slug) else {
        return error_response(StatusCode::NOT_FOUND, "unknown image slug");
    };
    if !builds_enabled() {
        return error_response(
            StatusCode::PRECONDITION_FAILED,
            "container pool image builds disabled (CONTAINER_POOL_IMAGE_BUILDS_ENABLED=false)",
        );
    }
    if image_build_delegate_url().is_some() {
        return match delegate_build_test(&slug, &body).await {
            Ok(response) => response,
            Err(error) => error_response(StatusCode::BAD_GATEWAY, error),
        };
    }
    let client = match connect_postgres().await {
        Ok(c) => c,
        Err(err) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, format!("postgres: {err}"))
        }
    };
    let revision = if let Some(text) = body.dockerfile_text.as_deref() {
        let notes = body
            .notes
            .clone()
            .unwrap_or_default()
            .chars()
            .take(8_000)
            .collect::<String>();
        match insert_revision(&client, entry, text, "user", &notes).await {
            Ok(r) => r,
            Err(err) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("insert: {err}"))
            }
        }
    } else if let Some(id) = body.revision_id.as_deref() {
        match fetch_revision_by_id(&client, id).await {
            Ok(Some(r)) if r.image_slug == slug => r,
            Ok(_) => return error_response(StatusCode::NOT_FOUND, "revision not found"),
            Err(err) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("revision: {err}"),
                )
            }
        }
    } else {
        match current_revision(&client, entry).await {
            Ok(r) => r,
            Err(err) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("revision: {err}"),
                )
            }
        }
    };
    let requested_test_command = body
        .test_command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if requested_test_command.is_some() && !custom_test_commands_enabled() {
        return error_response(
            StatusCode::FORBIDDEN,
            "custom testCommand is disabled; set CONTAINER_POOL_IMAGE_CUSTOM_TEST_COMMANDS_ENABLED=true to allow it",
        );
    }
    let test_command = requested_test_command.unwrap_or(entry.default_test_command);
    if test_command.len() > 4_096 || test_command.contains('\0') {
        return error_response(
            StatusCode::BAD_REQUEST,
            "testCommand must be <= 4096 bytes and must not contain NUL bytes",
        );
    }
    let test_command = test_command.to_string();
    let candidate_tag = format!(
        "{}-cpool-test:{}",
        entry.image_ref.split(':').next().unwrap_or(entry.image_ref),
        &revision.id.replace('-', "")[..16],
    );
    let insert_sql = format!(
        "insert into container_pool_build_runs \
         (image_slug, revision_id, image_ref, candidate_tag, build_status, test_status, \
          overall_status, test_command) \
         values ($1, $2::text::uuid, $3, $4, 'queued', 'not_started', 'queued', $5) \
         returning {cols}",
        cols = BUILD_COLS_FULL,
    );
    let build_row = match client
        .query_one(
            insert_sql.as_str(),
            &[
                &entry.slug,
                &revision.id,
                &entry.image_ref,
                &candidate_tag,
                &test_command,
            ],
        )
        .await
    {
        Ok(r) => row_to_build(&r, true),
        Err(err) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("insert build: {err}"),
            )
        }
    };
    let revision_for_task = revision.clone();
    let build_for_task = build_row.clone();
    let entry_clone: CatalogEntry = entry.clone();
    tracing::info!(
        image_slug = %slug,
        build_id = %build_row.id,
        candidate_tag = %build_row.candidate_tag,
        "local container image build accepted"
    );
    tokio::spawn(async move {
        let build_id = build_for_task.id.clone();
        match run_build_and_test(entry_clone, revision_for_task, build_for_task).await {
            Ok(()) => tracing::info!(%build_id, "local container image build completed"),
            Err(err) => {
                tracing::error!(%build_id, "container-pool build/test orchestration failed: {err}")
            }
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "build": build_row })),
    )
        .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Build / test orchestrator (background tokio task)
// ─────────────────────────────────────────────────────────────────────────────

async fn run_build_and_test(
    _entry: CatalogEntry,
    revision: RevisionRow,
    build: BuildRunRow,
) -> Result<(), String> {
    let build_id = build.id.clone();
    let candidate_tag = build.candidate_tag.clone();
    let test_command = build.test_command.clone();

    update_build_started(&build_id, "build").await?;

    let work_root = build_root();
    if let Err(err) = fs::create_dir_all(&work_root) {
        update_build_error(
            &build_id,
            "build",
            &format!("mkdir build root failed: {err}"),
        )
        .await?;
        return Err(format!("mkdir build root failed: {err}"));
    }
    let work_dir = work_root.join(&build_id);
    if work_dir.exists() {
        let _ = fs::remove_dir_all(&work_dir);
    }
    if let Err(err) = fs::create_dir_all(&work_dir) {
        update_build_error(&build_id, "build", &format!("mkdir work dir failed: {err}")).await?;
        return Err(format!("mkdir work dir failed: {err}"));
    }

    let dockerfile_path = work_dir.join("Dockerfile");
    let text = revision
        .dockerfile_text
        .clone()
        .unwrap_or_else(|| String::new());
    if let Err(err) = fs::write(&dockerfile_path, text.as_bytes()) {
        update_build_error(&build_id, "build", &format!("write dockerfile: {err}")).await?;
        return Err(format!("write dockerfile: {err}"));
    }

    let context_dir = match resolve_repo_path(&revision.build_context) {
        Ok(p) => p,
        Err(err) => {
            update_build_error(&build_id, "build", &format!("resolve context: {err}")).await?;
            return Err(err);
        }
    };

    // ── BUILD ────────────────────────────────────────────────────────────────
    let mut build_cmd = TokioCommand::new(nerdctl_binary());
    let namespace = build_namespace();
    if !namespace.trim().is_empty() {
        build_cmd.arg("-n").arg(&namespace);
    }
    build_cmd
        .arg("build")
        .arg("--progress=plain")
        .arg("-f")
        .arg(&dockerfile_path)
        .arg("-t")
        .arg(&candidate_tag)
        .arg(&context_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let build_started = std::time::Instant::now();
    let build_output = timeout(build_timeout(), build_cmd.output()).await;
    let build_elapsed_ms = build_started.elapsed().as_millis();
    match build_output {
        Err(_) => {
            let msg = format!(
                "nerdctl build timed out after {} seconds",
                build_timeout().as_secs()
            );
            update_build_finished(&build_id, "build", "failed", "", &msg).await?;
            return Err(msg);
        }
        Ok(Err(err)) => {
            let msg = format!("nerdctl build spawn failed: {err}");
            update_build_finished(&build_id, "build", "failed", "", &msg).await?;
            return Err(msg);
        }
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let log = truncate_log(
                &format!(
                    "$ nerdctl -n {ns} build -f {df} -t {tag} {ctx}\n\
                     -- exit: {code}, elapsed: {ms}ms --\n\
                     STDOUT:\n{out}\n\nSTDERR:\n{err}",
                    ns = namespace,
                    df = dockerfile_path.display(),
                    tag = candidate_tag,
                    ctx = context_dir.display(),
                    code = output.status.code().unwrap_or(-1),
                    ms = build_elapsed_ms,
                    out = stdout,
                    err = stderr,
                ),
                32_000,
            );
            if !output.status.success() {
                update_build_finished(
                    &build_id,
                    "build",
                    "failed",
                    &log,
                    "nerdctl build returned non-zero status",
                )
                .await?;
                let _ = fs::remove_dir_all(&work_dir);
                return Err("nerdctl build returned non-zero status".to_string());
            }
            update_build_finished(&build_id, "build", "built", &log, "").await?;
        }
    }

    // ── TEST ─────────────────────────────────────────────────────────────────
    //
    // Run the smoke test detached (-d) rather than --rm. When this orchestrator
    // is executed from inside a Kubernetes pod, `nerdctl run --rm` creates the
    // stdio FIFOs under the pod's /run/containerd/fifo namespace, which the
    // host's containerd-shim cannot see; that surfaces as:
    //
    //   containerd-shim: opening file "/run/containerd/fifo/<n>/<id>-stdout"
    //   failed: open ... : no such file or directory
    //
    // Detached mode avoids real-time stdio forwarding entirely: containerd
    // captures stdout/stderr into its own log files, we `wait` for exit, then
    // `logs` to retrieve output, and finally `rm` for cleanup. This is the
    // same pattern dd-container-pool uses for warm worker containers.
    update_build_started(&build_id, "test").await?;
    let test_container_name = format!("cpool-smoketest-{}", &build_id.replace('-', "")[..16]);

    let mut start_cmd = TokioCommand::new(nerdctl_binary());
    if !namespace.trim().is_empty() {
        start_cmd.arg("-n").arg(&namespace);
    }
    start_cmd
        .arg("run")
        .arg("-d")
        .arg("--name")
        .arg(&test_container_name)
        .arg("--network")
        .arg("host")
        .arg("--pull")
        .arg("never")
        .arg("--entrypoint")
        .arg("/bin/sh")
        .arg(&candidate_tag)
        .arg("-c")
        .arg(&test_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let test_started = std::time::Instant::now();
    let start_result = timeout(test_timeout(), start_cmd.output()).await;
    let _ = fs::remove_dir_all(&work_dir);

    let start_output = match start_result {
        Err(_) => {
            let msg = format!(
                "smoke test (`run -d`) timed out after {} seconds",
                test_timeout().as_secs()
            );
            update_build_finished(&build_id, "test", "failed", "", &msg).await?;
            return Err(msg);
        }
        Ok(Err(err)) => {
            let msg = format!("smoke test spawn failed: {err}");
            update_build_finished(&build_id, "test", "failed", "", &msg).await?;
            return Err(msg);
        }
        Ok(Ok(output)) => output,
    };

    if !start_output.status.success() {
        let stdout = String::from_utf8_lossy(&start_output.stdout);
        let stderr = String::from_utf8_lossy(&start_output.stderr);
        let log = truncate_log(
            &format!(
                "$ nerdctl -n {ns} run -d --name {name} --network host --pull never \
                 --entrypoint /bin/sh {tag} -c {cmd:?}\n\
                 -- exit: {code} --\nSTDOUT:\n{out}\n\nSTDERR:\n{err}",
                ns = namespace,
                name = test_container_name,
                tag = candidate_tag,
                cmd = test_command,
                code = start_output.status.code().unwrap_or(-1),
                out = stdout,
                err = stderr,
            ),
            32_000,
        );
        let _ = nerdctl_rm(&namespace, &test_container_name).await;
        update_build_finished(
            &build_id,
            "test",
            "failed",
            &log,
            "smoke test container failed to start",
        )
        .await?;
        return Err("smoke test failed to start".to_string());
    }

    // Wait for the test container to exit (or hit the configured budget).
    let wait_budget = test_timeout();
    let mut wait_cmd = TokioCommand::new(nerdctl_binary());
    if !namespace.trim().is_empty() {
        wait_cmd.arg("-n").arg(&namespace);
    }
    wait_cmd
        .arg("wait")
        .arg(&test_container_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let wait_result = timeout(wait_budget, wait_cmd.output()).await;

    // Pull logs regardless of how wait turned out so the operator UI shows
    // something useful even on timeout.
    let mut logs_cmd = TokioCommand::new(nerdctl_binary());
    if !namespace.trim().is_empty() {
        logs_cmd.arg("-n").arg(&namespace);
    }
    logs_cmd
        .arg("logs")
        .arg(&test_container_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let logs_output = timeout(Duration::from_secs(10), logs_cmd.output()).await;
    let test_elapsed_ms = test_started.elapsed().as_millis();
    let (logs_stdout, logs_stderr) = match logs_output {
        Ok(Ok(o)) => (
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        ),
        _ => (String::new(), String::new()),
    };
    let _ = nerdctl_rm(&namespace, &test_container_name).await;

    match wait_result {
        Err(_) => {
            let msg = format!(
                "smoke test exceeded {} seconds in `nerdctl wait`",
                wait_budget.as_secs()
            );
            let log = truncate_log(
                &format!(
                    "$ nerdctl -n {ns} run -d ... && wait && logs && rm  ({tag})\n\
                     -- TIMED OUT after {ms}ms --\nLOGS STDOUT:\n{out}\n\nLOGS STDERR:\n{err}",
                    ns = namespace,
                    tag = candidate_tag,
                    ms = test_elapsed_ms,
                    out = logs_stdout,
                    err = logs_stderr,
                ),
                32_000,
            );
            update_build_finished(&build_id, "test", "failed", &log, &msg).await?;
            return Err(msg);
        }
        Ok(Err(err)) => {
            let msg = format!("smoke test `nerdctl wait` failed to spawn: {err}");
            update_build_finished(&build_id, "test", "failed", "", &msg).await?;
            return Err(msg);
        }
        Ok(Ok(wait_out)) => {
            // `nerdctl wait` prints the container exit code on stdout.
            let stdout = String::from_utf8_lossy(&wait_out.stdout);
            let exit_code: i32 = stdout
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .parse()
                .unwrap_or(-1);
            let log = truncate_log(
                &format!(
                    "$ nerdctl -n {ns} run -d --name {name} --network host --pull never \
                     --entrypoint /bin/sh {tag} -c {cmd:?}\n\
                     -- container exit: {exit_code}, elapsed: {ms}ms --\n\
                     LOGS STDOUT:\n{out}\n\nLOGS STDERR:\n{err}",
                    ns = namespace,
                    name = test_container_name,
                    tag = candidate_tag,
                    cmd = test_command,
                    ms = test_elapsed_ms,
                    out = logs_stdout,
                    err = logs_stderr,
                ),
                32_000,
            );
            if exit_code != 0 {
                update_build_finished(
                    &build_id,
                    "test",
                    "failed",
                    &log,
                    &format!("smoke test container exited with code {exit_code}"),
                )
                .await?;
                return Err(format!("smoke test failed (exit={exit_code})"));
            }
            update_build_finished(&build_id, "test", "passed", &log, "").await?;
        }
    }

    Ok(())
}

async fn nerdctl_rm(namespace: &str, container_name: &str) -> Result<(), String> {
    let mut cmd = TokioCommand::new(nerdctl_binary());
    if !namespace.trim().is_empty() {
        cmd.arg("-n").arg(namespace);
    }
    cmd.arg("rm")
        .arg("-f")
        .arg(container_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = timeout(Duration::from_secs(15), cmd.output()).await;
    Ok(())
}

async fn update_build_started(build_id: &str, phase: &str) -> Result<(), String> {
    let client = connect_postgres().await?;
    let (sql, status_value, overall) = match phase {
        "build" => (
            "update container_pool_build_runs set \
             build_started_at = now(), build_status = 'building', overall_status = 'running', \
             updated_at = now() where id = $1::text::uuid",
            "building",
            "running",
        ),
        "test" => (
            "update container_pool_build_runs set \
             test_started_at = now(), test_status = 'testing', overall_status = 'running', \
             updated_at = now() where id = $1::text::uuid",
            "testing",
            "running",
        ),
        _ => return Err(format!("unknown phase: {phase}")),
    };
    let _ = status_value;
    let _ = overall;
    client
        .execute(sql, &[&build_id])
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn update_build_finished(
    build_id: &str,
    phase: &str,
    status: &str,
    log: &str,
    error_message: &str,
) -> Result<(), String> {
    let client = connect_postgres().await?;
    let error_opt: Option<&str> = if error_message.is_empty() {
        None
    } else {
        Some(error_message)
    };
    let log_owned = truncate_log(log, 60_000);
    let log_ref: &str = log_owned.as_str();
    // Compute the next overall_status here so each SQL parameter has exactly one
    // role (otherwise tokio-postgres rejects with "inconsistent types deduced
    // for parameter $N" when the same placeholder is used as both an assignment
    // target and a comparison operand).
    let overall_next: Option<&str> = match phase {
        "build" if status == "failed" => Some("failed"),
        "test" if status == "passed" => Some("passed"),
        "test" if status == "failed" => Some("failed"),
        _ => None,
    };
    let sql = match phase {
        "build" => {
            "update container_pool_build_runs set \
             build_status = $2::varchar, \
             build_finished_at = now(), \
             build_log_excerpt = $3::text, \
             error_message = coalesce($4::text, error_message), \
             overall_status = coalesce($5::varchar, overall_status), \
             updated_at = now() \
             where id = $1::text::uuid"
        }
        "test" => {
            "update container_pool_build_runs set \
             test_status = $2::varchar, \
             test_finished_at = now(), \
             test_log_excerpt = $3::text, \
             error_message = coalesce($4::text, error_message), \
             overall_status = coalesce($5::varchar, overall_status), \
             updated_at = now() \
             where id = $1::text::uuid"
        }
        _ => return Err(format!("unknown phase: {phase}")),
    };
    client
        .execute(
            sql,
            &[&build_id, &status, &log_ref, &error_opt, &overall_next],
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn update_build_error(build_id: &str, phase: &str, message: &str) -> Result<(), String> {
    let client = connect_postgres().await?;
    let _ = phase;
    let _ = client
        .execute(
            "update container_pool_build_runs set \
             overall_status = 'errored', error_message = $2, updated_at = now() \
             where id = $1::text::uuid",
            &[&build_id, &message],
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Router export
// ─────────────────────────────────────────────────────────────────────────────

pub fn router() -> Router {
    Router::new()
        .route("/api/container-pool/images", get(list_images))
        .route("/api/container-pool/images/:slug", get(get_image))
        .route(
            "/api/container-pool/images/:slug/dockerfile",
            get(get_dockerfile).put(put_dockerfile),
        )
        .route(
            "/api/container-pool/images/:slug/revisions",
            get(list_revisions),
        )
        .route("/api/container-pool/images/:slug/builds", get(list_builds))
        .route(
            "/api/container-pool/images/:slug/build-test",
            post(trigger_build_test),
        )
        .route("/api/container-pool/builds/:build_id", get(get_build))
        .route_layer(middleware::from_fn(require_operator_auth))
}

/// Minimal privileged-builder surface. The builder role intentionally omits
/// the catalog read/write and build-status APIs; only the authenticated build
/// trigger is reachable inside its NetworkPolicy boundary.
pub fn builder_router() -> Router {
    Router::new()
        .route(
            "/api/container-pool/images/:slug/build-test",
            post(trigger_build_test),
        )
        .route_layer(middleware::from_fn(require_operator_auth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn builder_router_exposes_only_authenticated_build_trigger() {
        let catalog_response = builder_router()
            .oneshot(
                Request::builder()
                    .uri("/api/container-pool/images")
                    .body(Body::empty())
                    .expect("catalog request"),
            )
            .await
            .expect("catalog response");
        assert_eq!(catalog_response.status(), StatusCode::NOT_FOUND);

        let build_response = builder_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/container-pool/images/nodejs/build-test")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("build request"),
            )
            .await
            .expect("build response");
        assert_eq!(build_response.status(), StatusCode::UNAUTHORIZED);
    }
}
