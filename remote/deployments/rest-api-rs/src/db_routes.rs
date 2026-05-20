use std::collections::HashSet;

use axum::{
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_postgres::Client;

const DEFAULT_SCHEMA: &str = "public";
const SOURCE: &str = "rds-postgres";
const PG_DEFS_SOURCE: &str = "remote/libs/pg-defs";

#[derive(Deserialize)]
struct DbTablesQuery {
    schema: Option<String>,
    include_views: Option<bool>,
}

#[derive(Deserialize)]
struct DbRowsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    order: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DbColumnMetadata {
    name: String,
    ordinal_position: i32,
    data_type: String,
    udt_name: String,
    is_nullable: bool,
    has_default: bool,
    default: Option<String>,
    is_primary_key: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DbTableContractMetadata {
    source: String,
    known: bool,
    missing_columns: Vec<String>,
    extra_columns: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DbTableMetadata {
    schema: String,
    name: String,
    table_type: String,
    writable: bool,
    primary_key: Vec<String>,
    columns: Vec<DbColumnMetadata>,
    contract: DbTableContractMetadata,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbTablesResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    schema: Option<String>,
    tables: Vec<DbTableMetadata>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbTableResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    table: Option<DbTableMetadata>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbRowsResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    schema: String,
    table: String,
    primary_key: Vec<String>,
    limit: i64,
    offset: i64,
    rows: Vec<Value>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbRowResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    schema: String,
    table: String,
    primary_key: Vec<String>,
    row: Option<Value>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbContractTable {
    schema: String,
    name: String,
    columns: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbContractResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    tables: Vec<DbContractTable>,
}

pub fn router() -> Router {
    Router::new()
        .route("/contract/tables", get(contract_tables))
        .route("/pg-defs/tables", get(contract_tables))
        .route("/tables", get(list_tables))
        .route("/tables/:table", get(table_metadata_default_schema))
        .route(
            "/tables/:table/rows",
            get(list_rows_default_schema).post(insert_row_default_schema),
        )
        .route(
            "/tables/:table/rows/:id",
            get(get_row_default_schema)
                .patch(update_row_default_schema)
                .delete(delete_row_default_schema),
        )
        .route(
            "/schemas/:schema/tables/:table",
            get(table_metadata_explicit_schema),
        )
        .route(
            "/schemas/:schema/tables/:table/rows",
            get(list_rows_explicit_schema).post(insert_row_explicit_schema),
        )
        .route(
            "/schemas/:schema/tables/:table/rows/:id",
            get(get_row_explicit_schema)
                .patch(update_row_explicit_schema)
                .delete(delete_row_explicit_schema),
        )
}

async fn contract_tables(headers: HeaderMap) -> Response {
    super::record_request("GET", "/api/db/contract/tables", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, false) {
        return response;
    }

    let tables = super::pg_contract::CANONICAL_TABLES
        .iter()
        .map(|table| DbContractTable {
            schema: DEFAULT_SCHEMA.to_string(),
            name: table.name.to_string(),
            columns: table
                .columns
                .iter()
                .map(|column| (*column).to_string())
                .collect(),
        })
        .collect::<Vec<_>>();

    Json(DbContractResponse {
        ok: true,
        source: PG_DEFS_SOURCE.to_string(),
        generated_at_ms: super::now_ms(),
        tables,
    })
    .into_response()
}

async fn list_tables(headers: HeaderMap, Query(query): Query<DbTablesQuery>) -> Response {
    super::record_request("GET", "/api/db/tables", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let schema = match normalize_optional_schema(query.schema.as_deref()) {
        Ok(schema) => schema,
        Err(error) => return bad_request(error),
    };

    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first table discovery failed to connect to postgres: {error}");
            return db_error_response("table discovery");
        }
    };

    match fetch_tables(
        &client,
        schema.as_deref(),
        query.include_views.unwrap_or(true),
    )
    .await
    {
        Ok(tables) => Json(DbTablesResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            tables,
            errors: Vec::new(),
        })
        .into_response(),
        Err(error) => {
            eprintln!("db-first table discovery failed: {error}");
            db_error_response("table discovery")
        }
    }
}

async fn table_metadata_default_schema(headers: HeaderMap, Path(table): Path<String>) -> Response {
    table_metadata(headers, DEFAULT_SCHEMA.to_string(), table).await
}

async fn table_metadata_explicit_schema(
    headers: HeaderMap,
    Path((schema, table)): Path<(String, String)>,
) -> Response {
    table_metadata(headers, schema, table).await
}

async fn list_rows_default_schema(
    headers: HeaderMap,
    Path(table): Path<String>,
    Query(query): Query<DbRowsQuery>,
) -> Response {
    list_rows(headers, DEFAULT_SCHEMA.to_string(), table, query).await
}

async fn list_rows_explicit_schema(
    headers: HeaderMap,
    Path((schema, table)): Path<(String, String)>,
    Query(query): Query<DbRowsQuery>,
) -> Response {
    list_rows(headers, schema, table, query).await
}

async fn get_row_default_schema(
    headers: HeaderMap,
    Path((table, id)): Path<(String, String)>,
) -> Response {
    get_row(headers, DEFAULT_SCHEMA.to_string(), table, id).await
}

async fn get_row_explicit_schema(
    headers: HeaderMap,
    Path((schema, table, id)): Path<(String, String, String)>,
) -> Response {
    get_row(headers, schema, table, id).await
}

async fn insert_row_default_schema(
    headers: HeaderMap,
    Path(table): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    insert_row(headers, DEFAULT_SCHEMA.to_string(), table, body).await
}

async fn insert_row_explicit_schema(
    headers: HeaderMap,
    Path((schema, table)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    insert_row(headers, schema, table, body).await
}

async fn update_row_default_schema(
    headers: HeaderMap,
    Path((table, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    update_row(headers, DEFAULT_SCHEMA.to_string(), table, id, body).await
}

async fn update_row_explicit_schema(
    headers: HeaderMap,
    Path((schema, table, id)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    update_row(headers, schema, table, id, body).await
}

async fn delete_row_default_schema(
    headers: HeaderMap,
    Path((table, id)): Path<(String, String)>,
) -> Response {
    delete_row_by_id(headers, DEFAULT_SCHEMA.to_string(), table, id).await
}

async fn delete_row_explicit_schema(
    headers: HeaderMap,
    Path((schema, table, id)): Path<(String, String, String)>,
) -> Response {
    delete_row_by_id(headers, schema, table, id).await
}

async fn table_metadata(headers: HeaderMap, schema: String, table: String) -> Response {
    super::record_request("GET", "/api/db/tables/:table", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first table metadata failed to connect to postgres: {error}");
            return db_error_response("table metadata");
        }
    };
    match load_table_metadata(&client, &schema, &table).await {
        Ok(Some(table)) => Json(DbTableResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            table: Some(table),
            errors: Vec::new(),
        })
        .into_response(),
        Ok(None) => not_found(format!("{schema}.{table} was not found")),
        Err(error) => {
            eprintln!("db-first table metadata failed: {error}");
            db_error_response("table metadata")
        }
    }
}

async fn list_rows(
    headers: HeaderMap,
    schema: String,
    table: String,
    query: DbRowsQuery,
) -> Response {
    super::record_request("GET", "/api/db/tables/:table/rows", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let limit = rows_limit(&query);
    let offset = rows_offset(&query);
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first row list failed to connect to postgres: {error}");
            return db_error_response("row list");
        }
    };
    let metadata = match require_table_metadata(&client, &schema, &table).await {
        Ok(table) => table,
        Err(response) => return response,
    };
    let order_clause = match order_clause(&metadata, query.order.as_deref()) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let table_name = qualified_name(&schema, &table);
    let sql =
        format!("select to_jsonb(t) as row from {table_name} t {order_clause} limit $1 offset $2");
    match client.query(&sql, &[&limit, &offset]).await {
        Ok(rows) => Json(DbRowsResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            table,
            primary_key: metadata.primary_key,
            limit,
            offset,
            rows: rows.iter().map(row_json_value).collect(),
            errors: Vec::new(),
        })
        .into_response(),
        Err(error) => {
            eprintln!("db-first row list failed: {error}");
            db_error_response("row list")
        }
    }
}

async fn get_row(headers: HeaderMap, schema: String, table: String, id: String) -> Response {
    super::record_request("GET", "/api/db/tables/:table/rows/:id", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first row fetch failed to connect to postgres: {error}");
            return db_error_response("row fetch");
        }
    };
    let metadata = match require_table_metadata(&client, &schema, &table).await {
        Ok(table) => table,
        Err(response) => return response,
    };
    let pk = match single_primary_key(&metadata) {
        Ok(pk) => pk,
        Err(error) => return bad_request(error),
    };
    let table_name = qualified_name(&schema, &table);
    let pk_name = quote_identifier(pk);
    let sql = format!(
        "select to_jsonb(t) as row from {table_name} t where t.{pk_name}::text = $1 limit 1"
    );
    match client.query_opt(&sql, &[&id]).await {
        Ok(Some(row)) => Json(DbRowResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            table,
            primary_key: metadata.primary_key,
            row: Some(row_json_value(&row)),
            errors: Vec::new(),
        })
        .into_response(),
        Ok(None) => not_found(format!("{schema}.{table} row was not found")),
        Err(error) => {
            eprintln!("db-first row fetch failed: {error}");
            db_error_response("row fetch")
        }
    }
}

