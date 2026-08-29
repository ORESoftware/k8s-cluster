#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    env, fs,
    path::{Component, Path as FsPath, PathBuf},
    process::Command,
    time::Duration,
};

use axum::{
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::db::connect_postgres;
use crate::metrics::record_request;
use crate::shared::{
    authorized_image_builder_request, env_bool, looks_like_uuid, nats_lambda_functions_subject,
    nats_url, now_ms, postgres_database_url, public_data_source_error, row_bool, row_i32,
    row_opt_string, row_string, row_value, unauthorized_response, worker_auth_secret,
};
use crate::types::{
    LambdaFunctionRow, LambdaFunctionSaveRequest, LambdaFunctionsResponse, LambdasQuery,
    NatsLambdaFunctionMessage,
};

pub(crate) fn row_to_lambda_function(row: &tokio_postgres::Row) -> LambdaFunctionRow {
    LambdaFunctionRow {
        id: row_string(row, "id"),
        slug: row_string(row, "slug"),
        display_name: row_string(row, "display_name"),
        description: row_string(row, "description"),
        runtime: row_string(row, "runtime"),
        entry_command: row_string(row, "entry_command"),
        function_body: row_string(row, "function_body"),
        reuse_key: row_opt_string(row, "reuse_key"),
        idle_timeout_seconds: row_i32(row, "idle_timeout_seconds"),
        max_run_ms: row_i32(row, "max_run_ms"),
        containerized: row_bool(row, "containerized"),
        container_image: row_opt_string(row, "container_image"),
        container_build_status: row_string(row, "container_build_status"),
        container_build_error: row_opt_string(row, "container_build_error"),
        container_built_at: row_opt_string(row, "container_built_at"),
        status: row_string(row, "status"),
        labels: row_value(row, "labels", json!([])),
        meta_data: row_value(row, "meta_data", json!({})),
        last_invoked_at: row_opt_string(row, "last_invoked_at"),
        created_at: row_opt_string(row, "created_at"),
        updated_at: row_opt_string(row, "updated_at"),
    }
}

pub(crate) fn lambda_limit_from_query(query: &LambdasQuery) -> i64 {
    query.limit.unwrap_or(100).clamp(1, 250)
}

pub(crate) fn lambda_search_pattern(query: &LambdasQuery) -> String {
    query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"))
        .unwrap_or_default()
}

pub(crate) fn normalize_lambda_slug(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in input.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

pub(crate) fn validate_lambda_status(input: Option<&str>) -> String {
    match input.unwrap_or("draft").trim() {
        "draft" => "draft".to_string(),
        "active" => "active".to_string(),
        "paused" => "paused".to_string(),
        "archived" => "archived".to_string(),
        _ => "draft".to_string(),
    }
}

pub(crate) fn normalize_lambda_runtime_alias(input: &str) -> Option<&'static str> {
    match input.trim() {
        "node" | "nodejs" | "javascript" | "typescript" => Some("nodejs"),
        "python" | "python3" => Some("python3"),
        "ruby" => Some("ruby"),
        "bash" | "shell" => Some("bash"),
        "go" | "golang" => Some("golang"),
        "dart" => Some("dart"),
        "erlang" | "erl" => Some("erlang"),
        "elixir" | "ex" => Some("elixir"),
        "java" | "jvm" => Some("java"),
        "browser" | "playwright" | "puppeteer" | "chromium" | "headless" | "scraper" => {
            Some("browser")
        }
        _ => None,
    }
}

pub(crate) fn validate_lambda_runtime(input: Option<&str>) -> Result<String, String> {
    let value = input.unwrap_or("javascript");
    normalize_lambda_runtime_alias(value)
        .map(ToString::to_string)
        .ok_or_else(|| {
            "runtime must be one of nodejs, python3, ruby, bash, golang, dart, erlang, elixir, java, or browser (Playwright/Puppeteer)"
                .to_string()
        })
}

pub(crate) fn lambda_host_runtime_allowed(runtime: &str) -> bool {
    env::var("LAMBDA_ALLOW_HOST_RUNTIMES")
        .unwrap_or_else(|_| "nodejs".to_string())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(normalize_lambda_runtime_alias)
        .any(|allowed| allowed == runtime)
}

pub(crate) fn validate_lambda_reuse_key(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(reuse_key) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if reuse_key.len() > 120 {
        return Err("reuseKey must be 120 characters or fewer".to_string());
    }
    if !reuse_key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-'))
        || !reuse_key
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return Err(
            "reuseKey may contain only ASCII letters, numbers, '.', '_', ':', and '-' and must start with a letter or number"
                .to_string(),
        );
    }
    Ok(Some(reuse_key.to_string()))
}

