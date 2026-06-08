use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, now_ms};

const MAX_CONNECTIONS: usize = 128;
const MAX_TAGS: usize = 24;
const MAX_SETTINGS: usize = 48;
const MAX_SETTING_VALUE_BYTES: usize = 512;
const MAX_ALLOWED_SCHEMAS: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveConnectionRequest {
    pub connection_id: String,
    pub name: String,
    pub engine: ConnectionEngine,
    pub mode: Option<ConnectionMode>,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub secret_ref: Option<String>,
    pub endpoint: Option<ConnectionEndpoint>,
    pub default_database: Option<String>,
    pub default_schema: Option<String>,
    pub allowed_schemas: Option<Vec<String>>,
    pub settings: Option<BTreeMap<String, String>>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConnectionEngine {
    Postgres,
    Mysql,
    #[serde(rename = "bigquery")]
    BigQuery,
    Snowflake,
    Redshift,
    Prometheus,
    Loki,
    Parquet,
    CsvJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConnectionMode {
    LiveQuery,
    Import,
    MetadataOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionEndpoint {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub region: Option<String>,
    pub project_id: Option<String>,
    pub warehouse: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataConnection {
    pub connection_id: String,
    pub name: String,
    pub engine: ConnectionEngine,
    pub mode: ConnectionMode,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub secret_ref: Option<String>,
    pub endpoint: Option<ConnectionEndpoint>,
    pub default_database: Option<String>,
    pub default_schema: Option<String>,
    pub allowed_schemas: Vec<String>,
    pub settings: BTreeMap<String, String>,
    pub disabled: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataConnectionSummary {
    connection_id: String,
    name: String,
    engine: ConnectionEngine,
    mode: ConnectionMode,
    owner: Option<String>,
    tag_count: usize,
    secret_ref_configured: bool,
    endpoint_configured: bool,
    disabled: bool,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveConnectionResponse {
    ok: bool,
    connection: DataConnection,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionTestPlanResponse {
    ok: bool,
    schema_version: &'static str,
    generated_at_ms: u128,
    connection_id: String,
    engine: ConnectionEngine,
    mode: ConnectionMode,
    disabled: bool,
    checks: Vec<ConnectionCheck>,
    planner_notes: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionCheck {
    id: &'static str,
    status: CheckStatus,
    description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CheckStatus {
    Ready,
    Warning,
    Skipped,
}

impl SaveConnectionRequest {
    pub(crate) fn into_connection(self, now_ms: u128) -> Result<DataConnection, String> {
        let connection_id = clean_identifier(&self.connection_id).ok_or_else(|| {
            "connectionId must contain letters, numbers, dash, underscore, dot, or colon"
                .to_string()
        })?;
        let name = bounded_label("connection name", &self.name, 160)?;
        let owner = self
            .owner
            .as_deref()
            .map(|owner| bounded_label("connection owner", owner, 120))
            .transpose()?;
        let mode = self.mode.unwrap_or_else(|| self.engine.default_mode());
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        let secret_ref = self
            .secret_ref
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "connection secretRef must be a safe identifier".to_string())
            })
            .transpose()?;
        if self.engine.requires_secret_ref() && secret_ref.is_none() {
            return Err(format!(
                "{} connections require secretRef instead of inline credentials",
                self.engine.label()
            ));
        }
        let endpoint = self
            .endpoint
            .map(|endpoint| endpoint.normalized(self.engine))
            .transpose()?;
        if self.engine.requires_endpoint() && endpoint.is_none() {
            return Err(format!(
                "{} connections require endpoint metadata",
                self.engine.label()
            ));
        }
        let default_database =
            normalize_optional_identifier(self.default_database, "defaultDatabase")?;
        let default_schema = normalize_optional_identifier(self.default_schema, "defaultSchema")?;
        let allowed_schemas = normalize_identifier_vec(
            self.allowed_schemas.unwrap_or_default(),
            "allowedSchemas",
            MAX_ALLOWED_SCHEMAS,
        )?;
        let settings = normalize_settings(self.settings.unwrap_or_default())?;
        Ok(DataConnection {
            connection_id,
            name,
            engine: self.engine,
            mode,
            owner,
            tags,
            secret_ref,
            endpoint,
            default_database,
            default_schema,
            allowed_schemas,
            settings,
            disabled: self.disabled.unwrap_or(false),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl DataConnection {
    pub(crate) fn summary(&self) -> DataConnectionSummary {
        DataConnectionSummary {
            connection_id: self.connection_id.clone(),
            name: self.name.clone(),
            engine: self.engine,
            mode: self.mode,
            owner: self.owner.clone(),
            tag_count: self.tags.len(),
            secret_ref_configured: self.secret_ref.is_some(),
            endpoint_configured: self.endpoint.is_some(),
            disabled: self.disabled,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

impl ConnectionEndpoint {
    fn normalized(self, engine: ConnectionEngine) -> Result<Self, String> {
        let host = self
            .host
            .as_deref()
            .map(|value| bounded_host(value))
            .transpose()?;
        let region = self
            .region
            .as_deref()
            .map(|value| bounded_label("connection region", value, 80))
            .transpose()?;
        let project_id = self
            .project_id
            .as_deref()
            .map(|value| bounded_label("connection projectId", value, 120))
            .transpose()?;
        let warehouse = self
            .warehouse
            .as_deref()
            .map(|value| bounded_label("connection warehouse", value, 120))
            .transpose()?;
        if matches!(
            engine,
            ConnectionEngine::Postgres
                | ConnectionEngine::Mysql
                | ConnectionEngine::Redshift
                | ConnectionEngine::Prometheus
                | ConnectionEngine::Loki
        ) && host.is_none()
        {
            return Err(format!("{} endpoint requires host", engine.label()));
        }
        if engine == ConnectionEngine::BigQuery && project_id.is_none() {
            return Err("bigquery endpoint requires projectId".to_string());
        }
        if engine == ConnectionEngine::Snowflake && warehouse.is_none() {
            return Err("snowflake endpoint requires warehouse".to_string());
        }
        Ok(Self {
            host,
            port: self.port,
            region,
            project_id,
            warehouse,
        })
    }
}

impl ConnectionEngine {
    fn label(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::BigQuery => "bigquery",
            Self::Snowflake => "snowflake",
            Self::Redshift => "redshift",
            Self::Prometheus => "prometheus",
            Self::Loki => "loki",
            Self::Parquet => "parquet",
            Self::CsvJson => "csv-json",
        }
    }

    fn default_mode(self) -> ConnectionMode {
        match self {
            Self::Parquet | Self::CsvJson => ConnectionMode::Import,
            _ => ConnectionMode::LiveQuery,
        }
    }

    fn requires_secret_ref(self) -> bool {
        !matches!(self, Self::Parquet | Self::CsvJson)
    }

    fn requires_endpoint(self) -> bool {
        !matches!(self, Self::Parquet | Self::CsvJson)
    }

    fn sql_dialect(self) -> &'static str {
        match self {
            Self::Postgres => "postgresql",
            Self::Mysql => "mysql",
            Self::BigQuery => "bigquery-standard-sql",
            Self::Snowflake => "snowflake-sql",
            Self::Redshift => "redshift-postgresql",
            Self::Prometheus => "promql",
            Self::Loki => "logql",
            Self::Parquet => "datafusion-sql",
            Self::CsvJson => "in-memory-sql",
        }
    }
}

pub(crate) fn save_response(
    connection: DataConnection,
    warnings: Vec<String>,
) -> SaveConnectionResponse {
    SaveConnectionResponse {
        ok: true,
        connection,
        warnings,
    }
}

pub(crate) fn catalog_payload(connections: Vec<DataConnectionSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.connections.v1",
        "connections": connections,
        "limits": limits_payload()
    })
}

pub(crate) fn test_plan(connection: &DataConnection) -> ConnectionTestPlanResponse {
    let mut warnings = Vec::new();
    if connection.disabled {
        warnings.push("connection is disabled; live query planners should skip it".to_string());
    }
    let checks = vec![
        ConnectionCheck {
            id: "secret-ref",
            status: if connection.engine.requires_secret_ref() {
                if connection.secret_ref.is_some() {
                    CheckStatus::Ready
                } else {
                    CheckStatus::Warning
                }
            } else {
                CheckStatus::Skipped
            },
            description: if connection.engine.requires_secret_ref() {
                "secretRef is required for this engine and can be resolved by an external worker"
                    .to_string()
            } else {
                "local/import engines do not require secretRef".to_string()
            },
        },
        ConnectionCheck {
            id: "endpoint-metadata",
            status: if connection.engine.requires_endpoint() {
                if connection.endpoint.is_some() {
                    CheckStatus::Ready
                } else {
                    CheckStatus::Warning
                }
            } else {
                CheckStatus::Skipped
            },
            description:
                "endpoint metadata is present for planner routing without testing network access"
                    .to_string(),
        },
        ConnectionCheck {
            id: "dialect-target",
            status: CheckStatus::Ready,
            description: format!(
                "query planners should emit {} for this connection",
                connection.engine.sql_dialect()
            ),
        },
    ];
    ConnectionTestPlanResponse {
        ok: true,
        schema_version: "data-viz.connection-test-plan.v1",
        generated_at_ms: now_ms(),
        connection_id: connection.connection_id.clone(),
        engine: connection.engine,
        mode: connection.mode,
        disabled: connection.disabled,
        checks,
        planner_notes: vec![
            "dry-run only; this service does not open sockets or call cloud APIs".to_string(),
            "secret material must be resolved by a separately authenticated connector worker"
                .to_string(),
            format!("dialect target: {}", connection.engine.sql_dialect()),
        ],
        warnings,
    }
}

pub(crate) fn max_connections() -> usize {
    MAX_CONNECTIONS
}

fn normalize_optional_identifier(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<String>, String> {
    value
        .map(|value| clean_identifier(&value).ok_or_else(|| format!("{field} is invalid")))
        .transpose()
}

fn normalize_identifier_vec(
    values: Vec<String>,
    label: &str,
    max: usize,
) -> Result<Vec<String>, String> {
    if values.len() > max {
        return Err(format!("{label} exceeds max {max}"));
    }
    let mut normalized = values
        .into_iter()
        .filter_map(|value| clean_identifier(&value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    normalized.sort();
    Ok(normalized)
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("connection tags exceeds max {MAX_TAGS}"));
    }
    let mut tags = tags
        .into_iter()
        .filter_map(|tag| clean_identifier(&tag.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    tags.sort();
    Ok(tags)
}

fn normalize_settings(
    settings: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if settings.len() > MAX_SETTINGS {
        return Err(format!("connection settings exceeds max {MAX_SETTINGS}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in settings {
        let key = clean_identifier(&key).ok_or_else(|| "connection setting key is invalid")?;
        let key_lower = key.to_ascii_lowercase();
        if key_lower.contains("token")
            || key_lower.contains("secret")
            || key_lower.contains("password")
            || key_lower.contains("private_key")
            || key_lower == "url"
            || key_lower.ends_with("_url")
            || key_lower.ends_with("url")
        {
            return Err(format!(
                "connection setting `{key}` looks secret-bearing; use secretRef instead"
            ));
        }
        let value = value.trim().to_string();
        if value.len() > MAX_SETTING_VALUE_BYTES {
            return Err(format!(
                "connection setting `{key}` exceeds max {MAX_SETTING_VALUE_BYTES} bytes"
            ));
        }
        normalized.insert(key, value);
    }
    Ok(normalized)
}

fn bounded_label(label: &str, value: &str, max_len: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_len {
        Err(format!("{label} must be 1-{max_len} characters"))
    } else {
        Ok(value)
    }
}

fn bounded_host(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
    {
        Err("connection host must be a safe hostname".to_string())
    } else {
        Ok(value)
    }
}

fn limits_payload() -> Value {
    json!({
        "maxConnections": MAX_CONNECTIONS,
        "maxTags": MAX_TAGS,
        "maxSettings": MAX_SETTINGS,
        "maxSettingValueBytes": MAX_SETTING_VALUE_BYTES,
        "maxAllowedSchemas": MAX_ALLOWED_SCHEMAS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warehouse_connection_requires_secret_ref() {
        let error = SaveConnectionRequest {
            connection_id: "warehouse".to_string(),
            name: "Warehouse".to_string(),
            engine: ConnectionEngine::Postgres,
            mode: None,
            owner: None,
            tags: None,
            secret_ref: None,
            endpoint: Some(ConnectionEndpoint {
                host: Some("warehouse.example.com".to_string()),
                port: Some(5432),
                region: None,
                project_id: None,
                warehouse: None,
            }),
            default_database: Some("analytics".to_string()),
            default_schema: Some("public".to_string()),
            allowed_schemas: None,
            settings: None,
            disabled: None,
        }
        .into_connection(100)
        .expect_err("secret ref required");

        assert!(error.contains("secretRef"));
    }

    #[test]
    fn connection_test_plan_is_dry_run_and_dialect_aware() {
        let connection = SaveConnectionRequest {
            connection_id: "bigquery-prod".to_string(),
            name: "BigQuery Prod".to_string(),
            engine: ConnectionEngine::BigQuery,
            mode: Some(ConnectionMode::LiveQuery),
            owner: Some("analytics".to_string()),
            tags: Some(vec!["warehouse".to_string()]),
            secret_ref: Some("gcp.bigquery.prod".to_string()),
            endpoint: Some(ConnectionEndpoint {
                host: None,
                port: None,
                region: Some("us".to_string()),
                project_id: Some("demo-prod".to_string()),
                warehouse: None,
            }),
            default_database: None,
            default_schema: None,
            allowed_schemas: Some(vec!["analytics".to_string()]),
            settings: Some(BTreeMap::from([(
                "maximumBytesBilled".to_string(),
                "1000000000".to_string(),
            )])),
            disabled: None,
        }
        .into_connection(100)
        .expect("connection validates");

        let plan = test_plan(&connection);
        assert_eq!(plan.engine, ConnectionEngine::BigQuery);
        assert!(plan
            .planner_notes
            .iter()
            .any(|note| note.contains("bigquery-standard-sql")));
        assert!(connection.summary().secret_ref_configured);
    }

    #[test]
    fn connection_accepts_public_bigquery_engine_name() {
        let request: SaveConnectionRequest = serde_json::from_value(serde_json::json!({
            "connectionId": "bigquery-json",
            "name": "BigQuery JSON",
            "engine": "bigquery",
            "secretRef": "gcp.bigquery.prod",
            "endpoint": {
                "projectId": "demo-prod"
            }
        }))
        .expect("bigquery public engine name deserializes");

        assert_eq!(request.engine, ConnectionEngine::BigQuery);
    }

    #[test]
    fn connection_rejects_secret_like_settings() {
        let error = SaveConnectionRequest {
            connection_id: "bad".to_string(),
            name: "Bad".to_string(),
            engine: ConnectionEngine::CsvJson,
            mode: None,
            owner: None,
            tags: None,
            secret_ref: None,
            endpoint: None,
            default_database: None,
            default_schema: None,
            allowed_schemas: None,
            settings: Some(BTreeMap::from([(
                "password".to_string(),
                "nope".to_string(),
            )])),
            disabled: None,
        }
        .into_connection(100)
        .expect_err("secret-like setting rejected");

        assert!(error.contains("secret-bearing"));
    }
}