async fn insert_row(headers: HeaderMap, schema: String, table: String, body: Value) -> Response {
    super::record_request("POST", "/api/db/tables/:table/rows", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let body = match row_object_from_body(body) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first row insert failed to connect to postgres: {error}");
            return db_error_response("row insert");
        }
    };
    let metadata = match require_table_metadata(&client, &schema, &table).await {
        Ok(table) => table,
        Err(response) => return response,
    };
    if !metadata.writable {
        return method_not_allowed(format!("{} is not a base table", metadata.table_type));
    }
    let columns = match body_columns(&metadata, &body, true) {
        Ok(columns) => columns,
        Err(error) => return bad_request(error),
    };
    let table_name = qualified_name(&schema, &table);
    let sql = if columns.is_empty() {
        format!("insert into {table_name} as t default values returning to_jsonb(t) as row")
    } else {
        let insert_columns = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        let select_columns = columns
            .iter()
            .map(|column| format!("r.{}", quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "insert into {table_name} as t ({insert_columns}) \
             select {select_columns} from jsonb_populate_record(null::{table_name}, $1::jsonb) as r \
             returning to_jsonb(t) as row"
        )
    };
    let row_json = Value::Object(body);
    let result = if columns.is_empty() {
        client.query_one(&sql, &[]).await
    } else {
        client.query_one(&sql, &[&row_json]).await
    };
    match result {
        Ok(row) => Json(DbRowResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            table,
            primary_key: metadata.primary_key,
            row: Some(row_json_value(&row)),
            errors: Vec::new(),
        })
        .into_response(),
        Err(error) => {
            eprintln!("db-first row insert failed: {error}");
            db_error_response("row insert")
        }
    }
}

