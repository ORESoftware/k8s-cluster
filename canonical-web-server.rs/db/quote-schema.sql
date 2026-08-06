-- Canonical quote/context schema.
-- `canonical_context` is the physical SQL identifier for the logical
-- "canonical-context" record set; snake_case avoids requiring quoted names in
-- every SeaORM query and policy.
--
-- `compliance_quote.owner_id` is a provider-neutral Shared Auth principal UUID,
-- not necessarily a row in this product's Supabase `auth.users` table. Identity
-- proof happens before the request enters the transaction; RLS then binds the
-- row to `request.jwt.claim.sub` through `auth.uid()`.

CREATE TABLE IF NOT EXISTS canonical_context (
  id uuid PRIMARY KEY,
  context_key text NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  context_markdown text NOT NULL CHECK (octet_length(context_markdown) <= 65536),
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (context_key, version)
);

CREATE INDEX IF NOT EXISTS canonical_context_active_key_idx
  ON canonical_context (context_key, active, version DESC);

CREATE TABLE IF NOT EXISTS compliance_quote (
  id uuid PRIMARY KEY,
  owner_id uuid NOT NULL,
  status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
  request jsonb NOT NULL CHECK (jsonb_typeof(request) = 'object'),
  analysis jsonb CHECK (analysis IS NULL OR jsonb_typeof(analysis) = 'object'),
  model text NOT NULL CHECK (length(model) BETWEEN 1 AND 128),
  failure_code text CHECK (failure_code IS NULL OR length(failure_code) <= 100),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((status = 'ready') = (analysis IS NOT NULL)),
  CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE INDEX IF NOT EXISTS compliance_quote_owner_created_idx
  ON compliance_quote (owner_id, created_at DESC);
CREATE INDEX IF NOT EXISTS compliance_quote_owner_status_idx
  ON compliance_quote (owner_id, status, updated_at DESC);

ALTER TABLE canonical_context ENABLE ROW LEVEL SECURITY;
ALTER TABLE canonical_context FORCE ROW LEVEL SECURITY;
ALTER TABLE compliance_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE compliance_quote FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS canonical_context_runtime_read ON canonical_context;
CREATE POLICY canonical_context_runtime_read ON canonical_context
  FOR SELECT
  USING (current_user = 'canonical_web_server' AND active = true);

DROP POLICY IF EXISTS compliance_quote_owner ON compliance_quote;
CREATE POLICY compliance_quote_owner ON compliance_quote
  USING (owner_id = auth.uid())
  WITH CHECK (owner_id = auth.uid());

INSERT INTO canonical_context (
  id, context_key, version, context_markdown, active
) VALUES (
  'f15acb30-d99b-4f85-a0bc-b50a7db66b65',
  'quote-analysis',
  1,
  $context$
Use the current Canonical delivery catalog and rate card maintained by the
compliance team. Estimates must separate Canonical readiness/remediation work,
independent assessor or 3PAO fees, cloud/vendor pass-through costs, and optional
ongoing vCISO support. Mark any unavailable rate-card input as missing rather
than inventing a number.
$context$,
  true
)
ON CONFLICT (context_key, version) DO NOTHING;