pub(crate) fn lambda_select_sql() -> &'static str {
    r#"
    select
      id::text as id,
      slug,
      display_name,
      description,
      runtime,
      entry_command,
      function_body,
      reuse_key,
      idle_timeout_seconds,
      max_run_ms,
      containerized,
      container_image,
      container_build_status,
      container_build_error,
      to_char(container_built_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as container_built_at,
      status,
      labels,
      meta_data,
      to_char(last_invoked_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_invoked_at,
      to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
      to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
    from lambda_functions
    "#
}

pub(crate) fn lambda_entry_command_for_runtime(runtime: &str) -> String {
    match runtime {
        "python3" => {
            "env -i PATH=\"$PATH\" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py"
        }
        "ruby" => "env -i PATH=\"$PATH\" ruby child-runtimes/ruby-function-runner.rb",
        "bash" => {
            "env -i PATH=\"$PATH\" NODE_NO_WARNINGS=1 node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs"
        }
        "golang" | "dart" | "erlang" | "elixir" | "java" => {
            return format!(
                "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"{runtime}\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs"
            );
        }
        "browser" => {
            "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 node child-runtimes/browser-function-runner.mjs"
        }
        _ => {
            "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 node --permission --allow-net child-runtimes/js-function-runner.mjs"
        }
    }
    .to_string()
}

pub(crate) fn managed_lambda_entry_command(value: &str) -> bool {
    [
        "nodejs", "python3", "ruby", "bash", "golang", "dart", "erlang", "elixir", "java",
        "browser",
    ]
    .iter()
    .map(|runtime| lambda_entry_command_for_runtime(runtime))
    .any(|command| command == value)
}

pub(crate) fn validate_lambda_entry_command(value: Option<&str>, runtime: &str) -> Result<String, String> {
    let entry_command = lambda_entry_command_for_runtime(runtime);
    let Some(command) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(entry_command);
    };
    if !managed_lambda_entry_command(command) {
        return Err("entryCommand must use the managed lambda child runtime".to_string());
    }
    Ok(entry_command)
}

pub(crate) fn cleaned_lambda_input(
    request: &LambdaFunctionSaveRequest,
) -> Result<
    (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        i32,
        i32,
        bool,
        String,
        Value,
        Value,
    ),
    String,
> {
    let slug = normalize_lambda_slug(&request.slug);
    if slug.len() < 3 || slug.len() > 120 {
        return Err("slug must normalize to 3-120 characters".to_string());
    }

    let display_name = request.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err("displayName is required".to_string());
    }

    let function_body = request.function_body.trim().to_string();
    if function_body.is_empty() {
        return Err("functionBody is required".to_string());
    }
    if function_body.len() > 262_144 {
        return Err("functionBody exceeds configured byte limit".to_string());
    }

    let description = request
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let runtime = validate_lambda_runtime(request.runtime.as_deref())?;
    let entry_command = validate_lambda_entry_command(request.entry_command.as_deref(), &runtime)?;
    let reuse_key = validate_lambda_reuse_key(request.reuse_key.as_deref())?;
    let idle_timeout_seconds = request.idle_timeout_seconds.unwrap_or(300).clamp(1, 3600);
    let max_run_ms = request.max_run_ms.unwrap_or(30_000).clamp(1_000, 300_000);
    let containerized = request.containerized.unwrap_or(false);
    if !containerized && !lambda_host_runtime_allowed(&runtime) {
        return Err(format!(
            "{runtime} lambdas require containerized=true; host execution is disabled for this runtime"
        ));
    }
    let status = validate_lambda_status(request.status.as_deref());
    let labels = request.labels.clone().unwrap_or_else(|| json!([]));
    if !labels.is_array() {
        return Err("labels must be a JSON array".to_string());
    }
    let meta_data = request.meta_data.clone().unwrap_or_else(|| json!({}));
    if !meta_data.is_object() {
        return Err("metaData must be a JSON object".to_string());
    }

    Ok((
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ))
}

