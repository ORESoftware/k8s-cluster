-- ---------------------------------------------------------------------------
-- dd-fsharp-ws-server data plane.
--
-- This schema is deliberately disjoint from dd-gleamlang-presence-server's
-- `presence_*` schema (table, trigger, publication, slot, NOTIFY channels,
-- NATS subjects) so the F# demo can run in parallel without coordinating LSN
-- progression or NOTIFY channel ownership with another consumer.
--
-- Three parallel ingest paths converge on the same `UnifiedEvent` inside the
-- F# server's PresenceFanIn module:
--
--   1. LISTEN/NOTIFY  — sub-ms latency, fire-and-forget.   (PgListen.fs)
--   2. Outbox poll    — durable, app-controlled.           (PgOutbox.fs)
--   3. WAL CDC        — schema-agnostic, slot-retained.    (PgWal.fs)
--
-- All three are idempotent on `event_id`; the dedup cache in PresenceFanIn
-- absorbs overlap.
--
-- Boot-time migrator lives in PgSchema.fs and runs this file verbatim via
-- `npgsql.ExecuteNonQuery`. Every statement is idempotent — safe to run on
-- every pod start.
-- ---------------------------------------------------------------------------

-- 1. Event table -------------------------------------------------------------

CREATE TABLE IF NOT EXISTS fsws_events (
    seq          BIGSERIAL    PRIMARY KEY,
    event_id     UUID         NOT NULL UNIQUE,
    kind         TEXT         NOT NULL,
    conv_id      UUID         NOT NULL,
    payload      JSONB        NOT NULL,
    occurred_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    soft_deleted BOOLEAN      NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS fsws_events_conv_seq_idx
    ON fsws_events (conv_id, seq);

-- Outbox polling reads in `occurred_at` order with `seq` as a tiebreaker.
-- A partial index keeps the hot path tight even as the table grows.
CREATE INDEX IF NOT EXISTS fsws_events_occurred_idx
    ON fsws_events (occurred_at, seq)
    WHERE soft_deleted = false;


-- 2. Sharded NOTIFY trigger --------------------------------------------------
--
-- Sixteen-way shard mirrors what `presence_change_*` uses. Payload is a JSON
-- object with just enough fields for the consumer to fetch the full row
-- if it doesn't already have it cached.

CREATE OR REPLACE FUNCTION fsws_shard_of(conv UUID)
RETURNS INT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS
$$ SELECT abs(hashtext(conv::text)) % 16 $$;

CREATE OR REPLACE FUNCTION fsws_notify_event_change()
RETURNS trigger LANGUAGE plpgsql AS
$$
DECLARE
    payload  text;
    chan     text;
BEGIN
    chan := 'fsws_change_' || fsws_shard_of(NEW.conv_id);

    payload := json_build_object(
        'event_id',    NEW.event_id,
        'seq',         NEW.seq,
        'kind',        NEW.kind,
        'conv_id',     NEW.conv_id,
        'occurred_at', NEW.occurred_at,
        'soft_deleted',NEW.soft_deleted
    )::text;

    -- NOTIFY payloads are capped at 8000 bytes (1 page). Our payload is
    -- small (~200 bytes), but if a future schema change pushes us over,
    -- this is where to chunk / move to NATS-only.
    PERFORM pg_notify(chan, payload);

    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS fsws_events_notify ON fsws_events;
CREATE TRIGGER fsws_events_notify
    AFTER INSERT OR UPDATE ON fsws_events
    FOR EACH ROW EXECUTE FUNCTION fsws_notify_event_change();


-- 3. Logical replication publication ----------------------------------------
--
-- `CREATE PUBLICATION ... IF NOT EXISTS` doesn't exist in any released PG,
-- so we guard with a DO block. Restricted to the one table the F# server
-- owns — same posture as `presence_pub`.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_publication WHERE pubname = 'fsws_pub'
    ) THEN
        CREATE PUBLICATION fsws_pub FOR TABLE fsws_events;
    END IF;
END
$$;


-- 4. WAL slot helpers --------------------------------------------------------
--
-- The F# server calls `fsws_ensure_wal_slot('fsws_wal_<machine>')` on every
-- boot. Idempotent: only creates the slot if it doesn't already exist for
-- this pod identity.
--
-- We use wal2json because the cluster already has it installed for the
-- presence server. If a future deploy switches to a PG without wal2json,
-- `fsws_wal_available()` returns false and PgWal silently skips
-- itself — the LISTEN/NOTIFY + outbox paths still cover everything.

CREATE OR REPLACE FUNCTION fsws_wal_available()
RETURNS boolean LANGUAGE sql STABLE AS
$$
    SELECT EXISTS (
        SELECT 1 FROM pg_available_extensions WHERE name = 'wal2json'
    )
    AND current_setting('wal_level') = 'logical'
$$;

CREATE OR REPLACE FUNCTION fsws_ensure_wal_slot(p_slot_name text)
RETURNS void LANGUAGE plpgsql AS
$$
BEGIN
    IF NOT fsws_wal_available() THEN
        RAISE NOTICE 'fsws_ensure_wal_slot: wal2json or logical wal_level missing; skipping slot creation';
        RETURN;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_replication_slots WHERE slot_name = p_slot_name
    ) THEN
        PERFORM pg_create_logical_replication_slot(p_slot_name, 'wal2json');
        RAISE NOTICE 'fsws_ensure_wal_slot: created slot %', p_slot_name;
    END IF;
END;
$$;


-- 5. Convenience writer ------------------------------------------------------
--
-- Wraps the INSERT so the F# /ws/rx-publish handler can call a single SQL
-- function instead of building a parameterised INSERT in code. Returns the
-- inserted row so the WS reply can echo back `seq` and `occurred_at`.

CREATE OR REPLACE FUNCTION fsws_publish_event(
    p_event_id  UUID,
    p_kind      TEXT,
    p_conv_id   UUID,
    p_payload   JSONB
) RETURNS TABLE (
    seq         BIGINT,
    event_id    UUID,
    occurred_at TIMESTAMPTZ
) LANGUAGE sql AS
$$
    INSERT INTO fsws_events (event_id, kind, conv_id, payload)
    VALUES (p_event_id, p_kind, p_conv_id, p_payload)
    ON CONFLICT (event_id) DO NOTHING
    RETURNING seq, event_id, occurred_at;
$$;
