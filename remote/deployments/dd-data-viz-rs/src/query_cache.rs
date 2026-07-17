use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use serde::Serialize;
use serde_json::{json, Value};

const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHED_ROWS: usize = 1_000;
const DEFAULT_TTL_MS: u128 = 15 * 60 * 1_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryCacheEntry {
    pub cache_id: String,
    pub request_hash: String,
    pub source: String,
    pub dialect: String,
    pub row_count: usize,
    pub cached_row_count: usize,
    pub truncated: bool,
    pub rows: Vec<BTreeMap<String, Value>>,
    pub logical_plan: Value,
    pub warnings: Vec<String>,
    pub duration_ms: u128,
    pub hit_count: u64,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub expires_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryCacheSummary {
    cache_id: String,
    request_hash: String,
    source: String,
    dialect: String,
    row_count: usize,
    cached_row_count: usize,
    truncated: bool,
    warning_count: usize,
    duration_ms: u128,
    hit_count: u64,
    updated_at_ms: u128,
    expires_at_ms: u128,
}

impl QueryCacheEntry {
    pub(crate) fn summary(&self) -> QueryCacheSummary {
        QueryCacheSummary {
            cache_id: self.cache_id.clone(),
            request_hash: self.request_hash.clone(),
            source: self.source.clone(),
            dialect: self.dialect.clone(),
            row_count: self.row_count,
            cached_row_count: self.cached_row_count,
            truncated: self.truncated,
            warning_count: self.warnings.len(),
            duration_ms: self.duration_ms,
            hit_count: self.hit_count,
            updated_at_ms: self.updated_at_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }

    pub(crate) fn touch(&mut self, now_ms: u128) {
        self.hit_count = self.hit_count.saturating_add(1);
        self.updated_at_ms = now_ms;
    }
}

pub(crate) fn entry_from_result(
    request: &Value,
    logical_plan: Value,
    rows: Vec<BTreeMap<String, Value>>,
    row_count: usize,
    warnings: Vec<String>,
    duration_ms: u128,
    now_ms: u128,
) -> QueryCacheEntry {
    let request_hash = value_hash(request);
    let source = logical_plan
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let dialect = logical_plan
        .get("dialect")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let cached_rows = rows.into_iter().take(MAX_CACHED_ROWS).collect::<Vec<_>>();
    let cached_row_count = cached_rows.len();
    QueryCacheEntry {
        cache_id: format!("query-cache-{request_hash}"),
        request_hash,
        source,
        dialect,
        row_count,
        cached_row_count,
        truncated: row_count > cached_row_count,
        rows: cached_rows,
        logical_plan,
        warnings,
        duration_ms,
        hit_count: 0,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(DEFAULT_TTL_MS),
    }
}

pub(crate) fn store_entry(
    cache: &mut BTreeMap<String, QueryCacheEntry>,
    mut entry: QueryCacheEntry,
    now_ms: u128,
) -> QueryCacheEntry {
    prune_expired(cache, now_ms);
    if let Some(existing) = cache.get(&entry.cache_id) {
        entry.created_at_ms = existing.created_at_ms;
        entry.hit_count = existing.hit_count;
    } else if cache.len() >= MAX_CACHE_ENTRIES {
        if let Some(oldest_id) = cache
            .values()
            .min_by_key(|entry| entry.updated_at_ms)
            .map(|entry| entry.cache_id.clone())
        {
            cache.remove(&oldest_id);
        }
    }
    entry.updated_at_ms = now_ms;
    entry.expires_at_ms = now_ms.saturating_add(DEFAULT_TTL_MS);
    cache.insert(entry.cache_id.clone(), entry.clone());
    entry
}

pub(crate) fn prune_expired(cache: &mut BTreeMap<String, QueryCacheEntry>, now_ms: u128) {
    cache.retain(|_, entry| entry.expires_at_ms > now_ms);
}

pub(crate) fn catalog_payload(entries: Vec<QueryCacheSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.query-cache.v1",
        "entries": entries,
        "limits": limits_payload()
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxEntries": MAX_CACHE_ENTRIES,
        "maxCachedRows": MAX_CACHED_ROWS,
        "ttlMs": DEFAULT_TTL_MS,
        "posture": "in-memory result snapshots; summaries do not include raw query text"
    })
}

fn value_hash(value: &Value) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_cache_entry_truncates_rows_and_omits_raw_query() {
        let request = json!({
            "dialect": "sql",
            "query": "SELECT region FROM sales-lab"
        });
        let rows = (0..1_200)
            .map(|index| BTreeMap::from([("region".to_string(), Value::from(index))]))
            .collect::<Vec<_>>();
        let entry = entry_from_result(
            &request,
            json!({"source": "sales-lab", "dialect": "sql"}),
            rows,
            1_200,
            Vec::new(),
            4,
            100,
        );
        let summary = serde_json::to_value(entry.summary()).expect("summary serializes");

        assert_eq!(entry.cached_row_count, MAX_CACHED_ROWS);
        assert!(entry.truncated);
        assert!(summary.get("rows").is_none());
        assert!(summary.get("query").is_none());
        assert!(entry.cache_id.starts_with("query-cache-"));
    }

    #[test]
    fn query_cache_store_replaces_existing_entry() {
        let request = json!({"dialect": "sql", "query": "SELECT region FROM sales-lab"});
        let mut cache = BTreeMap::new();
        let entry = entry_from_result(
            &request,
            json!({"source": "sales-lab", "dialect": "sql"}),
            Vec::new(),
            0,
            Vec::new(),
            1,
            100,
        );
        let first = store_entry(&mut cache, entry, 100);
        let replacement = entry_from_result(
            &request,
            json!({"source": "sales-lab", "dialect": "sql"}),
            Vec::new(),
            0,
            vec!["fresh".to_string()],
            2,
            200,
        );
        let second = store_entry(&mut cache, replacement, 200);

        assert_eq!(first.cache_id, second.cache_id);
        assert_eq!(second.created_at_ms, 100);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.values().next().unwrap().warnings.len(), 1);
    }

    #[test]
    fn query_cache_prunes_expired_entries() {
        let request = json!({"dialect": "sql", "query": "SELECT region FROM sales-lab"});
        let mut cache = BTreeMap::new();
        let entry = entry_from_result(
            &request,
            json!({"source": "sales-lab", "dialect": "sql"}),
            Vec::new(),
            0,
            Vec::new(),
            1,
            100,
        );
        store_entry(&mut cache, entry, 100);
        prune_expired(&mut cache, DEFAULT_TTL_MS + 101);

        assert!(cache.is_empty());
    }
}