pub(crate) async fn fetch_lambda_functions_from_postgres(
    limit: i64,
    search_pattern: &str,
) -> Result<Vec<LambdaFunctionRow>, String> {
    let client = connect_postgres().await?;
    let rows = client
        .query(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and (
                    $2 = ''
                    or slug ilike $2
                    or display_name ilike $2
                    or description ilike $2
                  )
                order by updated_at desc, created_at desc
                limit $1
                "#,
                lambda_select_sql()
            ),
            &[&limit, &search_pattern],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows.iter().map(row_to_lambda_function).collect())
}

pub(crate) async fn fetch_lambda_function_by_slug(slug: &str) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and slug = $1
                limit 1
                "#,
                lambda_select_sql()
            ),
            &[&slug],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(row_to_lambda_function(&row))
}

pub(crate) async fn fetch_lambda_function_by_id(id: &str) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and id = $1::text::uuid
                limit 1
                "#,
                lambda_select_sql()
            ),
            &[&id],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(row_to_lambda_function(&row))
}

pub(crate) async fn fetch_lambda_function_by_identifier(
    identifier: &str,
) -> Result<LambdaFunctionRow, String> {
    let identifier = identifier.trim();
    if looks_like_uuid(identifier) {
        fetch_lambda_function_by_id(identifier).await
    } else {
        fetch_lambda_function_by_slug(identifier).await
    }
}

pub(crate) async fn insert_lambda_function_to_postgres(
    request: &LambdaFunctionSaveRequest,
) -> Result<LambdaFunctionRow, String> {
    let (
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ) = cleaned_lambda_input(request)?;
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            r#"
                insert into lambda_functions
                  (slug, display_name, description, runtime, entry_command, function_body, reuse_key,
                   idle_timeout_seconds, max_run_ms, containerized, container_build_status,
                   status, labels, meta_data, is_soft_deleted,
                   created_at, updated_at)
                values
                  ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   case when $10 then 'pending' else 'not_requested' end,
                   $11, $12, $13, false, now(), now())
                returning slug
                "#,
            &[
                &slug,
                &display_name,
                &description,
                &runtime,
                &entry_command,
                &function_body,
                &reuse_key,
                &idle_timeout_seconds,
                &max_run_ms,
                &containerized,
                &status,
                &labels,
                &meta_data,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    let returned_slug = row.try_get::<_, String>("slug").unwrap_or(slug);
    let function = fetch_lambda_function_by_slug(&returned_slug).await?;
    maybe_package_lambda_image(function).await
}

