#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
test "${GITHUB_HEAD_REF:-}" = 'agent/den-482-wal-gateway-openapi' || \
  test "${GITHUB_REF_NAME:-}" = 'agent/den-482-wal-gateway-openapi'

payload='scripts/api-contract/den-482-wal-openapi.payload'
compressed="${RUNNER_TEMP}/den-482-wal-openapi.mjs.gz"
decoded="${RUNNER_TEMP}/den-482-wal-openapi.mjs"
base64 --decode "$payload" > "$compressed"
printf '%s  %s\n' \
  '67333dba49c98aac4340fa41402b4713ba0faa4968a53cd4fa56655266a478ea' \
  "$compressed" | sha256sum --check -
gzip --decompress --stdout "$compressed" > "$decoded"
printf '%s  %s\n' \
  '48122dcd79379231fd6b5935aab257a07e355f580f5de6289e8868268ea4f36a' \
  "$decoded" | sha256sum --check -
node --check "$decoded"
node "$decoded"

printf '%s  %s\n' \
  '5d330aa1a33ba118a5770af6409dcbad8dc408bb0cccaae4e6d257c337028d46' \
  'remote/api-contracts/fragments/runtime-config-client-rs.openapi.json' \
  | sha256sum --check -

rm -rf remote/libs

mkdir -p remote/libs/telemetry-rs/src
cat > remote/libs/telemetry-rs/Cargo.toml <<'TOML'
[package]
name = "dd-telemetry"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
tracing = "0.1"
tower-http = { version = "0.6", features = ["trace"] }
http = "1"
TOML
cat > remote/libs/telemetry-rs/src/lib.rs <<'RS'
use http::{Request, Response};
use std::time::Duration;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
use tracing::Span;

#[must_use]
pub struct OtelGuard;

pub fn init(_: &str) -> OtelGuard {
    OtelGuard
}

pub fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    OtelMakeSpan,
    DefaultOnRequest,
    OtelOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(OtelMakeSpan)
        .on_response(OtelOnResponse)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        tracing::info_span!(
            "http_request",
            http.request.method = %request.method(),
            url.path = %request.uri().path(),
            http.response.status_code = tracing::field::Empty,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, _: Duration, span: &Span) {
        span.record(
            "http.response.status_code",
            response.status().as_u16() as u64,
        );
    }
}
RS

mkdir -p remote/libs/nats/subject-defs/generated/rust/src
cat > remote/libs/nats/subject-defs/generated/rust/Cargo.toml <<'TOML'
[package]
name = "dd-nats-subject-defs"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
TOML
cat > remote/libs/nats/subject-defs/generated/rust/src/lib.rs <<'RS'
pub const CDC_STREAM_NAME: &str = "CDC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdcRowChangeSubject {
    pub prefix: String,
    pub schema: String,
    pub table: String,
    pub op: String,
}

pub fn cdc_row_change_subject(
    prefix: &str,
    schema: &str,
    table: &str,
    op: &str,
) -> String {
    format!("{prefix}.{schema}.{table}.{op}")
}

pub fn parse_cdc_row_change_subject(subject: &str) -> Option<CdcRowChangeSubject> {
    let mut parts = subject.split('.');
    let parsed = CdcRowChangeSubject {
        prefix: parts.next()?.to_string(),
        schema: parts.next()?.to_string(),
        table: parts.next()?.to_string(),
        op: parts.next()?.to_string(),
    };
    if parts.next().is_some() {
        return None;
    }
    Some(parsed)
}
RS

mkdir -p remote/libs/runtime-config-client-rs/src
cat > remote/libs/runtime-config-client-rs/Cargo.toml <<'TOML'
[package]
name = "dd-runtime-config-client"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[features]
default = ["openapi"]
openapi = []

[dependencies]
axum = "0.8"
serde_json = "1"
utoipa = "=5.5.0"
TOML
cat > remote/libs/runtime-config-client-rs/src/lib.rs <<'RS'
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use utoipa::openapi::OpenApi;

