//! Postgres-backed multi-signal search engine.
//!
//! Combines the five retrieval signals over `search_documents` / `search_edges`:
//!   1. lexical  — `content_tsv @@ websearch_to_tsquery(...)`     (same words)
//!   2. trigram  — `content <-> $q` via pg_trgm                   (same characters)
//!   3. semantic — `embedding <=> $vec` via pgvector cosine       (same meaning)
//!   4. structured — JSONB/typed predicate filters (hard WHERE)   (same attributes)
//!   5. graph    — recursive-CTE traversal of edges from seeds    (same relationships)
//!
//! Text signals each produce a ranked candidate list; those are merged with
//! Reciprocal Rank Fusion, structured filters constrain every signal, the graph
//! contributes an additional ranked list, and an optional rerank stage reorders
//! the fused top-N via the existing rerank providers.

mod entities;
pub mod filters;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::embedder::Embedder;
use crate::error::ApiError;
use crate::providers::rerank::{RerankRegistry, RerankRequest};
use crate::providers::{EmbedRequest, InputType};
use entities::{search_documents, search_edges};
use filters::{push, to_values, Bound};

const RRF_K: f64 = 60.0;
/// How many fused candidates to hand the reranker before truncating to top_k.
const RERANK_POOL: usize = 100;

// --- request / response types ----------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EdgeRef {
    /// external_id of the destination document.
    pub to: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct IndexDoc {
    #[serde(default)]
    pub external_id: Option<String>,
    pub content: String,
    #[serde(default)]
    pub attributes: Value,
    #[serde(default)]
    pub edges: Vec<EdgeRef>,
}

#[derive(Debug, Deserialize)]
pub struct IndexRequest {
    pub collection: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    pub documents: Vec<IndexDoc>,
}

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub collection: String,
    pub indexed: usize,
    pub edges: usize,
}

fn weight_one() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct Signals {
    #[serde(default = "weight_one")]
    pub lexical: f64,
    #[serde(default = "weight_one")]
    pub trigram: f64,
    #[serde(default = "weight_one")]
    pub semantic: f64,
}

impl Default for Signals {
    fn default() -> Self {
        Self { lexical: 1.0, trigram: 1.0, semantic: 1.0 }
    }
}

#[derive(Debug, Deserialize)]
pub struct GraphCfg {
    /// Seed documents by external_id; results are docs reachable from them.
    #[serde(default)]
    pub seeds: Vec<String>,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub max_hops: Option<u32>,
    #[serde(default = "weight_one")]
    pub weight: f64,
}

#[derive(Debug, Deserialize)]
pub struct RerankCfg {
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub collection: String,
    pub query: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub signals: Option<Signals>,
    /// Structured filters (see `filters` module). Defaults to no filter.
    #[serde(default)]
    pub filters: Value,
    #[serde(default)]
    pub graph: Option<GraphCfg>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub rerank: Option<RerankCfg>,
}

fn default_top_k() -> usize {
    10
}

#[derive(Debug, Serialize)]
pub struct Hit {
    pub id: String,
    pub external_id: Option<String>,
    pub content: String,
    pub attributes: Value,
    pub score: f64,
    /// Per-signal 1-based rank of this doc within each signal that matched it.
    pub signals: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub collection: String,
    pub signals_used: Vec<String>,
    pub hits: Vec<Hit>,
}

#[derive(Debug, Deserialize)]
pub struct AddEdgesRequest {
    pub collection: String,
    pub edges: Vec<EdgeTriple>,
}

#[derive(Debug, Deserialize)]
pub struct EdgeTriple {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub collection: String,
    pub external_ids: Vec<String>,
}

// --- service ----------------------------------------------------------------

pub struct SearchService {
    pool: DatabaseConnection,
    embedder: Arc<Embedder>,
    rerank: Arc<RerankRegistry>,
    search_dim: u32,
    candidate_k: usize,
    max_hops: u32,
}

fn db_err(e: sea_orm::DbErr) -> ApiError {
    ApiError::from(e)
}

fn vector_literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