pub(crate) async fn update_lambda_function_in_postgres(
    id: &str,
    request: &LambdaFunctionSaveRequest,
) -> Result<LambdaFunctionRow, String> {
    let (
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ) = cleaned_lambda_input(request)?;
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            r#"
                update lambda_functions
                set
                  slug = $2,
                  display_name = $3,
                  description = $4,
                  runtime = $5,
                  entry_command = $6,
                  function_body = $7,
                  reuse_key = $8,
                  idle_timeout_seconds = $9,
                  max_run_ms = $10,
                  containerized = $11,
                  container_image = case when $11 then container_image else null end,
                  container_build_status = case when $11 then 'pending' else 'not_requested' end,
                  container_build_error = null,
                  container_built_at = case when $11 then container_built_at else null end,
                  status = $12,
                  labels = $13,
                  meta_data = $14,
                  updated_at = now()
                where id = $1::text::uuid
                  and is_soft_deleted = false
                returning slug
                "#,
            &[
                &id,
                &slug,
                &display_name,
                &description,
                &runtime,
                &entry_command,
                &function_body,
                &reuse_key,
                &idle_timeout_seconds,
                &max_run_ms,
                &containerized,
                &status,
                &labels,
                &meta_data,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    let returned_slug = row.try_get::<_, String>("slug").unwrap_or(slug);
    let function = fetch_lambda_function_by_slug(&returned_slug).await?;
    maybe_package_lambda_image(function).await
}

pub(crate) fn lambda_image_repository() -> String {
    env::var("LAMBDA_IMAGE_REPOSITORY")
        .unwrap_or_else(|_| "docker.io/library/dd-lambda-function".to_string())
}

pub(crate) fn lambda_image_tag(function: &LambdaFunctionRow) -> String {
    let short_id = function.id.chars().take(8).collect::<String>();
    format!(
        "{}:{}-{}",
        lambda_image_repository(),
        function.slug,
        short_id
    )
}

pub(crate) fn lambda_image_build_root() -> PathBuf {
    PathBuf::from(
        env::var("LAMBDA_IMAGE_BUILD_ROOT").unwrap_or_else(|_| "/var/lib/dd-lambdas".to_string()),
    )
}

pub(crate) fn validate_lambda_image_build_root(path: &FsPath) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("lambda image build root must be an absolute path".to_string());
    }
    if path.parent().is_none() {
        return Err("lambda image build root must not be filesystem root".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("lambda image build root must not contain . or .. path components".to_string());
    }
    Ok(())
}

pub(crate) fn lambda_image_repo_root() -> PathBuf {
    PathBuf::from(
        env::var("LAMBDA_IMAGE_REPO_ROOT").unwrap_or_else(|_| "/opt/dd-next-1".to_string()),
    )
}

pub(crate) fn lambda_image_build_namespace() -> String {
    env::var("LAMBDA_IMAGE_BUILD_NAMESPACE").unwrap_or_else(|_| "k8s.io".to_string())
}

pub(crate) fn lambda_image_build_nerdctl() -> String {
    env::var("LAMBDA_IMAGE_BUILD_NERDCTL").unwrap_or_else(|_| "/usr/local/bin/nerdctl".to_string())
}

pub(crate) fn lambda_runner_source(runtime: &str) -> (&'static str, &'static str) {
    match runtime {
        "python3" => ("python-function-runner.py", "runner.py"),
        "ruby" => ("ruby-function-runner.rb", "runner.rb"),
        "bash" => ("bash-function-runner.mjs", "runner.mjs"),
        "golang" | "dart" | "erlang" | "elixir" | "java" => {
            ("polyglot-function-runner.mjs", "runner.mjs")
        }
        "browser" => ("browser-function-runner.mjs", "runner.mjs"),
        _ => ("js-function-runner.mjs", "runner.mjs"),
    }
}

pub(crate) fn polyglot_lambda_container_dockerfile(
    runtime: &str,
    base_image: &str,
    package_install: &str,
    user_setup: &str,
    label: &str,
) -> String {
    format!(
        r#"FROM {base_image}
{package_install}
{user_setup}
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
ENV LAMBDA_TARGET_RUNTIME={runtime}
USER 10001:10001
ENTRYPOINT ["node", "/opt/dd-lambda/runner.mjs"]
"#
    )
}