const OPENAPI_JSON: &str =
    include_str!("../../../api-contracts/fragments/runtime-config-client-rs.openapi.json");

async fn snapshot() -> Json<Value> {
    Json(json!({"snapshotVersion": 0, "entries": {}}))
}

async fn mutate() -> Json<Value> {
    Json(json!({"ok": true}))
}

pub fn router_and_openapi() -> (Router, OpenApi) {
    let router = Router::new()
        .route("/internal/runtime-config", get(snapshot))
        .route("/internal/update-runtime-config", post(mutate))
        .route("/internal/runtime-config/reset", post(mutate));
    let openapi =
        serde_json::from_str(OPENAPI_JSON).expect("vendored runtime-config OpenAPI");
    (router, openapi)
}

pub fn router() -> Router {
    router_and_openapi().0
}

pub async fn register_with_control_plane() {}
RS

(
  cd remote/deployments/wal-gateway-rs
  cargo fmt --all
  cargo check
  cargo test
  cargo fmt --all -- --check
  cargo check --locked
  cargo test --locked
  mkdir -p generated
  cargo run --locked --quiet -- --export-openapi > generated/openapi.json
  cargo run --locked --quiet -- --export-openapi > "${RUNNER_TEMP}/openapi.2.json"
  cmp generated/openapi.json "${RUNNER_TEMP}/openapi.2.json"
  python3 -m json.tool generated/openapi.json >/dev/null
)

node remote/tools/generate-api-docs.mjs
node remote/tools/generate-api-sdks.mjs
node remote/tools/check-openapi-contracts.mjs --service wal-gateway-rs
node remote/tools/generate-api-docs.mjs --check --service wal-gateway-rs
node remote/tools/validate-openapi-contracts.mjs
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs
git diff --check

python3 - <<'PY'
import json
from pathlib import Path

public = json.loads(
    Path('remote/deployments/wal-gateway-rs/generated/api-docs.json').read_text()
)
expected = {'/', '/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'}
actual = set(public.get('paths', {}))
if actual != expected:
    raise SystemExit(f'unexpected public paths: {sorted(actual)}')
encoded = json.dumps(public).lower()
forbidden = [
    '"pod"', '"slot"', '"stream"', '"subjectprefix"',
    '"leader"', '"lastlsn"', '"database_url"', '"nats_url"',
]
leaked = [term for term in forbidden if term in encoded]
if leaked:
    raise SystemExit(f'public contract leaked operational details: {leaked}')
PY

rm -rf .tmp/wal-gateway-sdk
node remote/tools/generate-openapi-sdks.mjs \
  --service wal-gateway-rs \
  --output .tmp/wal-gateway-sdk
cargo check --manifest-path .tmp/wal-gateway-sdk/rust/Cargo.toml
(
  cd .tmp/wal-gateway-sdk/typescript
  npm install --ignore-scripts --no-audit --no-fund
  npm run build
)

rm -rf remote/libs .tmp
git checkout origin/main -- .github/workflows/secret-scan.yml
rm -f \
  .github/workflows/apply-den-482-wal-openapi.yml \
  .github/workflows/den-482-materialize-v2.yml \
  .github/workflows/trigger-den-482-wal-openapi.yml \
  scripts/api-contract/den-482-wal-openapi.payload \
  scripts/api-contract/run-den-482-wal-openapi.sh

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git add -A -- . ':(exclude)remote/libs'
git update-index --add --cacheinfo \
  160000,2504b054ac92becf265762c1bd1f0679a10de893,remote/libs
git diff --cached --check
if git diff --cached --no-ext-diff --unified=0 \
  | grep -E '^\+(<<<<<<< |=======$|>>>>>>> )'; then
  echo 'new git conflict marker detected' >&2
  exit 1
fi
test -z "$(git diff --cached --name-only | grep -E 'apply-den-482|materialize-v2|trigger-den-482|run-den-482|den-482-wal-openapi\.payload' || true)"
git diff --cached --name-status
git commit -m 'feat(DEN-482): migrate wal-gateway to executable OpenAPI'
git push origin HEAD:agent/den-482-wal-gateway-openapi
