// Cross-submodule contract test for the local-first sync path.
//
// The sync contract is defined in THREE repos that must agree:
//   - apps/fiducia-interfaces/sql/customer.sql  -- the Postgres source of truth
//     (`version`/`updated_at` columns + the `bump_row_version` trigger) and the
//     documented transport-neutral shape `{table, op, id, version, row}`.
//   - apps/fiducia-sync/src/lib.rs              -- the Rust reconcile core that
//     defines the `ChangeEvent` envelope, including the optional backend-only
//     `write_key` echo discriminator, and the lowercase `ChangeOp` enum.
//   - apps/fiducia-sync/sdk/src/transports/decode.mjs -- the TS/JS transport
//     decoders that turn Supabase-realtime / backend-WS frames into that same
//     ChangeEvent shape.
//
// A drift between them (a renamed field, a dropped column, a changed op value)
// silently breaks sync, so this asserts they stay coherent. Pure `node --test`:
// it only reads files and dynamically imports the pure JS decoder. Requires the
// app submodules to be checked out (CI does `submodules: recursive`); when they
// are absent it skips with an actionable message rather than crashing.

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

const root = path.resolve(import.meta.dirname, "..");

const CUSTOMER_SQL = "apps/fiducia-interfaces/sql/customer.sql";
const SYNC_README = "apps/fiducia-sync/README.md";
const SYNC_LIB = "apps/fiducia-sync/src/lib.rs";
const SYNC_DECODE = "apps/fiducia-sync/sdk/src/transports/decode.mjs";

// Every transport carries these fields. The backend may additionally echo the
// client-minted write identity; Supabase CDC cannot synthesize that identity.
const REQUIRED_CHANGE_EVENT_FIELDS = ["at_ms", "id", "op", "row", "table", "version"];
const RUST_CHANGE_EVENT_FIELDS = [...REQUIRED_CHANGE_EVENT_FIELDS, "write_key"].sort();

const missing = [CUSTOMER_SQL, SYNC_README, SYNC_LIB, SYNC_DECODE].filter(
  (rel) => !existsSync(path.join(root, rel)),
);
const skip = missing.length
  ? `app submodules not checked out (missing: ${missing.join(", ")}); ` +
    `run: git submodule update --init --recursive`
  : false;

function read(rel) {
  return readFileSync(path.join(root, rel), "utf8");
}

test("interfaces customer.sql declares the version/updated_at sync contract", { skip }, () => {
  const sql = read(CUSTOMER_SQL);

  // The two per-row sync columns.
  assert.match(sql, /\bversion\s+bigint\b/, "customer.sql must define a `version bigint` column");
  assert.match(sql, /\bupdated_at\s+timestamptz\b/, "customer.sql must define an `updated_at timestamptz` column");

  // The trigger that advances BOTH monotonically on every write.
  assert.match(sql, /bump_row_version/, "customer.sql must define/use the bump_row_version trigger");
  assert.match(sql, /new\.version\s*:=\s*old\.version\s*\+\s*1/, "bump_row_version must increment version");
  assert.match(sql, /new\.updated_at\s*:=\s*now\(\)/, "bump_row_version must stamp updated_at");

  // The documented change-event shape the sync engine ships.
  assert.match(
    sql,
    /\{\s*table\s*,\s*op\s*,\s*id\s*,\s*version\s*,\s*row\s*\}/,
    "customer.sql must document the {table, op, id, version, row} change-event shape",
  );
});

test("fiducia-sync Rust core defines the ChangeEvent envelope and lowercase ChangeOp", { skip }, () => {
  const lib = read(SYNC_LIB);

  const struct = lib.match(/pub struct ChangeEvent\s*\{([\s\S]*?)\}/);
  assert.ok(struct, "src/lib.rs must define `pub struct ChangeEvent`");
  const fields = [...struct[1].matchAll(/pub\s+(\w+)\s*:/g)].map((m) => m[1]).sort();
  assert.deepEqual(fields, RUST_CHANGE_EVENT_FIELDS, "ChangeEvent must include the required envelope and optional write identity");

  // version is the monotonic ordering key (i64).
  assert.match(struct[1], /pub\s+version\s*:\s*i64/, "ChangeEvent.version must be i64");
  assert.match(struct[1], /pub\s+write_key\s*:\s*Option<String>/, "ChangeEvent.write_key must remain optional");

  // op is a lowercase-serialized Upsert|Delete enum.
  const op = lib.match(/pub enum ChangeOp\s*\{([\s\S]*?)\}/);
  assert.ok(op, "src/lib.rs must define `pub enum ChangeOp`");
  assert.match(op[1], /\bUpsert\b/);
  assert.match(op[1], /\bDelete\b/);
  assert.match(lib, /rename_all\s*=\s*"lowercase"/, "ChangeOp must serialize lowercase (upsert/delete)");

  // The core ties its ordering key back to the interfaces trigger.
  assert.match(lib, /bump_row_version/, "lib.rs must reference the bump_row_version ordering key");
  assert.match(lib, /fiducia-interfaces/, "lib.rs must name fiducia-interfaces as the trigger source");
});

