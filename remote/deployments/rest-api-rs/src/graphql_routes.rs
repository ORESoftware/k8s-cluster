use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_graphql::*;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response as AxumResponse},
    routing::get,
    Json as AxumJson, Router,
};
use redis::AsyncCommands;
use serde_json::{json, Value};

type RestApiSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

#[derive(Clone)]
struct RequestContext {
    authorized_control: bool,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlHealth {
    ok: bool,
    service: String,
    mode: String,
    graphql: bool,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlCapability {
    name: String,
    configured: bool,
    enabled: bool,
    notes: String,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentsDataConfig {
    rds_configured: bool,
    postgres_configured: bool,
    supabase_configured: bool,
    nats_configured: bool,
    nats_url: String,
    postgres_plan: String,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentsSummary {
    thread_count: i32,
    task_count: i32,
    running_count: i32,
    failed_count: i32,
    done_count: i32,
    pr_count: i32,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentThread {
    id: String,
    title: String,
    repo: String,
    base_branch: String,
    archived_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    task_count: i64,
    active_task_count: i64,
    latest_task_at: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentTask {
    id: String,
    thread_id: String,
    thread_title: Option<String>,
    prompt: String,
    status: String,
    branch: Option<String>,
    pr_url: Option<String>,
    pr_state: Option<String>,
    exit_reason: Option<String>,
    error_message: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    last_event_seq: i32,
    event_count: i64,
    latest_event_kind: Option<String>,
    latest_payload: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentEvent {
    task_id: String,
    seq: i32,
    event_kind: String,
    payload: Json<Value>,
    created_at: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlKnownGitRepo {
    id: String,
    repo_url: String,
    display_name: String,
    provider: String,
    default_branch: String,
    status: String,
    last_verified_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlLambdaFunction {
    id: String,
    slug: String,
    display_name: String,
    description: String,
    runtime: String,
    entry_command: String,
    function_body: String,
    reuse_key: Option<String>,
    idle_timeout_seconds: i32,
    max_run_ms: i32,
    containerized: bool,
    container_image: Option<String>,
    container_build_status: String,
    container_build_error: Option<String>,
    container_built_at: Option<String>,
    status: String,
    labels: Json<Value>,
    meta_data: Json<Value>,
    last_invoked_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlAgentsSnapshot {
    ok: bool,
    source: String,
    generated_at_ms: String,
    config: GraphqlAgentsDataConfig,
    summary: GraphqlAgentsSummary,
    threads: Vec<GraphqlAgentThread>,
    tasks: Vec<GraphqlAgentTask>,
    errors: Vec<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlTaskEvents {
    ok: bool,
    source: String,
    task_id: String,
    generated_at_ms: String,
    events: Vec<GraphqlAgentEvent>,
    errors: Vec<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlThreadContext {
    ok: bool,
    source: String,
    thread_id: String,
    generated_at_ms: String,
    tasks: Vec<GraphqlAgentTask>,
    errors: Vec<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlKnownGitRepos {
    ok: bool,
    source: String,
    generated_at_ms: String,
    repos: Vec<GraphqlKnownGitRepo>,
    errors: Vec<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlLambdaFunctions {
    ok: bool,
    source: String,
    generated_at_ms: String,
    functions: Vec<GraphqlLambdaFunction>,
    errors: Vec<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlDataSourceStatus {
    name: String,
    configured: bool,
    ok: bool,
    version: Option<String>,
    message: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlRedisValue {
    ok: bool,
    key: String,
    value: Option<String>,
    message: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlSubservice {
    name: String,
    base_url: String,
}

#[derive(InputObject)]
#[graphql(rename_fields = "camelCase")]
struct GraphqlHeaderInput {
    name: String,
    value: String,
}

#[derive(InputObject)]
#[graphql(rename_fields = "camelCase")]
struct ClusterRestCallInput {
    service: String,
    path: String,
    method: Option<String>,
    body: Option<Json<Value>>,
    headers: Option<Vec<GraphqlHeaderInput>>,
    forward_server_auth: Option<bool>,
    timeout_ms: Option<u64>,
}

#[derive(InputObject)]
#[graphql(rename_fields = "camelCase")]
struct ClusterGraphqlCallInput {
    service: String,
    path: Option<String>,
    query: String,
    operation_name: Option<String>,
    variables: Option<Json<Value>>,
    headers: Option<Vec<GraphqlHeaderInput>>,
    forward_server_auth: Option<bool>,
    timeout_ms: Option<u64>,
}

#[derive(SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct ClusterCallResponse {
    ok: bool,
    service: String,
    method: String,
    path: String,
    status: i32,
    content_type: Option<String>,
    body: String,
    json: Option<Json<Value>>,
}

pub fn router() -> Router {
    let schema = build_schema();
    Router::new()
        .route("/graphql", get(graphiql).post(graphql_handler))
        .route("/api/graphql", get(api_graphiql).post(graphql_handler))
        .route("/graphql/schema", get(graphql_schema_sdl))
        .route("/api/graphql/schema", get(graphql_schema_sdl))
        .layer(DefaultBodyLimit::max(graphql_request_body_limit_bytes()))
        .with_state(schema)
}

fn build_schema() -> RestApiSchema {
    let builder = Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .limit_depth(graphql_depth_limit())
        .limit_complexity(graphql_complexity_limit());
    if graphql_introspection_enabled() {
        builder.finish()
    } else {
        builder.disable_introspection().finish()
    }
}

async fn graphiql(headers: HeaderMap) -> AxumResponse {
    graphiql_for("/graphql", &headers)
}

async fn api_graphiql(headers: HeaderMap) -> AxumResponse {
    graphiql_for("/api/graphql", &headers)
}

fn graphiql_for(endpoint: &'static str, headers: &HeaderMap) -> AxumResponse {
    super::record_request("GET", endpoint, StatusCode::OK);
    if !graphql_ide_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if graphql_ide_auth_required() && !authorized_graphql_control(headers) {
        return super::unauthorized_response();
    }
    Html(
        async_graphql::http::GraphiQLSource::build()
            .endpoint(endpoint)
            .finish(),
    )
    .into_response()
}

async fn graphql_schema_sdl(
    State(schema): State<RestApiSchema>,
    headers: HeaderMap,
) -> AxumResponse {
    super::record_request("GET", "/graphql/schema", StatusCode::OK);
    if graphql_schema_auth_required() && !authorized_graphql_control(&headers) {
        return super::unauthorized_response();
    }
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        schema.sdl(),
    )
        .into_response()
}

async fn graphql_handler(
    State(schema): State<RestApiSchema>,
    headers: HeaderMap,
    AxumJson(request): AxumJson<async_graphql::Request>,
) -> AxumResponse {
    super::record_request("POST", "/graphql", StatusCode::OK);
    let request_context = RequestContext {
        authorized_control: authorized_graphql_control(&headers),
    };
    if graphql_auth_required() && !request_context.authorized_control {
        return super::unauthorized_response();
    }
    AxumJson(schema.execute(request.data(request_context)).await).into_response()
}

struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> GraphqlHealth {
        GraphqlHealth {
            ok: true,
            service: "dd-remote-rest-api".to_string(),
            mode: "database-boundary".to_string(),
            graphql: true,
        }
    }

    async fn capabilities(&self) -> Vec<GraphqlCapability> {
        vec![
            GraphqlCapability {
                name: "postgres".to_string(),
                configured: super::postgres_database_url().is_some(),
                enabled: true,
                notes: "Domain resolvers read the REST API's canonical Postgres contract."
                    .to_string(),
            },
            GraphqlCapability {
                name: "cockroachdb".to_string(),
                configured: cockroach_database_url().is_some(),
                enabled: true,
                notes:
                    "CockroachDB uses its Postgres-compatible wire protocol for direct resolvers."
                        .to_string(),
            },
            GraphqlCapability {
                name: "redis".to_string(),
                configured: redis_url().is_some(),
                enabled: true,
                notes: "Redis health is available; key reads require explicit opt-in and auth."
                    .to_string(),
            },
            GraphqlCapability {
                name: "cluster-subservices".to_string(),
                configured: !configured_subservices().is_empty(),
                enabled: service_calls_enabled(),
                notes: "REST/GraphQL subservice calls use a named allowlist, never arbitrary URLs."
                    .to_string(),
            },
        ]
    }

    async fn agents_snapshot(&self, limit: Option<i32>) -> GraphqlAgentsSnapshot {
        agents_snapshot_graphql(limit).await
    }

    async fn agent_threads(&self, limit: Option<i32>) -> Vec<GraphqlAgentThread> {
        agents_snapshot_graphql(limit).await.threads
    }

    async fn agent_tasks(&self, limit: Option<i32>) -> Vec<GraphqlAgentTask> {
        agents_snapshot_graphql(limit).await.tasks
    }

    async fn task_events(&self, task_id: String, limit: Option<i32>) -> GraphqlTaskEvents {
        let limit = limit.unwrap_or(100).clamp(1, 500) as i64;
        super::record_request("GRAPHQL", "taskEvents", StatusCode::OK);
        if super::postgres_database_url().is_some() {
            match super::fetch_agent_events_from_postgres(&task_id, limit).await {
                Ok(events) => {
                    return GraphqlTaskEvents {
                        ok: true,
                        source: "postgres".to_string(),
                        task_id,
                        generated_at_ms: super::now_ms().to_string(),
                        events: events.iter().map(GraphqlAgentEvent::from).collect(),
                        errors: Vec::new(),
                    };
                }
                Err(error) => {
                    return GraphqlTaskEvents {
                        ok: false,
                        source: "postgres".to_string(),
                        task_id,
                        generated_at_ms: super::now_ms().to_string(),
                        events: Vec::new(),
                        errors: graphql_backend_errors("postgres events", error),
                    };
                }
            }
        }
        GraphqlTaskEvents {
            ok: false,
            source: "postgres".to_string(),
            task_id,
            generated_at_ms: super::now_ms().to_string(),
            events: Vec::new(),
            errors: vec![
                "postgres database URL is not configured; task events are unavailable".to_string(),
            ],
        }
    }

    async fn thread_context(&self, thread_id: String, limit: Option<i32>) -> GraphqlThreadContext {
        let limit = limit.unwrap_or(20).clamp(1, 100) as i64;
        super::record_request("GRAPHQL", "threadContext", StatusCode::OK);
        let response = if super::postgres_database_url().is_some() {
            match super::fetch_thread_context_from_postgres(&thread_id, limit).await {
                Ok(tasks) => super::ThreadContextResponse {
                    ok: true,
                    source: "postgres".to_string(),
                    thread_id,
                    generated_at_ms: super::now_ms(),
                    tasks,
                    errors: Vec::new(),
                },
                Err(error) => super::runtime_thread_context(
                    &thread_id,
                    limit,
                    graphql_backend_errors("postgres", error),
                ),
            }
        } else {
            super::runtime_thread_context(
                &thread_id,
                limit,
                vec![
                    "postgres database URL is not configured; showing runtime memory only"
                        .to_string(),
                ],
            )
        };
        response.into()
    }

    async fn known_git_repos(&self, limit: Option<i32>) -> GraphqlKnownGitRepos {
        let limit = limit.unwrap_or(50).clamp(1, 200) as i64;
        super::record_request("GRAPHQL", "knownGitRepos", StatusCode::OK);
        if super::postgres_database_url().is_none() {
            return GraphqlKnownGitRepos {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                repos: Vec::new(),
                errors: vec!["postgres database URL is not configured".to_string()],
            };
        }
        match super::fetch_known_git_repos_from_postgres(limit).await {
            Ok(repos) => GraphqlKnownGitRepos {
                ok: true,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                repos: repos.iter().map(GraphqlKnownGitRepo::from).collect(),
                errors: Vec::new(),
            },
            Err(error) => GraphqlKnownGitRepos {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                repos: Vec::new(),
                errors: graphql_backend_errors("postgres", error),
            },
        }
    }

    async fn lambda_functions(
        &self,
        limit: Option<i32>,
        search: Option<String>,
    ) -> GraphqlLambdaFunctions {
        let limit = limit.unwrap_or(100).clamp(1, 250) as i64;
        let query = super::LambdasQuery {
            limit: Some(limit),
            search,
        };
        super::record_request("GRAPHQL", "lambdaFunctions", StatusCode::OK);
        if super::postgres_database_url().is_none() {
            return GraphqlLambdaFunctions {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                functions: Vec::new(),
                errors: vec!["postgres database URL is not configured".to_string()],
            };
        }
        match super::fetch_lambda_functions_from_postgres(
            super::lambda_limit_from_query(&query),
            &super::lambda_search_pattern(&query),
        )
        .await
        {
            Ok(functions) => GraphqlLambdaFunctions {
                ok: true,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                functions: functions.iter().map(GraphqlLambdaFunction::from).collect(),
                errors: Vec::new(),
            },
            Err(error) => GraphqlLambdaFunctions {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: super::now_ms().to_string(),
                functions: Vec::new(),
                errors: graphql_backend_errors("postgres lambda functions", error),
            },
        }
    }

    async fn lambda_function(&self, id_or_slug: String) -> Result<Option<GraphqlLambdaFunction>> {
        super::record_request("GRAPHQL", "lambdaFunction", StatusCode::OK);
        if super::postgres_database_url().is_none() {
            return Err(Error::new("postgres database URL is not configured"));
        }
        match super::fetch_lambda_function_by_identifier(&id_or_slug).await {
            Ok(function) => Ok(Some((&function).into())),
            Err(error) if error.contains("query returned no rows") => Ok(None),
            Err(error) => Err(graphql_backend_error("postgres lambda function", error)),
        }
    }

    async fn data_sources(&self) -> Vec<GraphqlDataSourceStatus> {
        vec![
            postgres_status().await,
            cockroach_status().await,
            redis_status().await,
        ]
    }

    async fn postgres_status(&self) -> GraphqlDataSourceStatus {
        postgres_status().await
    }

    async fn cockroach_status(&self) -> GraphqlDataSourceStatus {
        cockroach_status().await
    }

    async fn redis_status(&self) -> GraphqlDataSourceStatus {
        redis_status().await
    }

    async fn redis_get(&self, ctx: &Context<'_>, key: String) -> Result<GraphqlRedisValue> {
        require_control_auth(ctx)?;
        if !super::env_bool("REST_API_GRAPHQL_REDIS_READS_ENABLED", false) {
            return Err(Error::new(
                "Redis key reads are disabled; set REST_API_GRAPHQL_REDIS_READS_ENABLED=true",
            ));
        }
        validate_redis_key(&key)?;
        let Some(url) = redis_url() else {
            return Ok(GraphqlRedisValue {
                ok: false,
                key,
                value: None,
                message: Some("redis URL is not configured".to_string()),
            });
        };
        let client = redis::Client::open(url).map_err(|error| Error::new(error.to_string()))?;
        let lookup_key = key.clone();
        let value = with_timeout(redis_timeout_ms(), async move {
            let mut connection = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|error| error.to_string())?;
            let value: Option<String> = connection
                .get(&lookup_key)
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(value)
        })
        .await?;
        Ok(GraphqlRedisValue {
            ok: true,
            key,
            value,
            message: None,
        })
    }

    async fn subservices(&self) -> Vec<GraphqlSubservice> {
        configured_subservices()
            .into_iter()
            .map(|(name, base_url)| GraphqlSubservice { name, base_url })
            .collect()
    }
}

struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn call_cluster_rest(
        &self,
        ctx: &Context<'_>,
        input: ClusterRestCallInput,
    ) -> Result<ClusterCallResponse> {
        require_control_auth(ctx)?;
        ensure_service_calls_enabled()?;
        let service = normalize_service_alias(&input.service)?;
        let base_url = resolve_subservice_base_url(&service)?;
        let path = validate_cluster_path(&input.path)?;
        let method = validate_rest_method(input.method.as_deref().unwrap_or("GET"))?;
        let timeout_ms = input
            .timeout_ms
            .unwrap_or_else(subservice_timeout_ms)
            .clamp(100, 30_000);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|error| Error::new(error.to_string()))?;
        let url = format!("{}{}", base_url.trim_end_matches('/'), path);
        let mut request = client.request(method.clone(), url);
        request = apply_graphql_headers(request, input.headers.as_deref())?;
        if input.forward_server_auth.unwrap_or(false) {
            request = apply_worker_auth(request)?;
        }
        if let Some(body) = input.body {
            validate_json_payload_bytes(
                "REST subservice body",
                &body.0,
                subservice_request_body_limit_bytes(),
            )?;
            request = request.json(&body.0);
        }
        let response = request
            .send()
            .await
            .map_err(|error| Error::new(error.to_string()))?;
        cluster_call_response(service, method.as_str(), path, response).await
    }

    async fn call_cluster_graphql(
        &self,
        ctx: &Context<'_>,
        input: ClusterGraphqlCallInput,
    ) -> Result<ClusterCallResponse> {
        require_control_auth(ctx)?;
        ensure_service_calls_enabled()?;
        let service = normalize_service_alias(&input.service)?;
        let base_url = resolve_subservice_base_url(&service)?;
        let path = validate_cluster_path(input.path.as_deref().unwrap_or("/graphql"))?;
        validate_subservice_graphql_query(&input.query)?;
        let timeout_ms = input
            .timeout_ms
            .unwrap_or_else(subservice_timeout_ms)
            .clamp(100, 30_000);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|error| Error::new(error.to_string()))?;
        let url = format!("{}{}", base_url.trim_end_matches('/'), path);
        let mut body = json!({ "query": input.query });
        if let Some(operation_name) = input.operation_name {
            body["operationName"] =
                Value::String(validate_graphql_operation_name(&operation_name)?);
        }
        if let Some(variables) = input.variables {
            body["variables"] = variables.0;
        }
        validate_json_payload_bytes(
            "GraphQL subservice request",
            &body,
            subservice_request_body_limit_bytes(),
        )?;
        let mut request = client.post(url).json(&body);
        request = apply_graphql_headers(request, input.headers.as_deref())?;
        if input.forward_server_auth.unwrap_or(false) {
            request = apply_worker_auth(request)?;
        }
        let response = request
            .send()
            .await
            .map_err(|error| Error::new(error.to_string()))?;
        cluster_call_response(service, "POST", path, response).await
    }
}

impl From<super::AgentsSnapshot> for GraphqlAgentsSnapshot {
    fn from(value: super::AgentsSnapshot) -> Self {
        GraphqlAgentsSnapshot {
            ok: value.ok,
            source: value.source,
            generated_at_ms: value.generated_at_ms.to_string(),
            config: value.config.into(),
            summary: value.summary.into(),
            threads: value.threads.iter().map(GraphqlAgentThread::from).collect(),
            tasks: value.tasks.iter().map(GraphqlAgentTask::from).collect(),
            errors: value.errors,
        }
    }
}

impl From<super::AgentsDataConfig> for GraphqlAgentsDataConfig {
    fn from(value: super::AgentsDataConfig) -> Self {
        GraphqlAgentsDataConfig {
            rds_configured: value.rds_configured,
            postgres_configured: value.postgres_configured,
            supabase_configured: value.supabase_configured,
            nats_configured: value.nats_configured,
            nats_url: value.nats_url,
            postgres_plan: value.postgres_plan,
        }
    }
}

impl From<super::AgentsSummary> for GraphqlAgentsSummary {
    fn from(value: super::AgentsSummary) -> Self {
        GraphqlAgentsSummary {
            thread_count: saturated_i32(value.thread_count),
            task_count: saturated_i32(value.task_count),
            running_count: saturated_i32(value.running_count),
            failed_count: saturated_i32(value.failed_count),
            done_count: saturated_i32(value.done_count),
            pr_count: saturated_i32(value.pr_count),
        }
    }
}

impl From<&super::AgentThreadRow> for GraphqlAgentThread {
    fn from(value: &super::AgentThreadRow) -> Self {
        GraphqlAgentThread {
            id: value.id.clone(),
            title: value.title.clone(),
            repo: value.repo.clone(),
            base_branch: value.base_branch.clone(),
            archived_at: value.archived_at.clone(),
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
            task_count: value.task_count,
            active_task_count: value.active_task_count,
            latest_task_at: value.latest_task_at.clone(),
        }
    }
}

impl From<&super::AgentTaskRow> for GraphqlAgentTask {
    fn from(value: &super::AgentTaskRow) -> Self {
        GraphqlAgentTask {
            id: value.id.clone(),
            thread_id: value.thread_id.clone(),
            thread_title: value.thread_title.clone(),
            prompt: value.prompt.clone(),
            status: value.status.clone(),
            branch: value.branch.clone(),
            pr_url: value.pr_url.clone(),
            pr_state: value.pr_state.clone(),
            exit_reason: value.exit_reason.clone(),
            error_message: value.error_message.clone(),
            started_at: value.started_at.clone(),
            finished_at: value.finished_at.clone(),
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
            last_event_seq: value.last_event_seq,
            event_count: value.event_count,
            latest_event_kind: value.latest_event_kind.clone(),
            latest_payload: value.latest_payload.clone(),
        }
    }
}

impl From<&super::AgentEventRow> for GraphqlAgentEvent {
    fn from(value: &super::AgentEventRow) -> Self {
        GraphqlAgentEvent {
            task_id: value.task_id.clone(),
            seq: value.seq,
            event_kind: value.event_kind.clone(),
            payload: Json(value.payload.clone()),
            created_at: value.created_at.clone(),
        }
    }
}

impl From<super::ThreadContextResponse> for GraphqlThreadContext {
    fn from(value: super::ThreadContextResponse) -> Self {
        GraphqlThreadContext {
            ok: value.ok,
            source: value.source,
            thread_id: value.thread_id,
            generated_at_ms: value.generated_at_ms.to_string(),
            tasks: value.tasks.iter().map(GraphqlAgentTask::from).collect(),
            errors: value.errors,
        }
    }
}

impl From<&super::KnownGitRepoRow> for GraphqlKnownGitRepo {
    fn from(value: &super::KnownGitRepoRow) -> Self {
        GraphqlKnownGitRepo {
            id: value.id.clone(),
            repo_url: value.repo_url.clone(),
            display_name: value.display_name.clone(),
            provider: value.provider.clone(),
            default_branch: value.default_branch.clone(),
            status: value.status.clone(),
            last_verified_at: value.last_verified_at.clone(),
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

impl From<&super::LambdaFunctionRow> for GraphqlLambdaFunction {
    fn from(value: &super::LambdaFunctionRow) -> Self {
        GraphqlLambdaFunction {
            id: value.id.clone(),
            slug: value.slug.clone(),
            display_name: value.display_name.clone(),
            description: value.description.clone(),
            runtime: value.runtime.clone(),
            entry_command: value.entry_command.clone(),
            function_body: value.function_body.clone(),
            reuse_key: value.reuse_key.clone(),
            idle_timeout_seconds: value.idle_timeout_seconds,
            max_run_ms: value.max_run_ms,
            containerized: value.containerized,
            container_image: value.container_image.clone(),
            container_build_status: value.container_build_status.clone(),
            container_build_error: value.container_build_error.clone(),
            container_built_at: value.container_built_at.clone(),
            status: value.status.clone(),
            labels: Json(value.labels.clone()),
            meta_data: Json(value.meta_data.clone()),
            last_invoked_at: value.last_invoked_at.clone(),
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

async fn agents_snapshot_graphql(limit: Option<i32>) -> GraphqlAgentsSnapshot {
    let limit = limit.unwrap_or(50).clamp(1, 200) as i64;
    super::record_request("GRAPHQL", "agentsSnapshot", StatusCode::OK);
    super::fetch_agents_snapshot(limit).await.into()
}

async fn postgres_status() -> GraphqlDataSourceStatus {
    let configured = super::postgres_database_url().is_some();
    if !configured {
        return GraphqlDataSourceStatus {
            name: "postgres".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some("postgres database URL is not configured".to_string()),
        };
    }
    match database_version_with_url(super::postgres_database_url().as_deref()).await {
        Ok(version) => GraphqlDataSourceStatus {
            name: "postgres".to_string(),
            configured,
            ok: true,
            version: Some(version),
            message: None,
        },
        Err(error) => GraphqlDataSourceStatus {
            name: "postgres".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some(error),
        },
    }
}

async fn cockroach_status() -> GraphqlDataSourceStatus {
    let configured = cockroach_database_url().is_some();
    if !configured {
        return GraphqlDataSourceStatus {
            name: "cockroachdb".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some("CockroachDB URL is not configured".to_string()),
        };
    }
    match database_version_with_url(cockroach_database_url().as_deref()).await {
        Ok(version) => GraphqlDataSourceStatus {
            name: "cockroachdb".to_string(),
            configured,
            ok: true,
            version: Some(version),
            message: None,
        },
        Err(error) => GraphqlDataSourceStatus {
            name: "cockroachdb".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some(error),
        },
    }
}

async fn database_version_with_url(database_url: Option<&str>) -> Result<String, String> {
    let Some(database_url) = database_url else {
        return Err("database URL is not configured".to_string());
    };
    with_timeout(database_timeout_ms(), async move {
        let client = super::connect_postgres_with_url(database_url).await?;
        let row = client
            .query_one("select version() as version", &[])
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(super::row_string(&row, "version"))
    })
    .await
    .map_err(|error| format!("database status check failed: {error}"))
}

async fn redis_status() -> GraphqlDataSourceStatus {
    let configured = redis_url().is_some();
    let Some(url) = redis_url() else {
        return GraphqlDataSourceStatus {
            name: "redis".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some("redis URL is not configured".to_string()),
        };
    };
    let result = with_timeout(redis_timeout_ms(), async move {
        let client = redis::Client::open(url).map_err(|error| error.to_string())?;
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| error.to_string())?;
        let pong: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(pong)
    })
    .await;
    match result {
        Ok(pong) => GraphqlDataSourceStatus {
            name: "redis".to_string(),
            configured,
            ok: true,
            version: None,
            message: Some(pong),
        },
        Err(error) => GraphqlDataSourceStatus {
            name: "redis".to_string(),
            configured,
            ok: false,
            version: None,
            message: Some(format!("redis status check failed: {error}")),
        },
    }
}

async fn with_timeout<T, F>(timeout_ms: u64, future: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    tokio::time::timeout(Duration::from_millis(timeout_ms), future)
        .await
        .map_err(|_| format!("timed out after {timeout_ms} ms"))?
}

fn database_timeout_ms() -> u64 {
    super::env_u64("REST_API_GRAPHQL_DATABASE_TIMEOUT_MS", 2_000).clamp(100, 30_000)
}

fn redis_timeout_ms() -> u64 {
    super::env_u64("REST_API_GRAPHQL_REDIS_TIMEOUT_MS", 1_000).clamp(100, 30_000)
}

fn subservice_timeout_ms() -> u64 {
    super::env_u64("REST_API_GRAPHQL_SUBSERVICE_TIMEOUT_MS", 3_000).clamp(100, 30_000)
}

fn graphql_request_body_limit_bytes() -> usize {
    super::env_usize("REST_API_GRAPHQL_REQUEST_BYTES", 262_144).clamp(4_096, 2_097_152)
}

fn graphql_depth_limit() -> usize {
    super::env_usize("REST_API_GRAPHQL_DEPTH_LIMIT", 12).clamp(2, 64)
}

fn graphql_complexity_limit() -> usize {
    super::env_usize("REST_API_GRAPHQL_COMPLEXITY_LIMIT", 250).clamp(10, 10_000)
}

fn graphql_auth_required() -> bool {
    super::env_bool("REST_API_GRAPHQL_AUTH_REQUIRED", true)
}

fn graphql_schema_auth_required() -> bool {
    super::env_bool("REST_API_GRAPHQL_SCHEMA_AUTH_REQUIRED", true)
}

fn graphql_ide_enabled() -> bool {
    super::env_bool("REST_API_GRAPHQL_IDE_ENABLED", false)
}

fn graphql_ide_auth_required() -> bool {
    super::env_bool("REST_API_GRAPHQL_IDE_AUTH_REQUIRED", true)
}

fn graphql_introspection_enabled() -> bool {
    super::env_bool("REST_API_GRAPHQL_INTROSPECTION_ENABLED", false)
}

fn graphql_backend_errors(source: &str, error: String) -> Vec<String> {
    let mut errors = vec![super::public_data_source_error(source)];
    if super::env_bool("REST_API_GRAPHQL_EXPOSE_BACKEND_ERRORS", false) {
        errors.push(error);
    }
    errors
}

fn graphql_backend_error(source: &str, error: String) -> Error {
    let public_error = super::public_data_source_error(source);
    if super::env_bool("REST_API_GRAPHQL_EXPOSE_BACKEND_ERRORS", false) {
        Error::new(public_error).extend_with(|_, e| e.set("detail", error))
    } else {
        Error::new(public_error)
    }
}

fn subservice_request_body_limit_bytes() -> usize {
    super::env_usize("REST_API_GRAPHQL_SUBSERVICE_REQUEST_BYTES", 262_144).clamp(1_024, 2_097_152)
}

fn subservice_response_body_limit_bytes() -> usize {
    super::env_usize("REST_API_GRAPHQL_SUBSERVICE_RESPONSE_BYTES", 262_144).clamp(1_024, 2_097_152)
}

fn cockroach_database_url() -> Option<String> {
    super::first_env(&[
        "COCKROACH_DATABASE_URL",
        "COCKROACHDB_DATABASE_URL",
        "CRDB_DATABASE_URL",
    ])
}

fn redis_url() -> Option<String> {
    super::first_env(&["REDIS_URL", "REDIS_CACHE_URL", "DD_REDIS_CACHE_URL"]).or_else(|| {
        if super::env_bool("REST_API_GRAPHQL_REDIS_CLUSTER_DEFAULT_ENABLED", false) {
            Some("redis://dd-redis-cache.default.svc.cluster.local:6379".to_string())
        } else {
            None
        }
    })
}

fn configured_subservices() -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(raw) = super::first_env(&["REST_API_GRAPHQL_SERVICE_ALLOWLIST"]) {
        for item in raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if let Some((alias, url)) = item.split_once('=') {
                if let (Ok(alias), Ok(url)) = (
                    normalize_service_alias(alias),
                    normalize_subservice_base_url(url),
                ) {
                    out.insert(alias, url);
                }
                continue;
            }
            if let Ok(alias) = normalize_service_alias(item) {
                if let Some(url) = subservice_url_from_env(&alias) {
                    out.insert(alias, url);
                }
            }
        }
    }
    if let Some(url) = super::first_env(&["RUNTIME_CONFIG_BASE_URL"]) {
        if let Ok(url) = normalize_subservice_base_url(&url) {
            out.entry("runtime-config".to_string()).or_insert(url);
        }
    }
    out
}

fn service_calls_enabled() -> bool {
    super::env_bool("REST_API_GRAPHQL_SERVICE_CALLS_ENABLED", false)
}

fn ensure_service_calls_enabled() -> Result<()> {
    if service_calls_enabled() {
        Ok(())
    } else {
        Err(Error::new(
            "cluster subservice calls are disabled; set REST_API_GRAPHQL_SERVICE_CALLS_ENABLED=true",
        ))
    }
}

fn resolve_subservice_base_url(service: &str) -> Result<String> {
    configured_subservices()
        .get(service)
        .cloned()
        .ok_or_else(|| {
            Error::new(format!(
                "service '{service}' is not in the GraphQL allowlist"
            ))
        })
}

fn subservice_url_from_env(alias: &str) -> Option<String> {
    let env_key = format!("REST_API_GRAPHQL_SERVICE_{}_URL", service_env_key(alias));
    super::first_env(&[&env_key]).and_then(|value| normalize_subservice_base_url(&value).ok())
}

fn normalize_subservice_base_url(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.len() > 2_048 {
        return Err(Error::new("service URL must be 1-2048 characters"));
    }
    let mut url = reqwest::Url::parse(trimmed)
        .map_err(|_| Error::new("service URL must be a valid absolute http(s) URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::new("service URL must use http or https"));
    }
    if url.host_str().is_none() {
        return Err(Error::new("service URL must include a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::new("service URL must not include user info"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::new(
            "service URL must not include a query string or fragment",
        ));
    }
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(&normalized_path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn service_env_key(alias: &str) -> String {
    alias
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn normalize_service_alias(input: &str) -> Result<String> {
    let alias = input.trim().to_lowercase();
    if alias.is_empty() || alias.len() > 80 {
        return Err(Error::new("service must be 1-80 characters"));
    }
    if !alias
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(Error::new(
            "service may contain only ASCII letters, numbers, '-' and '_'",
        ));
    }
    Ok(alias)
}

fn validate_cluster_path(input: &str) -> Result<String> {
    let path = input.trim();
    if !path.starts_with('/') || path.starts_with("//") || path.contains("://") {
        return Err(Error::new(
            "path must be a relative absolute path like /healthz",
        ));
    }
    if path.contains("..")
        || path.contains('\\')
        || path.contains('#')
        || path.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(Error::new(
            "path contains unsupported traversal, fragment, whitespace, or control characters",
        ));
    }
    if path.len() > 1024 {
        return Err(Error::new("path must be 1024 characters or fewer"));
    }
    Ok(path.to_string())
}

fn validate_rest_method(input: &str) -> Result<reqwest::Method> {
    let method = input.trim().to_ascii_uppercase();
    let allowed = super::first_env(&["REST_API_GRAPHQL_REST_METHOD_ALLOWLIST"])
        .unwrap_or_else(|| "GET,POST".to_string())
        .split(',')
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    if !allowed.contains(&method) {
        return Err(Error::new(format!(
            "method {method} is not allowed by REST_API_GRAPHQL_REST_METHOD_ALLOWLIST"
        )));
    }
    reqwest::Method::from_bytes(method.as_bytes()).map_err(|error| Error::new(error.to_string()))
}

fn validate_subservice_graphql_query(query: &str) -> Result<()> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(Error::new("query is required"));
    }
    let max_bytes =
        super::env_usize("REST_API_GRAPHQL_SUBSERVICE_QUERY_BYTES", 65_536).clamp(1_024, 262_144);
    if query.len() > max_bytes {
        return Err(Error::new(format!(
            "query must be {max_bytes} bytes or fewer"
        )));
    }
    if query
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(Error::new("query contains unsupported control characters"));
    }
    Ok(())
}

fn validate_graphql_operation_name(input: &str) -> Result<String> {
    let value = input.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(Error::new("operationName must be 1-128 characters"));
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(Error::new("operationName is required"));
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(Error::new(
            "operationName must start with '_' or an ASCII letter",
        ));
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return Err(Error::new(
            "operationName may contain only ASCII letters, numbers, and '_'",
        ));
    }
    Ok(value.to_string())
}

fn validate_json_payload_bytes(label: &str, value: &Value, max_bytes: usize) -> Result<()> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| Error::new(format!("failed to encode {label}: {error}")))?;
    if encoded.len() > max_bytes {
        return Err(Error::new(format!(
            "{label} must be {max_bytes} bytes or fewer"
        )));
    }
    Ok(())
}

fn apply_graphql_headers(
    mut request: reqwest::RequestBuilder,
    headers: Option<&[GraphqlHeaderInput]>,
) -> Result<reqwest::RequestBuilder> {
    let Some(headers) = headers else {
        return Ok(request);
    };
    if headers.len() > 16 {
        return Err(Error::new("at most 16 forwarded headers are allowed"));
    }
    for header_input in headers {
        let name = header_input.name.trim().to_ascii_lowercase();
        if name.is_empty() || name.len() > 64 {
            return Err(Error::new("header names must be 1-64 characters"));
        }
        if matches!(
            name.as_str(),
            "authorization"
                | "auth"
                | "x-server-auth"
                | "x-agent-auth"
                | "cookie"
                | "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "upgrade"
                | "proxy-authorization"
        ) {
            return Err(Error::new(format!(
                "header '{name}' is managed by the REST API"
            )));
        }
        if name.starts_with("x-forwarded-") || name.starts_with("proxy-") {
            return Err(Error::new(format!(
                "header '{name}' is managed by the REST API"
            )));
        }
        if header_input.value.len() > 4_096
            || header_input
                .value
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\t'))
        {
            return Err(Error::new(
                "header values must be 4096 bytes or fewer and contain no control characters",
            ));
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| Error::new(error.to_string()))?;
        let header_value = HeaderValue::from_str(&header_input.value)
            .map_err(|error| Error::new(error.to_string()))?;
        request = request.header(header_name, header_value);
    }
    Ok(request)
}

fn apply_worker_auth(request: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
    let secret = super::worker_auth_secret()
        .ok_or_else(|| Error::new(super::missing_worker_auth_secret_message()))?;
    Ok(request.header("X-Server-Auth", secret))
}

async fn cluster_call_response(
    service: String,
    method: &str,
    path: String,
    response: reqwest::Response,
) -> Result<ClusterCallResponse> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|error| Error::new(error.to_string()))?;
    let max_bytes = subservice_response_body_limit_bytes();
    let body_bytes = if bytes.len() > max_bytes {
        &bytes[..max_bytes]
    } else {
        bytes.as_ref()
    };
    let mut body = String::from_utf8_lossy(body_bytes).to_string();
    if bytes.len() > max_bytes {
        body.push_str("\n[truncated]");
    }
    let parsed_json = serde_json::from_slice::<Value>(body_bytes).ok().map(Json);
    Ok(ClusterCallResponse {
        ok: status.is_success(),
        service,
        method: method.to_string(),
        path,
        status: status.as_u16() as i32,
        content_type,
        body,
        json: parsed_json,
    })
}

fn authorized_graphql_control(headers: &HeaderMap) -> bool {
    if super::authorized_internal_request(headers) {
        return true;
    }
    let Some(expected) = super::worker_auth_secret() else {
        return false;
    };
    headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn require_control_auth(ctx: &Context<'_>) -> Result<()> {
    let authorized = ctx
        .data_opt::<RequestContext>()
        .map(|context| context.authorized_control)
        .unwrap_or(false);
    if authorized {
        Ok(())
    } else {
        Err(Error::new("missing required dd internal auth header"))
    }
}

fn validate_redis_key(key: &str) -> Result<()> {
    if key.trim().is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(Error::new("redis key must be 1-512 non-control characters"));
    }
    let prefixes = super::first_env(&["REST_API_GRAPHQL_REDIS_KEY_PREFIXES"]).unwrap_or_default();
    let allowed = prefixes
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if allowed
        .iter()
        .any(|prefix| *prefix == "*" || key.starts_with(prefix))
    {
        Ok(())
    } else {
        Err(Error::new(
            "redis key is outside REST_API_GRAPHQL_REDIS_KEY_PREFIXES",
        ))
    }
}

fn saturated_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_subservice_urls_as_paths() {
        assert!(validate_cluster_path("https://example.com/healthz").is_err());
        assert!(validate_cluster_path("//example.com/healthz").is_err());
        assert!(validate_cluster_path("/../secret").is_err());
        assert!(validate_cluster_path("/healthz#fragment").is_err());
        assert!(validate_cluster_path("/healthz bad").is_err());
        assert!(validate_cluster_path("/healthz").is_ok());
    }

    #[test]
    fn normalizes_service_alias_for_env_lookup() {
        assert_eq!(
            normalize_service_alias("Runtime-Config").unwrap(),
            "runtime-config"
        );
        assert_eq!(service_env_key("runtime-config"), "RUNTIME_CONFIG");
        assert!(normalize_service_alias("bad/service").is_err());
    }

    #[test]
    fn normalizes_subservice_base_urls() {
        assert_eq!(
            normalize_subservice_base_url("http://service.default.svc.cluster.local:8080/")
                .unwrap(),
            "http://service.default.svc.cluster.local:8080"
        );
        assert_eq!(
            normalize_subservice_base_url("https://service.default.svc/api/").unwrap(),
            "https://service.default.svc/api"
        );
        assert!(normalize_subservice_base_url("file:///tmp/socket").is_err());
        assert!(normalize_subservice_base_url("https://user:pass@example.com").is_err());
        assert!(normalize_subservice_base_url("https://example.com?token=1").is_err());
    }

    #[test]
    fn validates_subservice_graphql_inputs() {
        assert!(validate_subservice_graphql_query("query { health { ok } }").is_ok());
        assert!(validate_subservice_graphql_query("").is_err());
        assert!(validate_subservice_graphql_query("query\u{0000}").is_err());
        assert_eq!(
            validate_graphql_operation_name(" HealthCheck ").unwrap(),
            "HealthCheck"
        );
        assert!(validate_graphql_operation_name("1Bad").is_err());
        assert!(validate_graphql_operation_name("bad-name").is_err());
    }

    #[test]
    fn rejects_managed_or_unsafe_forwarded_headers() {
        let request = reqwest::Client::new().get("http://service.local/healthz");
        assert!(apply_graphql_headers(
            request,
            Some(&[GraphqlHeaderInput {
                name: "Authorization".to_string(),
                value: "secret".to_string(),
            }])
        )
        .is_err());

        let request = reqwest::Client::new().get("http://service.local/healthz");
        assert!(apply_graphql_headers(
            request,
            Some(&[GraphqlHeaderInput {
                name: "X-Forwarded-Host".to_string(),
                value: "example.com".to_string(),
            }])
        )
        .is_err());

        let request = reqwest::Client::new().get("http://service.local/healthz");
        assert!(apply_graphql_headers(
            request,
            Some(&[GraphqlHeaderInput {
                name: "X-Debug".to_string(),
                value: "ok".to_string(),
            }])
        )
        .is_ok());
    }
}
