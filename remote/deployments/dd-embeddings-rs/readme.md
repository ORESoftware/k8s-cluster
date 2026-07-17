# dd-embeddings-rs

A Rust HTTP service that fronts every embedding provider we use behind one
normalized API, plus a thin RAG indexing layer over the cluster's Qdrant.

Two jobs:

1. **Embedding generation** — `POST /api/embeddings` with a provider + input,
   get back vectors in a single shape regardless of which upstream served them.
2. **RAG indexing** — `POST /api/rag/index` embeds documents and upserts them
   into a Qdrant collection; `POST /api/rag/search` embeds a query and returns
   nearest neighbors.

## Providers

The roster is **opt-in**: a provider is registered at boot only if its API key
is present in the secret (self-hosted upstreams need no key). The pod boots
fine with zero providers and `/api/providers` reports what's live.

| id          | upstream                         | wire format       | key env                 |
|-------------|----------------------------------|-------------------|-------------------------|
| `openai`    | OpenAI                           | OpenAI            | `OPENAI_API_KEY`        |
| `voyage`    | Voyage AI                        | OpenAI-ish        | `VOYAGE_API_KEY`        |
| `gemini`    | Google Gemini                    | Google            | `GEMINI_API_KEY`        |
| `cohere`    | Cohere                           | Cohere            | `COHERE_API_KEY`        |
| `mistral`   | Mistral                          | OpenAI            | `MISTRAL_API_KEY`       |
| `jina`      | Jina AI                          | OpenAI            | `JINA_API_KEY`          |
| `together`  | Together AI                      | OpenAI            | `TOGETHER_API_KEY`      |
| `fireworks` | Fireworks AI                     | OpenAI            | `FIREWORKS_API_KEY`     |
| `deepinfra` | DeepInfra                        | OpenAI            | `DEEPINFRA_API_KEY`     |
| `nomic`     | Nomic Atlas                      | OpenAI            | `NOMIC_API_KEY`         |
| `azure`     | Azure OpenAI                     | OpenAI (`api-key`)| `AZURE_OPENAI_API_KEY` + `AZURE_BASE_URL` |
| `tei`       | self-hosted HF TEI (`ai-ml` ns)  | OpenAI            | none                    |
| `ollama`    | self-hosted Ollama (`ai-ml` ns)  | OpenAI            | none                    |
| `upstage`   | Upstage (Solar)                  | OpenAI            | `UPSTAGE_API_KEY`       |
| `siliconflow`| SiliconFlow                     | OpenAI            | `SILICONFLOW_API_KEY`   |
| `github`    | GitHub Models                    | OpenAI            | `GITHUB_MODELS_TOKEN`   |
| `cloudflare`| Cloudflare Workers AI            | OpenAI            | `CLOUDFLARE_API_TOKEN` + `CLOUDFLARE_BASE_URL` |
| `databricks`| Databricks serving endpoints     | OpenAI            | `DATABRICKS_TOKEN` + `DATABRICKS_BASE_URL` |
| `vllm`      | self-hosted vLLM (`ai-ml` ns)    | OpenAI            | none                    |
| `infinity`  | self-hosted Infinity (`ai-ml` ns)| OpenAI            | none                    |

### About "Anthropic"

There is no first-party Anthropic embeddings API — Anthropic's own
documentation recommends **Voyage AI** for embeddings. So `provider:
"anthropic"` is registered as an **alias for `voyage`** (only when a Voyage key
is configured). Ask for `anthropic` and you get a Voyage vector.

## API

All `/api/*` functional routes require `Authorization: Bearer <token>` when
`EMBEDDINGS_API_AUTH_BEARER` is set (it is, in-cluster, from the secret).
Probes and docs are always public. Generated docs: `GET /api/docs.json`,
`GET /api/docs`, `GET /docs/api`.

```bash
# Generate embeddings
curl -s localhost:8090/api/embeddings -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "provider": "openai",
    "model": "text-embedding-3-small",
    "input": ["hello world", "second doc"],
    "dimensions": 512
  }'

# Index documents into Qdrant
curl -s localhost:8090/api/rag/index -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "collection": "kb",
    "provider": "voyage",
    "documents": [
      { "id": "doc-1", "text": "Qdrant is a vector database.", "metadata": {"src": "wiki"} },
      { "text": "Embeddings map text to vectors." }
    ]
  }'

# Search
curl -s localhost:8090/api/rag/search -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "collection": "kb",
    "provider": "voyage",
    "query": "what is a vector store?",
    "top_k": 3
  }'
```

`input_type` (`document` | `query`) controls the retrieval sub-space for
asymmetric models (Voyage, Cohere, Gemini, Nomic). The RAG layer sets it
automatically: `document` on index, `query` on search.

## Reranking

Second-stage relevance scoring for RAG — a cross-encoder ranks candidates far
more accurately than embedding cosine alone. Providers: `cohere`, `jina`,
`voyage` (and `anthropic` → `voyage`); they reuse the same API keys as the
embedding side, so no extra secrets.