pub(crate) fn lambda_container_dockerfile(runtime: &str, function: &LambdaFunctionRow) -> String {
    let label = format!(
        "LABEL dd.lambda.id=\"{}\" dd.lambda.slug=\"{}\" dd.lambda.runtime=\"{}\"",
        function.id, function.slug, runtime
    );
    match runtime {
        "python3" => format!(
            r#"FROM docker.io/library/python:3.12-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.py ./runner.py
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-lambda/runner.py"]
"#
        ),
        "ruby" => format!(
            r#"FROM docker.io/library/ruby:3.3-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.rb ./runner.rb
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["ruby", "/opt/dd-lambda/runner.rb"]
"#
        ),
        "bash" => format!(
            r#"FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  bash \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
ENV NODE_NO_WARNINGS=1
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "--allow-child-process", "/opt/dd-lambda/runner.mjs"]
"#
          ),
          "golang" => polyglot_lambda_container_dockerfile(
              runtime,
              "docker.io/library/golang:1.25-alpine",
              r#"RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current"#,
              r#"RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda"#,
              &label,
          ),
          "dart" => polyglot_lambda_container_dockerfile(
              runtime,
              "docker.io/library/dart:stable",
              "RUN apt-get update && apt-get install -y --no-install-recommends nodejs ca-certificates && apt-get clean",
              "RUN groupadd --system lambda && useradd --system --gid lambda --uid 10001 --create-home lambda",
              &label,
          ),
          "erlang" => polyglot_lambda_container_dockerfile(
              runtime,
              "docker.io/library/erlang:28-alpine",
              r#"RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current"#,
              r#"RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda"#,
              &label,
          ),
          "elixir" => polyglot_lambda_container_dockerfile(
              runtime,
              "docker.io/library/elixir:1.18-alpine",
              r#"RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current"#,
              r#"RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda"#,
              &label,
          ),
          "java" => polyglot_lambda_container_dockerfile(
              runtime,
              "docker.io/library/eclipse-temurin:21-jdk-alpine",
              r#"RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current"#,
              r#"RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda"#,
              &label,
          ),
          "browser" => format!(
              r#"FROM docker.io/library/dd-lambda-browser-runtime:dev
USER root
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
RUN chown 10001:10001 /opt/dd-lambda/runner.mjs /opt/dd-lambda/definition.json
USER 10001:10001
ENTRYPOINT ["node", "/opt/dd-lambda/runner.mjs"]
"#
          ),
          _ => format!(
              r#"FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
ENV NODE_NO_WARNINGS=1
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "/opt/dd-lambda/runner.mjs"]
"#
        ),
    }
}

pub(crate) fn copy_lambda_runner(
    repo_root: &FsPath,
    context_dir: &FsPath,
    runtime: &str,
) -> Result<(), String> {
    let (source_name, target_name) = lambda_runner_source(runtime);
    let source = repo_root
        .join("remote")
        .join("deployments")
        .join("gleam-lambda-runner")
        .join("child-runtimes")
        .join(source_name);
    let target = context_dir.join(target_name);
    fs::copy(&source, &target)
        .map(|_| ())
        .map_err(|error| format!("failed to copy lambda runner {}: {error}", source.display()))
}

pub(crate) fn harden_lambda_build_dir(path: &FsPath) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to restrict lambda image context: {error}"))?;
    }
    Ok(())
}