test("fiducia-sync README ties the sync fields to fiducia-interfaces and both transports", { skip }, () => {
  const readme = read(SYNC_README);

  assert.match(readme, /\bversion\b/, "README must name the version ordering key");
  assert.match(readme, /bump_row_version/, "README must reference the bump_row_version trigger");
  assert.match(readme, /fiducia-interfaces/, "README must point at fiducia-interfaces as the contract source");
  assert.match(readme, /ChangeEvent/, "README must name the ChangeEvent envelope");
  // Local-first sync must never depend on a single channel.
  assert.match(readme, /Supabase/, "README must document the Supabase-realtime transport");
  assert.match(readme, /\bWS\b/, "README must document the backend WS transport");
});

test("decode.mjs exports the transport decoders that speak the ChangeEvent shape", { skip }, async () => {
  const mod = await import(pathToFileURL(path.join(root, SYNC_DECODE)).href);

  for (const name of ["isChangeEvent", "decodeBackendMessage", "decodeSupabaseChange"]) {
    assert.equal(typeof mod[name], "function", `decode.mjs must export ${name}()`);
  }

  const event = { table: "api_keys", op: "upsert", id: "k1", version: 7, row: { name: "prod" }, at_ms: 123 };

  // The guard accepts a well-formed envelope and rejects contract violations.
  assert.equal(mod.isChangeEvent(event), true);
  assert.equal(mod.isChangeEvent({ ...event, write_key: "write-k1" }), true, "backend echoes may include a write identity");
  assert.equal(mod.isChangeEvent({ ...event, op: "patch" }), false, "op must be upsert|delete");
  assert.equal(mod.isChangeEvent({ ...event, version: "7" }), false, "version must be a number");
  assert.equal(mod.isChangeEvent({ ...event, write_key: 7 }), false, "write_key must be a string when present");

  // Backend WS/SSE frame -> ChangeEvent[]; non-sync frames decode to [].
  const frame = JSON.stringify({ event: "fiducia:sync", changes: [event] });
  assert.deepEqual(mod.decodeBackendMessage(frame), [event]);
  assert.deepEqual(mod.decodeBackendMessage(JSON.stringify({ event: "other" })), []);

  // Supabase postgres_changes payload -> ChangeEvent with the canonical fields.
  const upsert = mod.decodeSupabaseChange("api_keys", { eventType: "INSERT", new: { id: "k1", version: 7 } });
  assert.deepEqual(Object.keys(upsert).sort(), REQUIRED_CHANGE_EVENT_FIELDS, "Supabase events must contain the required envelope");
  assert.equal(upsert.op, "upsert");
  const del = mod.decodeSupabaseChange("api_keys", { eventType: "DELETE", old: { id: "k1", version: 8 } });
  assert.equal(del.op, "delete");
});

test("the ChangeEvent field set agrees across the Rust core, the JS decoder, and the SQL doc", { skip }, async () => {
  // Rust struct fields, parsed from source.
  const struct = read(SYNC_LIB).match(/pub struct ChangeEvent\s*\{([\s\S]*?)\}/);
  const rustFields = [...struct[1].matchAll(/pub\s+(\w+)\s*:/g)].map((m) => m[1]).sort();

  // JS decoder output fields, observed at runtime (survives shorthand props).
  const { decodeSupabaseChange } = await import(pathToFileURL(path.join(root, SYNC_DECODE)).href);
  const decoded = decodeSupabaseChange("api_keys", { eventType: "INSERT", new: { id: "k1", version: 7 } });
  const jsFields = Object.keys(decoded).sort();

  assert.deepEqual(rustFields, RUST_CHANGE_EVENT_FIELDS, "Rust ChangeEvent must model required and optional fields");
  assert.deepEqual(jsFields, REQUIRED_CHANGE_EVENT_FIELDS, "Supabase decoder must emit the required envelope");
  for (const field of REQUIRED_CHANGE_EVENT_FIELDS) {
    assert.ok(rustFields.includes(field), `Rust ChangeEvent must include required decoder field ${field}`);
  }
  assert.ok(rustFields.includes("write_key"), "Rust core must retain the backend-only write identity");
  assert.ok(!jsFields.includes("write_key"), "Supabase CDC must not fabricate a backend write identity");

  // SQL documents the transport-agnostic subset (no at_ms wire timestamp).
  const sqlShape = read(CUSTOMER_SQL).match(/\{\s*table\s*,\s*op\s*,\s*id\s*,\s*version\s*,\s*row\s*\}/);
  assert.ok(sqlShape, "customer.sql must document the change-event shape");
  for (const field of ["table", "op", "id", "version", "row"]) {
    assert.ok(rustFields.includes(field), `SQL-documented field ${field} must exist in the ChangeEvent envelope`);
  }
});