async fn update_row(
    headers: HeaderMap,
    schema: String,
    table: String,
    id: String,
    body: Value,
) -> Response {
    super::record_request("PATCH", "/api/db/tables/:table/rows/:id", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let body = match row_object_from_body(body) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first row update failed to connect to postgres: {error}");
            return db_error_response("row update");
        }
    };
    let metadata = match require_table_metadata(&client, &schema, &table).await {
        Ok(table) => table,
        Err(response) => return response,
    };
    if !metadata.writable {
        return method_not_allowed(format!("{} is not a base table", metadata.table_type));
    }
    let pk = match single_primary_key(&metadata) {
        Ok(pk) => pk,
        Err(error) => return bad_request(error),
    };
    let columns = match body_columns(&metadata, &body, false) {
        Ok(columns) if columns.is_empty() => return bad_request("PATCH body is empty".to_string()),
        Ok(columns) => columns,
        Err(error) => return bad_request(error),
    };
    let table_name = qualified_name(&schema, &table);
    let assignments = columns
        .iter()
        .map(|column| {
            let column = quote_identifier(column);
            format!("{column} = r.{column}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let pk_name = quote_identifier(pk);
    let sql = format!(
        "update {table_name} as t \
         set {assignments} \
         from jsonb_populate_record(null::{table_name}, $1::jsonb) as r \
         where t.{pk_name}::text = $2 \
         returning to_jsonb(t) as row"
    );
    let row_json = Value::Object(body);
    match client.query_opt(&sql, &[&row_json, &id]).await {
        Ok(Some(row)) => Json(DbRowResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            table,
            primary_key: metadata.primary_key,
            row: Some(row_json_value(&row)),
            errors: Vec::new(),
        })
        .into_response(),
        Ok(None) => not_found(format!("{schema}.{table} row was not found")),
        Err(error) => {
            eprintln!("db-first row update failed: {error}");
            db_error_response("row update")
        }
    }
}

async fn delete_row_by_id(
    headers: HeaderMap,
    schema: String,
    table: String,
    id: String,
) -> Response {
    super::record_request("DELETE", "/api/db/tables/:table/rows/:id", StatusCode::OK);
    if let Some(response) = require_db_route_access(&headers, true) {
        return response;
    }

    let (schema, table) = match normalize_table_path(&schema, &table) {
        Ok(value) => value,
        Err(error) => return bad_request(error),
    };
    let client = match super::connect_postgres().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("db-first row delete failed to connect to postgres: {error}");
            return db_error_response("row delete");
        }
    };
    let metadata = match require_table_metadata(&client, &schema, &table).await {
        Ok(table) => table,
        Err(response) => return response,
    };
    if !metadata.writable {
        return method_not_allowed(format!("{} is not a base table", metadata.table_type));
    }
    let pk = match single_primary_key(&metadata) {
        Ok(pk) => pk,
        Err(error) => return bad_request(error),
    };
    let table_name = qualified_name(&schema, &table);
    let pk_name = quote_identifier(pk);
    let sql = format!(
        "delete from {table_name} as t where t.{pk_name}::text = $1 returning to_jsonb(t) as row"
    );
    match client.query_opt(&sql, &[&id]).await {
        Ok(Some(row)) => Json(DbRowResponse {
            ok: true,
            source: SOURCE.to_string(),
            generated_at_ms: super::now_ms(),
            schema,
            table,
            primary_key: metadata.primary_key,
            row: Some(row_json_value(&row)),
            errors: Vec::new(),
        })
        .into_response(),
        Ok(None) => not_found(format!("{schema}.{table} row was not found")),
        Err(error) => {
            eprintln!("db-first row delete failed: {error}");
            db_error_response("row delete")
        }
    }
}