```bash
curl -s localhost:8090/api/rerank -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "provider": "cohere",
    "query": "how do vector databases work?",
    "documents": ["Qdrant stores vectors.", "Bananas are yellow.", "ANN search finds neighbors."],
    "top_n": 2
  }'
# → { "provider": "cohere", "model": "rerank-v3.5",
#     "results": [ { "index": 0, "score": 0.98 }, { "index": 2, "score": 0.81 } ] }
```

## RAG collection management

```bash
curl -s localhost:8090/api/rag/collections -H "Authorization: Bearer $TOK"          # list
curl -s -X DELETE localhost:8090/api/rag/collections/kb -H "Authorization: Bearer $TOK"  # drop a collection
curl -s localhost:8090/api/rag/delete -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{ "collection": "kb", "ids": ["doc-1"] }'  # delete points
```

Caller-supplied ids are normalized the same way on index and delete, so the id
you indexed with is the id you delete with.

## Multi-signal search (Postgres)

A search engine over Postgres that fuses the five retrieval signals — think a
small Elasticsearch built on `tsvector` + `pg_trgm` + `pgvector` + JSONB + a
graph table:

| Signal | Question | Mechanism |
|--------|----------|-----------|
| Lexical | "Did they say the same thing?" | `tsvector` / `ts_rank` |
| Trigram | "Did they type the same thing?" | `pg_trgm` `<->` |
| Semantic | "Did they mean the same thing?" | `pgvector` cosine `<=>` |
| Structured | "Does it satisfy the constraints?" | JSONB / typed predicate filters |
| Graph | "Is it connected to the same things?" | `search_edges` + recursive CTE |

The text signals each produce a ranked candidate list; they're merged with
**Reciprocal Rank Fusion**, structured filters are hard constraints on every
signal, the graph contributes an additional ranked list from seed documents,
and an optional **rerank** stage reorders the fused top-N.

This subsystem is **optional** and **owns its own database** (separate from the
shared `pg-defs` contract, like `billing-server-rs`). It activates only when
`DATABASE_URL` is set; otherwise `/api/search/*` returns 503
`search_not_configured` and the rest of the service is unaffected. Migrations
(`migrations/0001_init.sql`) run on boot when `SEARCH_RUN_MIGRATIONS=true` and
require the DB user to `CREATE EXTENSION vector, pg_trgm` (pgvector ≥ 0.5 for
HNSW). The embedding column is fixed at `EMBEDDINGS_SEARCH_DIM` (default 1536).

```bash
# Index documents (content + structured attributes + graph edges)
curl -s localhost:8090/api/search/index -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "collection": "products", "provider": "openai",
    "documents": [
      { "external_id": "p1", "content": "waterproof hiking boots",
        "attributes": { "price": 129, "waterproof": true, "type": "boots" },
        "edges": [ { "to": "p2", "relation": "related" } ] },
      { "external_id": "p2", "content": "wool hiking socks",
        "attributes": { "price": 18, "type": "socks" } }
    ]
  }'

# Hybrid query: all signals + structured filter + graph seed + rerank
curl -s localhost:8090/api/search -H "Authorization: Bearer $TOK" \
  -H 'content-type: application/json' -d '{
    "collection": "products", "provider": "openai",
    "query": "cheap waterproof hiking boots",
    "signals": { "lexical": 1.0, "trigram": 0.5, "semantic": 1.0 },
    "filters": { "price": { "lt": 150 }, "waterproof": true },
    "graph": { "seeds": ["p1"], "max_hops": 2 },
    "top_k": 10,
    "rerank": { "provider": "cohere" }
  }'
# → { "collection": "products", "signals_used": ["lexical","semantic","graph"],
#     "hits": [ { "external_id": "p1", "score": ..., "signals": {"lexical":1,"semantic":1}, ... } ] }
```

**Structured-filter DSL** (over the `attributes` JSONB) — every value is a bound
parameter, field names are charset-validated, so no SQL injection:
`{ "field": value }` (eq) or `{ "field": { "op": value } }` with
`eq/ne/gt/gte/lt/lte/in/contains/exists`.

Also: `POST /api/search/edges` (bulk edges), `POST /api/search/delete` (by
external_id), `GET /api/search/collections`, `DELETE /api/search/collections/{c}`.

Behavioral (collaborative-filtering) and generative retrieval are out of scope;
the fusion layer is built so another signal can be added without reworking the rest.

## Caching & metrics

Identical embeddings are served from a bounded in-memory cache
(`EMBEDDINGS_CACHE_MAX_ENTRIES` / `EMBEDDINGS_CACHE_MAX_ITEM_BYTES`), so
repeated text and re-indexing cost nothing and make no provider call. Prometheus
metrics (request/error/cache/per-provider counters) are exposed at `/metrics`
for the cluster scraper.