pub(crate) fn write_lambda_build_file(path: &FsPath, content: impl AsRef<[u8]>) -> Result<(), String> {
    fs::write(path, content).map_err(|error| {
        format!(
            "failed to write lambda image build file {}: {error}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "failed to restrict lambda image build file {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn package_lambda_image_sync(function: &LambdaFunctionRow, image: &str) -> Result<(), String> {
    let runtime = validate_lambda_runtime(Some(&function.runtime))?;
    let build_root = lambda_image_build_root();
    validate_lambda_image_build_root(&build_root)?;
    fs::create_dir_all(&build_root)
        .map_err(|error| format!("failed to create lambda image build root: {error}"))?;
    let context_dir = build_root.join(format!("lambda-{}", function.id));
    if context_dir.exists() {
        fs::remove_dir_all(&context_dir)
            .map_err(|error| format!("failed to reset lambda image context: {error}"))?;
    }
    fs::create_dir_all(&context_dir)
        .map_err(|error| format!("failed to create lambda image context: {error}"))?;
    harden_lambda_build_dir(&context_dir)?;
    copy_lambda_runner(&lambda_image_repo_root(), &context_dir, &runtime)?;
    write_lambda_build_file(
        &context_dir.join("definition.json"),
        serde_json::to_vec_pretty(function).map_err(|error| error.to_string())?,
    )?;
    write_lambda_build_file(
        &context_dir.join("Dockerfile"),
        lambda_container_dockerfile(&runtime, function),
    )?;

    let namespace = lambda_image_build_namespace();
    let mut command = Command::new(lambda_image_build_nerdctl());
    if !namespace.trim().is_empty() {
        command.arg("-n").arg(namespace);
    }
    command.arg("build").arg("-t").arg(image).arg(&context_dir);
    let output = command
        .output()
        .map_err(|error| format!("failed to run lambda image build: {error}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        return Err(format!(
            "lambda image build failed: {}",
            combined.chars().take(8192).collect::<String>()
        ));
    }
    Ok(())
}

pub(crate) async fn update_lambda_container_build(
    id: &str,
    image: Option<&str>,
    status: &str,
    error: Option<&str>,
    built: bool,
) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    client
        .execute(
            r#"
            update lambda_functions
            set
              container_image = $2,
              container_build_status = $3,
              container_build_error = $4,
              container_built_at = case when $5 then now() else container_built_at end,
              updated_at = now()
            where id = $1::text::uuid
              and is_soft_deleted = false
            "#,
            &[&id, &image, &status, &error, &built],
        )
        .await
        .map_err(|error| error.to_string())?;
    fetch_lambda_function_by_id(id).await
}

pub(crate) async fn maybe_package_lambda_image(
    function: LambdaFunctionRow,
) -> Result<LambdaFunctionRow, String> {
    if !function.containerized {
        return Ok(function);
    }

    let image = lambda_image_tag(&function);
    if !env_bool("LAMBDA_IMAGE_BUILD_ENABLED", false) {
        return update_lambda_container_build(
            &function.id,
            Some(&image),
            "skipped",
            Some("LAMBDA_IMAGE_BUILD_ENABLED is not true; image build deferred"),
            false,
        )
        .await
        .or(Ok(function));
    }

    let building =
        update_lambda_container_build(&function.id, Some(&image), "building", None, false)
            .await
            .unwrap_or(function);
    let result = if let Some(delegate_url) = image_build_delegate_url() {
        tracing::info!(
            function_id = %building.id,
            delegate = %delegate_url,
            "delegating lambda image build"
        );
        delegate_lambda_image(&delegate_url, &building.id).await
    } else {
        let build_input = building.clone();
        let image_for_build = image.clone();
        tokio::task::spawn_blocking(move || {
            package_lambda_image_sync(&build_input, &image_for_build)
        })
        .await
        .map_err(|error| error.to_string())?
    };

    match result {
        Ok(()) => update_lambda_container_build(&building.id, Some(&image), "built", None, true)
            .await
            .or(Ok(building)),
        Err(error) => {
            let public_error = error.chars().take(8192).collect::<String>();
            update_lambda_container_build(
                &building.id,
                Some(&image),
                "failed",
                Some(&public_error),
                false,
            )
            .await
            .or(Ok(building))
        }
    }
}

pub(crate) fn image_build_delegate_url() -> Option<String> {
    env::var("IMAGE_BUILD_DELEGATE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
}

pub(crate) async fn delegate_lambda_image(delegate_url: &str, function_id: &str) -> Result<(), String> {
    let secret = worker_auth_secret()
        .ok_or_else(|| "image builder delegation auth is not configured".to_string())?;
    let timeout_seconds = env::var("LAMBDA_IMAGE_BUILD_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1200)
        .clamp(60, 3600);
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|error| format!("failed to create image builder client: {error}"))?
        .post(format!(
            "{delegate_url}/internal/lambda-images/{function_id}/package"
        ))
        .header("x-server-auth", secret)
        .send()
        .await
        .map_err(|error| format!("image builder request failed: {error}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let detail = response.text().await.unwrap_or_default();
    Err(format!(
        "image builder returned HTTP {}: {}",
        status.as_u16(),
        detail.chars().take(8192).collect::<String>()
    ))
}

pub(crate) async fn package_lambda_image_internal(
    Path(function_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized_image_builder_request(&headers) {
        return unauthorized_response();
    }
    tracing::info!(%function_id, "internal lambda image build accepted");
    let function = match fetch_lambda_function_by_id(&function_id).await {
        Ok(function) => function,
        Err(error) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    };
    if !function.containerized {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "lambda is not containerized" })),
        )
            .into_response();
    }
    let image = lambda_image_tag(&function);
    let build_input = function.clone();
    let image_for_build = image.clone();
    match tokio::task::spawn_blocking(move || {
        package_lambda_image_sync(&build_input, &image_for_build)
    })
    .await
    {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "functionId": function.id, "image": image })),
        )
            .into_response(),
        Ok(Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": error.to_string() })),
        )
            .into_response(),
    }
}

