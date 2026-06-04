-- billing-server-rs :: notifications.
--
-- A "rule" says: when condition X is true for entity Y in tenant T, send a
-- message via channel Z to target W. Conditions are evaluated by a scheduled
-- job (notifications.evaluate_rules) that runs every N minutes per tenant.
--
-- A "dispatch" is the record of one outbound send. Throttling and dedupe
-- happen against this table (e.g. don't send more than 1 "overdue" notice
-- per customer per day).

CREATE TYPE notification_channel AS ENUM ('email', 'webhook', 'slack', 'sms');

CREATE TYPE notification_dispatch_status AS ENUM (
    'pending', 'sending', 'sent', 'failed', 'throttled', 'suppressed'
);

CREATE TABLE notification_rules (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key       BIGINT NOT NULL,
    kind            TEXT NOT NULL,
    -- e.g. "balance_negative", "payment_overdue", "payment_received",
    --      "reconciliation_break_opened", "lease_held_too_long"
    name            TEXT NOT NULL,
    params          JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Channel + target ("alice@example.com" / "https://.../webhook" / "#billing-alerts")
    channel         notification_channel NOT NULL,
    target          TEXT NOT NULL,
    -- Per-channel auth/signing material, sealed in the same envelope shape
    -- as provider credentials. Optional; webhook channel uses it for HMAC,
    -- email channel uses it for provider api key, etc.
    sealed_credential JSONB,
    template_id     TEXT,                       -- opaque ref to a template store; defaults baked in
    throttle_per_day INT NOT NULL DEFAULT 1,    -- max dispatches per (rule, target_resource, day)
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, kind, name)
);

CREATE INDEX notification_rules_tenant_idx ON notification_rules (tenant_id);
CREATE INDEX notification_rules_shard_idx ON notification_rules (shard_key);

CREATE TABLE notification_dispatches (
    id                  BIGSERIAL PRIMARY KEY,
    rule_id             UUID NOT NULL REFERENCES notification_rules(id) ON DELETE CASCADE,
    tenant_id           UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL,
    -- The thing the dispatch is about, e.g. user_id or invoice_id.
    target_resource     TEXT,
    channel             notification_channel NOT NULL,
    target              TEXT NOT NULL,
    payload             JSONB NOT NULL,
    status              notification_dispatch_status NOT NULL DEFAULT 'pending',
    provider_message_id TEXT,
    error               TEXT,
    sent_at             TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX notification_dispatches_tenant_idx
    ON notification_dispatches (tenant_id, created_at DESC);
CREATE INDEX notification_dispatches_rule_idx
    ON notification_dispatches (rule_id, created_at DESC);

-- Throttler lookup: one rule/resource over a UTC day range.
CREATE INDEX notification_dispatches_day_idx
    ON notification_dispatches (rule_id, target_resource, created_at DESC)
    WHERE status IN ('sent', 'pending', 'sending');