impl SearchService {
    pub fn new(
        pool: DatabaseConnection,
        embedder: Arc<Embedder>,
        rerank: Arc<RerankRegistry>,
        search_dim: u32,
        candidate_k: usize,
        max_hops: u32,
    ) -> Self {
        Self { pool, embedder, rerank, search_dim, candidate_k, max_hops }
    }

    pub async fn health(&self) -> Result<(), ApiError> {
        self.pool
            .query_one(Statement::from_string(DatabaseBackend::Postgres, "select 1"))
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Embed texts at the index's fixed dimensionality.
    async fn embed(
        &self,
        provider: &str,
        model: &Option<String>,
        texts: Vec<String>,
        input_type: InputType,
    ) -> Result<Vec<Vec<f32>>, ApiError> {
        let req = EmbedRequest {
            input: texts,
            model: model.clone(),
            dimensions: Some(self.search_dim),
            input_type,
        };
        let resp = self.embedder.embed(provider, &req).await?;
        if resp.dimensions as u32 != self.search_dim {
            return Err(ApiError::Invalid(format!(
                "model `{}` produced {}-dim vectors but the search index expects {} \
                 (use a model that supports {}-dim output)",
                resp.model, resp.dimensions, self.search_dim, self.search_dim
            )));
        }
        let mut out = vec![Vec::new(); resp.embeddings.len()];
        for e in resp.embeddings {
            if let Some(slot) = out.get_mut(e.index) {
                *slot = e.vector;
            }
        }
        Ok(out)
    }

    pub async fn index(&self, req: IndexRequest) -> Result<IndexResponse, ApiError> {
        let contents: Vec<String> = req.documents.iter().map(|d| d.content.clone()).collect();
        let vectors = self.embed(&req.provider, &req.model, contents, InputType::Document).await?;

        let tx = self.pool.begin().await.map_err(db_err)?;

        // external_id -> id for edge resolution within this batch.
        let mut id_by_ext: HashMap<String, Uuid> = HashMap::new();
        let mut doc_ids: Vec<Uuid> = Vec::with_capacity(req.documents.len());

        // The document upserts stay raw parameterized SQL: the `embedding`
        // column takes a `$n::vector` bind and the `on conflict` target is the
        // partial unique index (`where external_id is not null`), neither of
        // which the entity insert API can express.
        for (doc, vec) in req.documents.iter().zip(vectors.iter()) {
            let lit = vector_literal(vec);
            let attrs = if doc.attributes.is_null() {
                Value::Object(Default::default())
            } else {
                doc.attributes.clone()
            };
            let id: Uuid = if let Some(ext) = &doc.external_id {
                let stmt = Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "insert into search_documents (collection, external_id, content, attributes, embedding) \
                     values ($1, $2, $3, $4, $5::vector) \
                     on conflict (collection, external_id) where external_id is not null \
                     do update set content = excluded.content, attributes = excluded.attributes, \
                       embedding = excluded.embedding, updated_at = now() \
                     returning id",
                    [
                        req.collection.clone().into(),
                        ext.clone().into(),
                        doc.content.clone().into(),
                        attrs.into(),
                        lit.into(),
                    ],
                );
                let row = tx
                    .query_one(stmt)
                    .await
                    .map_err(db_err)?
                    .ok_or_else(|| ApiError::Db("insert returning id produced no row".into()))?;
                let row_id: Uuid = row.try_get("", "id").map_err(db_err)?;
                id_by_ext.insert(ext.clone(), row_id);
                row_id
            } else {
                let stmt = Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "insert into search_documents (collection, content, attributes, embedding) \
                     values ($1, $2, $3, $4::vector) returning id",
                    [
                        req.collection.clone().into(),
                        doc.content.clone().into(),
                        attrs.into(),
                        lit.into(),
                    ],
                );
                let row = tx
                    .query_one(stmt)
                    .await
                    .map_err(db_err)?
                    .ok_or_else(|| ApiError::Db("insert returning id produced no row".into()))?;
                row.try_get("", "id").map_err(db_err)?
            };
            doc_ids.push(id);
        }