pub(crate) async fn publish_lambda_function_update_to_nats(
    action: &str,
    function: &LambdaFunctionRow,
) -> Result<(), String> {
    let message = NatsLambdaFunctionMessage {
        version: 1,
        message_kind: "lambda.function.updated",
        action: action.to_string(),
        function_id: function.id.clone(),
        slug: function.slug.clone(),
        status: function.status.clone(),
        updated_at_ms: now_ms(),
    };
    let payload = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish(nats_lambda_functions_subject(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn image_builder_dependencies_ready() -> bool {
    FsPath::new("/run/containerd/containerd.sock").exists()
        && FsPath::new(&lambda_image_build_nerdctl()).exists()
        && FsPath::new("/usr/local/bin/buildctl").exists()
        && FsPath::new("/run/buildkit").is_dir()
        && worker_auth_secret().is_some()
        && postgres_database_url().is_some()
}

pub(crate) async fn image_builder_readyz() -> Response {
    let ready = image_builder_dependencies_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": "dd-image-builder",
            "dependenciesReady": ready,
        })),
    )
        .into_response()
}

pub(crate) async fn lambda_functions(Query(query): Query<LambdasQuery>) -> impl IntoResponse {
    record_request("GET", "/api/lambdas/functions", StatusCode::OK);
    if postgres_database_url().is_none() {
        return Json(LambdaFunctionsResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            functions: Vec::new(),
            errors: vec!["postgres database URL is not configured".to_string()],
        });
    }

    match fetch_lambda_functions_from_postgres(
        lambda_limit_from_query(&query),
        &lambda_search_pattern(&query),
    )
    .await
    {
        Ok(functions) => Json(LambdaFunctionsResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            functions,
            errors: Vec::new(),
        }),
        Err(error) => {
            tracing::error!("lambda functions postgres data source error: {error}");
            Json(LambdaFunctionsResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                functions: Vec::new(),
                errors: vec![public_data_source_error("postgres lambda functions")],
            })
        }
    }
}

pub(crate) async fn lambda_function(Path(identifier): Path<String>) -> Response {
    record_request("GET", "/api/lambdas/functions/:identifier", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match fetch_lambda_function_by_identifier(&identifier).await {
        Ok(function) => {
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            tracing::error!("lambda function fetch failed: {error}");
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "lambda function not found" })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn create_lambda_function(Json(request): Json<LambdaFunctionSaveRequest>) -> Response {
    record_request("POST", "/api/lambdas/functions", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match insert_lambda_function_to_postgres(&request).await {
        Ok(function) => {
            if let Err(error) = publish_lambda_function_update_to_nats("created", &function).await {
                tracing::error!("lambda function nats publish failed: {error}");
            }
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            tracing::error!("lambda function create failed: {error}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "failed to create lambda function" })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn update_lambda_function(
    Path(id): Path<String>,
    Json(request): Json<LambdaFunctionSaveRequest>,
) -> Response {
    record_request("PATCH", "/api/lambdas/functions/:id", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match update_lambda_function_in_postgres(&id, &request).await {
        Ok(function) => {
            if let Err(error) = publish_lambda_function_update_to_nats("updated", &function).await {
                tracing::error!("lambda function nats publish failed: {error}");
            }
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            tracing::error!("lambda function update failed: {error}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "failed to update lambda function" })),
            )
                .into_response()
        }
    }
}