fn require_db_route_access(headers: &HeaderMap, require_database: bool) -> Option<Response> {
    if !authorized_db_route(headers) {
        return Some(super::unauthorized_response());
    }
    if require_database && super::postgres_database_url().is_none() {
        return Some(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "postgres database URL is not configured" })),
            )
                .into_response(),
        );
    }
    None
}

fn authorized_db_route(headers: &HeaderMap) -> bool {
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

async fn fetch_tables(
    client: &Client,
    schema_filter: Option<&str>,
    include_views: bool,
) -> Result<Vec<DbTableMetadata>, String> {
    let rows = client
        .query(
            r#"
            select table_schema, table_name, table_type
            from information_schema.tables
            where table_schema not in ('pg_catalog', 'information_schema')
              and table_schema not like 'pg_toast%'
              and ($1::text is null or table_schema = $1)
              and table_type in ('BASE TABLE', 'VIEW')
              and ($2::bool or table_type = 'BASE TABLE')
            order by table_schema, table_name
            "#,
            &[&schema_filter, &include_views],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let schema = row_string(&row, "table_schema");
        let table = row_string(&row, "table_name");
        if let Some(metadata) = load_table_metadata(client, &schema, &table).await? {
            tables.push(metadata);
        }
    }
    Ok(tables)
}

async fn require_table_metadata(
    client: &Client,
    schema: &str,
    table: &str,
) -> Result<DbTableMetadata, Response> {
    match load_table_metadata(client, schema, table).await {
        Ok(Some(metadata)) => Ok(metadata),
        Ok(None) => Err(not_found(format!("{schema}.{table} was not found"))),
        Err(error) => {
            eprintln!("db-first table metadata lookup failed: {error}");
            Err(db_error_response("table metadata"))
        }
    }
}

async fn load_table_metadata(
    client: &Client,
    schema: &str,
    table: &str,
) -> Result<Option<DbTableMetadata>, String> {
    let table_row = client
        .query_opt(
            r#"
            select table_type
            from information_schema.tables
            where table_schema = $1
              and table_name = $2
              and table_schema not in ('pg_catalog', 'information_schema')
              and table_schema not like 'pg_toast%'
              and table_type in ('BASE TABLE', 'VIEW')
            limit 1
            "#,
            &[&schema, &table],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(table_row) = table_row else {
        return Ok(None);
    };
    let table_type = row_string(&table_row, "table_type");

    let pk_rows = client
        .query(
            r#"
            select kcu.column_name
            from information_schema.table_constraints tc
            join information_schema.key_column_usage kcu
              on kcu.constraint_schema = tc.constraint_schema
             and kcu.constraint_name = tc.constraint_name
             and kcu.table_schema = tc.table_schema
             and kcu.table_name = tc.table_name
            where tc.table_schema = $1
              and tc.table_name = $2
              and tc.constraint_type = 'PRIMARY KEY'
            order by kcu.ordinal_position
            "#,
            &[&schema, &table],
        )
        .await
        .map_err(|error| error.to_string())?;
    let primary_key = pk_rows
        .iter()
        .map(|row| row_string(row, "column_name"))
        .collect::<Vec<_>>();
    let primary_key_set = primary_key.iter().cloned().collect::<HashSet<_>>();

    let column_rows = client
        .query(
            r#"
            select
              column_name,
              ordinal_position::integer as ordinal_position,
              data_type,
              udt_name,
              (is_nullable = 'YES') as is_nullable,
              (column_default is not null) as has_default,
              column_default
            from information_schema.columns
            where table_schema = $1
              and table_name = $2
            order by ordinal_position
            "#,
            &[&schema, &table],
        )
        .await
        .map_err(|error| error.to_string())?;
    let columns = column_rows
        .iter()
        .map(|row| {
            let name = row_string(row, "column_name");
            DbColumnMetadata {
                is_primary_key: primary_key_set.contains(&name),
                name,
                ordinal_position: row_i32(row, "ordinal_position"),
                data_type: row_string(row, "data_type"),
                udt_name: row_string(row, "udt_name"),
                is_nullable: row_bool(row, "is_nullable"),
                has_default: row_bool(row, "has_default"),
                default: row_opt_string(row, "column_default"),
            }
        })
        .collect::<Vec<_>>();
    let contract = table_contract(schema, table, &columns);

    Ok(Some(DbTableMetadata {
        schema: schema.to_string(),
        name: table.to_string(),
        table_type: table_type.clone(),
        writable: table_type == "BASE TABLE",
        primary_key,
        columns,
        contract,
    }))
}

fn table_contract(
    schema: &str,
    table: &str,
    columns: &[DbColumnMetadata],
) -> DbTableContractMetadata {
    let Some(canonical_columns) = (schema == DEFAULT_SCHEMA)
        .then(|| super::pg_contract::canonical_table_columns(table))
        .flatten()
    else {
        return DbTableContractMetadata {
            source: PG_DEFS_SOURCE.to_string(),
            known: false,
            missing_columns: Vec::new(),
            extra_columns: Vec::new(),
        };
    };

    let live_columns = columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let canonical_set = canonical_columns.iter().copied().collect::<HashSet<_>>();
    let mut missing_columns = canonical_columns
        .iter()
        .filter(|column| !live_columns.contains(**column))
        .map(|column| (*column).to_string())
        .collect::<Vec<_>>();
    let mut extra_columns = columns
        .iter()
        .filter(|column| !canonical_set.contains(column.name.as_str()))
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    missing_columns.sort();
    extra_columns.sort();

    DbTableContractMetadata {
        source: PG_DEFS_SOURCE.to_string(),
        known: true,
        missing_columns,
        extra_columns,
    }
}

fn rows_limit(query: &DbRowsQuery) -> i64 {
    query.limit.unwrap_or(100).clamp(1, 500)
}

fn rows_offset(query: &DbRowsQuery) -> i64 {
    query.offset.unwrap_or_default().clamp(0, 100_000)
}

fn order_clause(table: &DbTableMetadata, requested: Option<&str>) -> Result<String, String> {
    let Some(raw) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        let default_column = table
            .columns
            .iter()
            .find(|column| column.name == "updated_at")
            .or_else(|| {
                table
                    .columns
                    .iter()
                    .find(|column| column.name == "created_at")
            })
            .or_else(|| {
                table
                    .primary_key
                    .first()
                    .and_then(|name| table.columns.iter().find(|column| column.name == *name))
            })
            .or_else(|| table.columns.first());
        return Ok(default_column
            .map(|column| {
                let direction = if column.name == "updated_at" || column.name == "created_at" {
                    "desc"
                } else {
                    "asc"
                };
                format!("order by t.{} {direction}", quote_identifier(&column.name))
            })
            .unwrap_or_default());
    };

    let (column, direction) = if let Some(column) = raw.strip_prefix('-') {
        (column, "desc")
    } else if let Some(column) = raw.strip_suffix(".desc") {
        (column, "desc")
    } else if let Some(column) = raw.strip_suffix(":desc") {
        (column, "desc")
    } else if let Some(column) = raw.strip_suffix(".asc") {
        (column, "asc")
    } else if let Some(column) = raw.strip_suffix(":asc") {
        (column, "asc")
    } else {
        (raw, "asc")
    };
    if !table.columns.iter().any(|item| item.name == column) {
        return Err(format!("unknown order column `{column}`"));
    }
    Ok(format!(
        "order by t.{} {direction}",
        quote_identifier(column)
    ))
}

fn single_primary_key(table: &DbTableMetadata) -> Result<&str, String> {
    match table.primary_key.as_slice() {
        [column] => Ok(column),
        [] => Err(format!(
            "{}.{} does not have a primary key; only row listing is available",
            table.schema, table.name
        )),
        columns => Err(format!(
            "{}.{} has a composite primary key ({}) that is not addressable by /rows/:id yet",
            table.schema,
            table.name,
            columns.join(", ")
        )),
    }
}

fn body_columns(
    table: &DbTableMetadata,
    body: &Map<String, Value>,
    allow_primary_key: bool,
) -> Result<Vec<String>, String> {
    let requested = body.keys().cloned().collect::<HashSet<_>>();
    let known = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let mut unknown = requested
        .iter()
        .filter(|column| !known.contains(column.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unknown.sort();
    if !unknown.is_empty() {
        return Err(format!("unknown column(s): {}", unknown.join(", ")));
    }

    if !allow_primary_key {
        let mut primary_key_updates = requested
            .iter()
            .filter(|column| table.primary_key.contains(column))
            .cloned()
            .collect::<Vec<_>>();
        primary_key_updates.sort();
        if !primary_key_updates.is_empty() {
            return Err(format!(
                "primary key column(s) cannot be patched: {}",
                primary_key_updates.join(", ")
            ));
        }
    }

    Ok(table
        .columns
        .iter()
        .filter(|column| requested.contains(&column.name))
        .map(|column| column.name.clone())
        .collect())
}

fn row_object_from_body(body: Value) -> Result<Map<String, Value>, String> {
    match body {
        Value::Object(mut object) => {
            if object.len() == 1 {
                if let Some(value) = object.remove("row") {
                    return match value {
                        Value::Object(row) => Ok(row),
                        _ => Err("row must be a JSON object".to_string()),
                    };
                }
            }
            Ok(object)
        }
        _ => Err("request body must be a JSON object".to_string()),
    }
}

fn normalize_optional_schema(schema: Option<&str>) -> Result<Option<String>, String> {
    schema
        .map(|schema| normalize_identifier(schema, "schema"))
        .transpose()
}

fn normalize_table_path(schema: &str, table: &str) -> Result<(String, String), String> {
    let schema = normalize_identifier(schema, "schema")?;
    let table = normalize_identifier(table, "table")?;
    Ok((schema, table))
}

fn normalize_identifier(value: &str, kind: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{kind} is required"));
    }
    if value.len() > 63 {
        return Err(format!("{kind} must be 63 bytes or fewer"));
    }
    if matches!(value, "pg_catalog" | "information_schema") || value.starts_with("pg_toast") {
        return Err(format!("{kind} targets a system schema"));
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(format!("{kind} is required"));
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!(
            "{kind} must start with an ASCII letter or underscore"
        ));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(format!(
            "{kind} may contain only ASCII letters, numbers, and underscores"
        ));
    }
    Ok(value.to_string())
}

fn qualified_name(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_identifier(schema), quote_identifier(table))
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn row_json_value(row: &tokio_postgres::Row) -> Value {
    row.try_get::<_, Value>("row").unwrap_or_else(|_| json!({}))
}

fn row_string(row: &tokio_postgres::Row, column: &str) -> String {
    row.try_get::<_, String>(column).unwrap_or_default()
}

fn row_opt_string(row: &tokio_postgres::Row, column: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(column).ok().flatten()
}

fn row_i32(row: &tokio_postgres::Row, column: &str) -> i32 {
    row.try_get::<_, i32>(column).unwrap_or_default()
}

fn row_bool(row: &tokio_postgres::Row, column: &str) -> bool {
    row.try_get::<_, bool>(column).unwrap_or_default()
}

fn bad_request(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
}

fn not_found(error: String) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": error }))).into_response()
}