## Configuration

| env                              | default                                              |
|----------------------------------|------------------------------------------------------|
| `EMBEDDINGS_HOST`                | `0.0.0.0`                                             |
| `EMBEDDINGS_PORT`                | `8090`                                               |
| `EMBEDDINGS_LOG_FORMAT`          | `json`                                               |
| `EMBEDDINGS_API_AUTH_BEARER`     | unset (API open if unset)                            |
| `EMBEDDINGS_REQUEST_TIMEOUT_SECS`| `30`                                                 |
| `QDRANT_URL`                     | `http://dd-qdrant.ai-ml.svc.cluster.local:6333`      |
| `QDRANT_API_KEY`                 | unset                                                |
| `<PROVIDER>_BASE_URL`            | per-provider override (e.g. `AZURE_BASE_URL`)        |
| `EMBEDDINGS_MAX_BATCH_SIZE`      | `256` — max texts per request                        |
| `EMBEDDINGS_MAX_TOTAL_CHARS`     | `1000000` — max chars summed across a request        |
| `EMBEDDINGS_MAX_ITEM_CHARS`      | `100000` — max chars in any single text              |
| `EMBEDDINGS_MAX_TOP_K`           | `100` — search results are clamped to this           |
| `EMBEDDINGS_MAX_DIMENSIONS`      | `8192` — max requested dimensionality                |
| `EMBEDDINGS_MAX_CONCURRENCY`     | `32` — in-flight cost-bearing requests; excess → 503 |
| `EMBEDDINGS_CACHE_MAX_ENTRIES`   | `50000` — embedding cache size (0 disables)          |
| `EMBEDDINGS_CACHE_MAX_ITEM_BYTES`| `8192` — only cache texts at or below this length    |
| `DATABASE_URL`                   | unset — enables the Postgres search subsystem        |
| `SEARCH_RUN_MIGRATIONS`          | `true` — run search migrations on boot               |
| `EMBEDDINGS_SEARCH_DIM`          | `1536` — search index embedding dim (matches migration) |
| `EMBEDDINGS_SEARCH_CANDIDATE_K`  | `200` — per-signal candidate pool before fusion      |
| `EMBEDDINGS_SEARCH_MAX_HOPS`     | `4` — graph traversal hop cap                        |

## Security posture

- **Auth**: `/api/*` functional routes require a bearer token (constant-time
  compared) when `EMBEDDINGS_API_AUTH_BEARER` is set; the service logs a loud
  warning at boot if it isn't. Probes and docs are always public.
- **Input guardrails**: batch count, per-item and total character counts,
  `top_k`, and `dimensions` are all bounded (see the limits above) to cap
  memory use and third-party spend. Qdrant collection names are charset- and
  length-validated before they reach a REST path; the distance metric is
  allow-listed; model names are charset-validated (they're interpolated into
  the Gemini request URL, so this blocks path/query injection there) and the
  Gemini API key is sent as a header, never a URL query param; blank input
  items are rejected; caller-supplied document ids are normalized to valid Qdrant
  point ids (UUID/uint passthrough, else a stable UUIDv5) with the original
  preserved as `source_id` in the payload.
- **Load shedding**: a global semaphore bounds concurrent cost-bearing
  requests (`EMBEDDINGS_MAX_CONCURRENCY`); a saturated server returns 503
  immediately rather than fanning out unbounded outbound calls.
- **Resilience**: handler panics are caught and returned as clean 500s (no
  connection drop or backtrace leak); SIGTERM triggers a graceful drain so
  rolling deploys don't abort already-paid provider calls mid-flight.
- **Config hygiene**: `.env` is read only in debug builds — the in-cluster
  release binary takes config and secrets solely from the real environment.
  Every response carries `X-Content-Type-Options: nosniff`; the one HTML
  surface (`/api/docs`) reflects no user input.
- **No detail leakage**: upstream provider / Qdrant error bodies are logged
  server-side but never echoed to callers — responses carry a stable `kind`
  plus a generic message.
- **Egress**: the outbound HTTP client does not follow redirects (anti-SSRF),
  and the NetworkPolicy `except`s RFC-1918 + link-local ranges on internet
  egress, so the pod cannot reach internal services or the
  `169.254.169.254` instance-metadata endpoint even under a hypothetical SSRF.
  Ingress is restricted to the gateway and observability scrapers; the
  container drops all Linux capabilities.

## Deployment

Same pattern as the other in-cluster Rust services: ArgoCD app
`remote/argocd/apps/dd-embeddings-rs.application.yaml` syncs
`k8s/ec2/` (kustomize). The container builds the release binary in-cluster
from the hostPath-mounted repo on first boot (long startup probe budget),
then `exec`s it. Secrets come from AWS Secrets Manager via ESO
(`dd/remote-dev/embeddings-rs-secrets`).

The `Dockerfile` builds a standalone slim image for environments that prefer a
pushed artifact over the in-cluster build.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