        // Resolve + upsert edges.
        let mut edge_count = 0usize;
        for (doc, src_id) in req.documents.iter().zip(doc_ids.iter()) {
            for edge in &doc.edges {
                let dst = match id_by_ext.get(&edge.to) {
                    Some(id) => *id,
                    None => match self.resolve_one(&tx, &req.collection, &edge.to).await? {
                        Some(id) => id,
                        None => continue, // unknown target — skip
                    },
                };
                upsert_edge(
                    &tx,
                    *src_id,
                    dst,
                    edge.relation.clone().unwrap_or_else(|| "related".into()),
                    edge.weight.unwrap_or(1.0),
                )
                .await?;
                edge_count += 1;
            }
        }

        tx.commit().await.map_err(db_err)?;
        Ok(IndexResponse { collection: req.collection, indexed: doc_ids.len(), edges: edge_count })
    }

    async fn resolve_one<C: ConnectionTrait>(
        &self,
        conn: &C,
        collection: &str,
        external_id: &str,
    ) -> Result<Option<Uuid>, ApiError> {
        search_documents::Entity::find()
            .select_only()
            .column(search_documents::Column::Id)
            .filter(search_documents::Column::Collection.eq(collection))
            .filter(search_documents::Column::ExternalId.eq(external_id))
            .into_tuple::<Uuid>()
            .one(conn)
            .await
            .map_err(db_err)
    }

    async fn resolve_ids(&self, collection: &str, externals: &[String]) -> Result<Vec<Uuid>, ApiError> {
        search_documents::Entity::find()
            .select_only()
            .column(search_documents::Column::Id)
            .filter(search_documents::Column::Collection.eq(collection))
            .filter(search_documents::Column::ExternalId.is_in(externals.iter().cloned()))
            .into_tuple::<Uuid>()
            .all(&self.pool)
            .await
            .map_err(db_err)
    }

    /// Run a candidate query (selecting `id`) built from a SQL string + ordered
    /// binds, returning ids in rank order. Raw SQL by design: these queries are
    /// dynamically composed (rendered filter predicates, optional clauses) and
    /// use pgvector/tsquery/pg_trgm operators the entity API cannot express.
    async fn candidates(&self, sql: &str, binds: Vec<Bound>) -> Result<Vec<Uuid>, ApiError> {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, to_values(&binds));
        let rows = self.pool.query_all(stmt).await.map_err(db_err)?;
        rows.iter()
            .map(|r| r.try_get::<Uuid>("", "id").map_err(db_err))
            .collect()
    }

    pub async fn query(&self, req: SearchRequest) -> Result<SearchResponse, ApiError> {
        let signals = req.signals.unwrap_or_default();
        let k = self.candidate_k;
        let mut ranked_lists: Vec<(String, f64, Vec<Uuid>)> = Vec::new();

        // 1. lexical
        if signals.lexical > 0.0 {
            let mut b = Vec::new();
            let c = push(&mut b, Bound::Text(req.collection.clone()));
            let q = push(&mut b, Bound::Text(req.query.clone()));
            let f = filters::render(&req.filters, &mut b)?;
            let lim = push(&mut b, Bound::Int(k as i64));
            let fw = if f.is_empty() { String::new() } else { format!(" and {f}") };
            let sql = format!(
                "select id from search_documents \
                 where collection = ${c} and content_tsv @@ websearch_to_tsquery('english', ${q}){fw} \
                 order by ts_rank(content_tsv, websearch_to_tsquery('english', ${q})) desc limit ${lim}"
            );
            ranked_lists.push(("lexical".into(), signals.lexical, self.candidates(&sql, b).await?));
        }

        // 2. trigram
        if signals.trigram > 0.0 {
            let mut b = Vec::new();
            let c = push(&mut b, Bound::Text(req.collection.clone()));
            let q = push(&mut b, Bound::Text(req.query.clone()));
            let f = filters::render(&req.filters, &mut b)?;
            let lim = push(&mut b, Bound::Int(k as i64));
            let fw = if f.is_empty() { String::new() } else { format!(" and {f}") };
            let sql = format!(
                "select id from search_documents \
                 where collection = ${c} and content % ${q}{fw} \
                 order by content <-> ${q} asc limit ${lim}"
            );
            ranked_lists.push(("trigram".into(), signals.trigram, self.candidates(&sql, b).await?));
        }

        // 3. semantic
        if signals.semantic > 0.0 {
            let qv = self
                .embed(&req.provider, &req.model, vec![req.query.clone()], InputType::Query)
                .await?;
            if let Some(vec) = qv.into_iter().next() {
                let mut b = Vec::new();
                let c = push(&mut b, Bound::Text(req.collection.clone()));
                let v = push(&mut b, Bound::Text(vector_literal(&vec)));
                let f = filters::render(&req.filters, &mut b)?;
                let lim = push(&mut b, Bound::Int(k as i64));
                let fw = if f.is_empty() { String::new() } else { format!(" and {f}") };
                let sql = format!(
                    "select id from search_documents \
                     where collection = ${c} and embedding is not null{fw} \
                     order by embedding <=> ${v}::vector asc limit ${lim}"
                );
                ranked_lists.push(("semantic".into(), signals.semantic, self.candidates(&sql, b).await?));
            }
        }

        // 5. graph
        if let Some(g) = &req.graph {
            if g.weight > 0.0 && !g.seeds.is_empty() {
                let seed_ids = self.resolve_ids(&req.collection, &g.seeds).await?;
                if !seed_ids.is_empty() {
                    let hops = g.max_hops.unwrap_or(self.max_hops).min(self.max_hops);
                    let mut b = Vec::new();
                    let s = push(&mut b, Bound::Uuids(seed_ids));
                    let mh = push(&mut b, Bound::Int(hops as i64));
                    let rel_clause = match &g.relation {
                        Some(rel) => {
                            let r = push(&mut b, Bound::Text(rel.clone()));
                            format!(" and e.relation = ${r}")
                        }
                        None => String::new(),
                    };
                    let c = push(&mut b, Bound::Text(req.collection.clone()));
                    let f = filters::render(&req.filters, &mut b)?;
                    let lim = push(&mut b, Bound::Int(k as i64));
                    let fw = if f.is_empty() { String::new() } else { format!(" and {f}") };
                    let sql = format!(
                        "with recursive reach(id, hops) as ( \
                            select id, 0 from search_documents where id = any(${s}) \
                            union all \
                            select e.dst_id, r.hops + 1 from reach r \
                              join search_edges e on e.src_id = r.id \
                              where r.hops < ${mh}{rel_clause} \
                         ) \
                         select d.id from reach \
                           join search_documents d on d.id = reach.id \
                           where reach.hops > 0 and d.collection = ${c}{fw} \
                           group by d.id order by min(reach.hops) asc limit ${lim}"
                    );
                    ranked_lists.push(("graph".into(), g.weight, self.candidates(&sql, b).await?));
                }
            }
        }

        // Reciprocal Rank Fusion.
        let mut scores: HashMap<Uuid, f64> = HashMap::new();
        let mut per_signal: HashMap<Uuid, BTreeMap<String, usize>> = HashMap::new();
        let mut signals_used: Vec<String> = Vec::new();
        for (name, weight, ids) in &ranked_lists {
            if !ids.is_empty() {
                signals_used.push(name.clone());
            }
            for (rank, id) in ids.iter().enumerate() {
                *scores.entry(*id).or_insert(0.0) += weight / (RRF_K + (rank as f64) + 1.0);
                per_signal.entry(*id).or_default().insert(name.clone(), rank + 1);
            }
        }

        let mut ranked: Vec<(Uuid, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Pool to fetch: enough for an optional rerank, else just top_k.
        let pool_n = if req.rerank.is_some() { RERANK_POOL.max(req.top_k) } else { req.top_k };
        let pool_ids: Vec<Uuid> = ranked.iter().take(pool_n).map(|(id, _)| *id).collect();
        let score_by_id: HashMap<Uuid, f64> = ranked.iter().cloned().collect();

        let mut hits = self.fetch_hits(&pool_ids, &score_by_id, &per_signal).await?;

        // Optional rerank stage over the fused pool.
        if let Some(rc) = &req.rerank {
            let provider = self.rerank.resolve(&rc.provider)?;
            let docs: Vec<String> = hits.iter().map(|h| h.content.clone()).collect();
            if !docs.is_empty() {
                let rr = RerankRequest {
                    query: req.query.clone(),
                    documents: docs,
                    model: rc.model.clone(),
                    top_n: Some(req.top_k),
                };
                let result = provider.rerank(&rr).await?;
                let mut reordered: Vec<Hit> = Vec::with_capacity(result.results.len());
                for r in result.results {
                    if let Some(h) = hits.get_mut(r.index) {
                        let mut hit = std::mem::replace(h, placeholder_hit());
                        hit.score = r.score as f64;
                        reordered.push(hit);
                    }
                }
                hits = reordered;
            }
        }

        hits.truncate(req.top_k);
        Ok(SearchResponse { collection: req.collection, signals_used, hits })
    }

    async fn fetch_hits(
        &self,
        ids: &[Uuid],
        score_by_id: &HashMap<Uuid, f64>,
        per_signal: &HashMap<Uuid, BTreeMap<String, usize>>,
    ) -> Result<Vec<Hit>, ApiError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(Uuid, Option<String>, String, Value)> = search_documents::Entity::find()
            .select_only()
            .columns([
                search_documents::Column::Id,
                search_documents::Column::ExternalId,
                search_documents::Column::Content,
                search_documents::Column::Attributes,
            ])
            .filter(search_documents::Column::Id.is_in(ids.iter().copied()))
            .into_tuple()
            .all(&self.pool)
            .await
            .map_err(db_err)?;

        let mut by_id: HashMap<Uuid, Hit> = HashMap::new();
        for (id, external_id, content, attributes) in rows {
            by_id.insert(
                id,
                Hit {
                    id: id.to_string(),
                    external_id,
                    content,
                    attributes,
                    score: *score_by_id.get(&id).unwrap_or(&0.0),
                    signals: per_signal.get(&id).cloned().unwrap_or_default(),
                },
            );
        }
        // Preserve the fused order from `ids`.
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    pub async fn add_edges(&self, req: AddEdgesRequest) -> Result<usize, ApiError> {
        let mut added = 0usize;
        let tx = self.pool.begin().await.map_err(db_err)?;
        for e in &req.edges {
            let src = self.resolve_one(&tx, &req.collection, &e.from).await?;
            let dst = self.resolve_one(&tx, &req.collection, &e.to).await?;
            let (Some(src), Some(dst)) = (src, dst) else { continue };
            upsert_edge(
                &tx,
                src,
                dst,
                e.relation.clone().unwrap_or_else(|| "related".into()),
                e.weight.unwrap_or(1.0),
            )
            .await?;
            added += 1;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(added)
    }

    pub async fn delete(&self, req: DeleteRequest) -> Result<u64, ApiError> {
        let res = search_documents::Entity::delete_many()
            .filter(search_documents::Column::Collection.eq(req.collection.as_str()))
            .filter(search_documents::Column::ExternalId.is_in(req.external_ids.iter().cloned()))
            .exec(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected)
    }

    pub async fn list_collections(&self) -> Result<Vec<String>, ApiError> {
        search_documents::Entity::find()
            .select_only()
            .column(search_documents::Column::Collection)
            .distinct()
            .order_by_asc(search_documents::Column::Collection)
            .into_tuple::<String>()
            .all(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn delete_collection(&self, collection: &str) -> Result<u64, ApiError> {
        let res = search_documents::Entity::delete_many()
            .filter(search_documents::Column::Collection.eq(collection))
            .exec(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected)
    }
}

/// Upsert one graph edge (`on conflict (src_id, dst_id, relation) do update
/// set weight = excluded.weight`), on a transaction or the pool.
async fn upsert_edge<C: ConnectionTrait>(
    conn: &C,
    src_id: Uuid,
    dst_id: Uuid,
    relation: String,
    weight: f64,
) -> Result<(), ApiError> {
    let edge = search_edges::ActiveModel {
        src_id: Set(src_id),
        dst_id: Set(dst_id),
        relation: Set(relation),
        weight: Set(weight),
    };
    search_edges::Entity::insert(edge)
        .on_conflict(
            OnConflict::columns([
                search_edges::Column::SrcId,
                search_edges::Column::DstId,
                search_edges::Column::Relation,
            ])
            .update_column(search_edges::Column::Weight)
            .to_owned(),
        )
        .exec_without_returning(conn)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn placeholder_hit() -> Hit {
    Hit {
        id: String::new(),
        external_id: None,
        content: String::new(),
        attributes: Value::Null,
        score: 0.0,
        signals: BTreeMap::new(),
    }
}