fn method_not_allowed(error: String) -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({ "error": error })),
    )
        .into_response()
}

fn db_error_response(operation: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": super::public_data_source_error(&format!("postgres {operation}")) })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_safe_identifiers() {
        assert_eq!(
            normalize_table_path("public", "lambda_functions").unwrap(),
            ("public".to_string(), "lambda_functions".to_string())
        );
        assert!(normalize_table_path("pg_catalog", "pg_class").is_err());
        assert!(normalize_table_path("public", "lambda-functions").is_err());
    }

    #[test]
    fn row_body_accepts_bare_or_wrapped_json() {
        let bare = row_object_from_body(json!({ "slug": "demo" })).unwrap();
        assert_eq!(bare.get("slug").and_then(Value::as_str), Some("demo"));

        let wrapped = row_object_from_body(json!({ "row": { "slug": "demo" } })).unwrap();
        assert_eq!(wrapped.get("slug").and_then(Value::as_str), Some("demo"));
    }

    #[test]
    fn pg_defs_contract_is_available_for_db_first_metadata() {
        assert!(super::super::pg_contract::canonical_table_columns(
            super::super::pg_contract::LAMBDA_FUNCTIONS_TABLE
        )
        .is_some_and(|columns| columns.contains(&"function_body")));
    }
}
