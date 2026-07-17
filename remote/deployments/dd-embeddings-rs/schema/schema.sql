-- Canonical Postgres schema source for the dd-embeddings-rs search index.
--
-- This file is the desired-state contract for the service's OWN search
-- database (separate from the shared pg-defs RDS contract, like
-- billing-server-rs). It is the consolidated final state of
-- migrations/0001_init.sql; that frozen historical file remains under
-- migrations/ for audit only.
--
-- Why not remote/libs/pg-defs? The pg-defs adapter generators cannot
-- represent `vector`/`tsvector` columns (strict type switches reject them),
-- and that contract intentionally excludes pgvector tables (see the
-- des_soccer_moment_embeddings note in pg-defs schema/schema.sql). This
-- service therefore mirrors the billing-server-rs pattern: a service-local
-- declarative schema + scripts/dpm.sh.
--
-- Do not apply this file directly to a live database; generate and review a
-- migration with scripts/dpm.sh (dpm — declarative-postgres-migrate) instead.
-- The service never migrates at boot.
--
-- Prereqs: the database user must be able to CREATE EXTENSION `vector` and
-- `pg_trgm` (rds_superuser, or have an operator pre-create them), and the dpm
-- SHADOW_DATABASE_URL server must have both extensions installed so this file
-- can be materialized there. HNSW indexing needs pgvector >= 0.5.
--
-- The embedding column dimension (1536) is fixed here and must match the
-- service's EMBEDDINGS_SEARCH_DIM. Changing it requires a schema change + a
-- re-index of all rows.

create extension if not exists vector;
create extension if not exists pg_trgm;

create table if not exists search_documents (
  id           uuid primary key default gen_random_uuid(),
  collection   text not null,
  external_id  text,
  content      text not null,
  attributes   jsonb not null default '{}'::jsonb,
  embedding    vector(1536),
  -- Lexical vector maintained by Postgres, not the app.
  content_tsv  tsvector generated always as (to_tsvector('english', coalesce(content, ''))) stored,
  created_at   timestamptz not null default now(),
  updated_at   timestamptz not null default now(),
  constraint search_documents_attrs_object_chk check (jsonb_typeof(attributes) = 'object')
);

-- One logical document per (collection, external_id) so re-indexing upserts.
create unique index if not exists search_documents_collection_extid_uq
  on search_documents (collection, external_id)
  where external_id is not null;

create index if not exists search_documents_collection_idx
  on search_documents (collection);

-- 1. Lexical (same words).
create index if not exists search_documents_tsv_idx
  on search_documents using gin (content_tsv);

-- 2. Trigram (same characters).
create index if not exists search_documents_trgm_idx
  on search_documents using gin (content gin_trgm_ops);

-- 4. Structured (same attributes) — containment-optimized JSONB index.
create index if not exists search_documents_attrs_idx
  on search_documents using gin (attributes jsonb_path_ops);

-- 3. Semantic (same meaning) — cosine HNSW.
create index if not exists search_documents_embedding_idx
  on search_documents using hnsw (embedding vector_cosine_ops);

-- 5. Graph (same relationships).
create table if not exists search_edges (
  src_id   uuid not null references search_documents(id) on delete cascade,
  dst_id   uuid not null references search_documents(id) on delete cascade,
  relation text not null default 'related',
  weight   double precision not null default 1.0,
  primary key (src_id, dst_id, relation)
);

create index if not exists search_edges_dst_idx
  on search_edges (dst_id, relation);
