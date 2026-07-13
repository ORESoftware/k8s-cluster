// Self-tests for the generator: no network, no file writes (except the --check
// subprocess, which only reads). Pure schema -> string checks.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { build, loadTypes } from './generate.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

test('schema declares the expected canonical.cloud types', () => {
  const names = loadTypes().map((t) => t.name);
  assert.deepEqual(names, [
    'HealthStatus',
    'ServiceInfo',
    'DraftNoteValue',
    'DraftNoteKey',
    'MutationOperation',
    'MutationRequest',
    'WireRecord',
    'MutationResult',
    'MutationResponse',
    'ChangesQuery',
    'ChangesResponse',
    'AuditEngagement',
  ]);
});

test('sync schema keeps server-enforced batch, page, and draft-note bounds', () => {
  const schema = JSON.parse(readFileSync(join(root, 'schema/api.schema.json'), 'utf8'));
  assert.equal(schema.$defs.MutationRequest.properties.operations.minItems, 1);
  assert.equal(schema.$defs.MutationRequest.properties.operations.maxItems, 50);
  assert.equal(schema.$defs.ChangesQuery.properties.limit.maximum, 500);
  assert.equal(schema.$defs.DraftNoteValue.properties.title.maxLength, 200);
  assert.equal(schema.$defs.DraftNoteValue.properties.body.maxLength, 100_000);
});

test('build() emits one file per language', () => {
  const files = build();
  for (const rel of [
    'rust/src/lib.rs',
    'rust/Cargo.toml',
    'rust-wasm/src/lib.rs',
    'rust-wasm/Cargo.toml',
    'typescript/index.ts',
    'python/canonical_interfaces.py',
    'go/interfaces.go',
  ]) {
    assert.ok(rel in files, `missing ${rel}`);
  }
});

test('rust and rust-wasm never diverge in data shape (same structs + fields)', () => {
  const out = build();
  const pubLines = (s) => s.split('\n').map((l) => l.trim()).filter((l) => l.startsWith('pub '));
  assert.deepEqual(pubLines(out['rust-wasm/src/lib.rs']), pubLines(out['rust/src/lib.rs']));
});

test('rust-wasm is declaration-only Tsify (no wasm ABI baked in)', () => {
  const files = build();
  const wasm = files['rust-wasm/src/lib.rs'];
  assert.match(wasm, /use tsify::Tsify;/);
  assert.doesNotMatch(wasm, /into_wasm_abi|from_wasm_abi/);
  assert.doesNotMatch(wasm, /use wasm_bindgen::prelude/);
  assert.match(wasm, /pub struct ServiceInfo/);
  assert.match(files['rust-wasm/Cargo.toml'], /crate-type = \["cdylib", "rlib"\]/);
  assert.match(files['rust-wasm/Cargo.toml'], /tsify = /);
  // No serde_json::Value / BTreeMap field may reach tsify without a type override
  // (which would emit an undefined `Value` or a wrong `Map` in the .d.ts).
  const lines = wasm.split('\n');
  lines.forEach((line, i) => {
    if (/pub .*(serde_json::Value|BTreeMap)/.test(line)) {
      assert.match(lines[i - 1] || '', /#\[tsify\(type = /, `unguarded field: ${line.trim()}`);
    }
  });
});

test('generated types carry through to every language', () => {
  const files = build();
  assert.match(files['rust/src/lib.rs'], /pub struct ServiceInfo/);
  assert.match(files['rust/Cargo.toml'], /name = "canonical-interfaces"/);
  assert.match(files['typescript/index.ts'], /export type ServiceInfo = \{/);
  assert.match(files['python/canonical_interfaces.py'], /class AuditEngagement:/);
  assert.match(files['go/interfaces.go'], /package canonicalinterfaces/);
});

test('string enums surface as typed unions/literals per language', () => {
  const files = build();
  // AuditEngagement.framework is a string enum.
  assert.match(files['typescript/index.ts'], /framework: "soc2" \| "fedramp" \| "hipaa" \| "iso_27001" \| "pci_dss" \| "gdpr";/);
  assert.match(files['python/canonical_interfaces.py'], /Literal\["soc2", "fedramp", "hipaa", "iso_27001", "pci_dss", "gdpr"\]/);
  assert.match(files['rust/src/lib.rs'], /pub enum AuditEngagementFramework/);
  assert.match(files['typescript/index.ts'], /status: "applied" \| "conflict" \| "gone" \| "invalid" \| "idempotency_key_reused";/);
});

test('camelCase JSON fields stay camelCase on the wire and idiomatic in Rust', () => {
  const files = build();
  assert.match(files['typescript/index.ts'], /protocolVersion: number;/);
  assert.match(files['rust/src/lib.rs'], /#\[serde\(rename = "protocolVersion"\)\]\n    pub protocol_version: i64,/);
  assert.match(files['go/interfaces.go'], /ProtocolVersion int64 `json:"protocolVersion"`/);
});

test('required nullable decimal versions stay nullable in every adapter', () => {
  const files = build();
  assert.match(files['typescript/index.ts'], /baseVersion: string \| null;/);
  assert.match(files['rust/src/lib.rs'], /pub base_version: Option<String>,/);
  assert.match(files['python/canonical_interfaces.py'], /baseVersion: Optional\[str\]/);
  assert.match(files['go/interfaces.go'], /BaseVersion \*string `json:"baseVersion"`/);
});

test('optional fields are nullable/omittable per language', () => {
  const files = build();
  // AuditEngagement.target_report_date is optional.
  assert.match(files['typescript/index.ts'], /target_report_date\?: string;/);
  assert.match(files['rust/src/lib.rs'], /pub target_report_date: Option<String>,/);
  assert.match(files['go/interfaces.go'], /json:"target_report_date,omitempty"/);
});

test('generated files on disk are up to date (run: node src/generate.mjs)', () => {
  execFileSync('node', ['src/generate.mjs', '--check'], { cwd: root });
});
