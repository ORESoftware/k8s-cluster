// Self-tests for the generator: no network, no file writes (except the --check
// subprocess, which only reads). Pure schema -> string checks.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { build, loadTypes } from './generate-output.mjs';

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
    'QuoteRequest',
    'QuoteSubmissionResponse',
    'QuoteEstimate',
    'QuoteStatusEvent',
    'QuoteProblem',
    'QuoteSummary',
    'QuoteDetail',
    'QuoteListQuery',
    'QuoteListResponse',
    'QuoteRetryResponse',
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

test('quote schema keeps request, context, and estimate payloads bounded', () => {
  const schema = JSON.parse(readFileSync(join(root, 'schema/quote.schema.json'), 'utf8'));
  const request = schema.$defs.QuoteRequest;
  const estimate = schema.$defs.QuoteEstimate;

  assert.equal(request.additionalProperties, false);
  assert.equal(request.properties.frameworks.minItems, 1);
  assert.equal(request.properties.frameworks.maxItems, 12);
  assert.ok(request.properties.frameworks.items.enum.includes('soc2_type_2'));
  assert.ok(request.properties.frameworks.items.enum.includes('nist_csf_2'));
  assert.ok(request.properties.frameworks.items.enum.includes('nist_800_53'));
  assert.ok(request.properties.frameworks.items.enum.includes('hipaa'));
  assert.equal(request.properties.notes.maxLength, 5_000);
  assert.equal(request.properties.contextKey.maxLength, 128);
  assert.equal(request.properties.contextKey.default, 'quote-analysis');
  assert.equal(request.properties.answersVersion.const, 1);
  assert.equal(estimate.properties.summary.maxLength, 4_000);
  assert.equal(estimate.properties.assumptions.maxItems, 50);
  assert.equal(estimate.properties.nextSteps.maxItems, 25);
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
    'dart/lib/quote_v1.dart',
    'dart/pubspec.yaml',
  ]) {
    assert.ok(rel in files, `missing ${rel}`);
  }
});

test('rust and rust-wasm never diverge in data shape (same structs + fields)', () => {
  const out = build();
  const pubLines = (s) => s.split('\n').map((l) => l.trim()).filter((l) => l.startsWith('pub '));
  assert.deepEqual(pubLines(out['rust-wasm/src/lib.rs']), pubLines(out['rust/src/lib.rs']));
});

test('rust-wasm generated types remain declaration-only Tsify', () => {
  const files = build();
  const wasm = files['rust-wasm/src/lib.rs'];
  assert.match(wasm, /use tsify::Tsify;/);
  assert.doesNotMatch(wasm, /into_wasm_abi|from_wasm_abi/);
  assert.doesNotMatch(wasm, /use wasm_bindgen::prelude/);
  assert.doesNotMatch(wasm, /wasm_bindgen\(start\)/);
  assert.match(wasm, /pub struct ServiceInfo/);
  assert.match(wasm, /pub struct QuoteDetail/);
  assert.match(files['rust-wasm/Cargo.toml'], /crate-type = \["cdylib", "rlib"\]/);
  assert.match(files['rust-wasm/Cargo.toml'], /tsify = /);
  const lines = wasm.split('\n');
  lines.forEach((line, i) => {
    if (/pub .*(serde_json::Value|BTreeMap)/.test(line)) {
      assert.match(lines[i - 1] || '', /#\[tsify\(type = /, `unguarded field: ${line.trim()}`);
    }
  });
});

test('rust-wasm package entrypoint owns only a no-op lifecycle hook', () => {
  const files = build();
  assert.match(
    files['rust-wasm/Cargo.toml'],
    /path = "\.\.\/\.\.\/src\/rust-wasm-entry\.rs"/,
  );

  const entry = readFileSync(join(root, 'src/rust-wasm-entry.rs'), 'utf8');
  assert.match(entry, /use wasm_bindgen::prelude::wasm_bindgen;/);
  assert.match(
    entry,
    /#\[wasm_bindgen\(start\)\]\npub fn initialize_wasm_module\(\) \{\}/,
  );
  assert.match(entry, /include!\("\.\.\/generated\/rust-wasm\/src\/lib\.rs"\);/);

  const executableText = entry.replace(/^\s*\/\/.*$/gm, '');
  assert.doesNotMatch(
    executableText,
    /\b(?:fetch|request|submit|write|delete|storage|cookie|websocket|beacon|probe|execute)\b/i,
  );
});

test('generated types carry through to every language', () => {
  const files = build();
  assert.match(files['rust/src/lib.rs'], /pub struct ServiceInfo/);
  assert.match(files['rust/src/lib.rs'], /pub struct QuoteRequest/);
  assert.match(files['rust/src/lib.rs'], /pub struct QuoteListResponse/);
  assert.match(files['rust/Cargo.toml'], /name = "canonical-interfaces"/);
  assert.match(files['typescript/index.ts'], /export type ServiceInfo = \{/);
  assert.match(files['typescript/index.ts'], /export type QuoteEstimate = \{/);
  assert.match(files['typescript/index.ts'], /export type QuoteDetail = \{/);
  assert.match(files['python/canonical_interfaces.py'], /class AuditEngagement:/);
  assert.match(files['python/canonical_interfaces.py'], /class QuoteStatusEvent:/);
  assert.match(files['python/canonical_interfaces.py'], /class QuoteRetryResponse:/);
  assert.match(files['go/interfaces.go'], /package canonicalinterfaces/);
  assert.match(files['go/interfaces.go'], /type QuoteProblem struct/);
  assert.match(files['go/interfaces.go'], /type QuoteListResponse struct/);
  assert.match(files['dart/lib/quote_v1.dart'], /final class QuoteRequest/);
  assert.match(files['dart/lib/quote_v1.dart'], /final class QuoteDetail/);
});

test('string enums surface as typed unions/literals per language', () => {
  const files = build();
  assert.match(files['typescript/index.ts'], /framework: "soc2" \| "fedramp" \| "hipaa" \| "iso_27001" \| "pci_dss" \| "gdpr";/);
  assert.match(files['python/canonical_interfaces.py'], /Literal\["soc2", "fedramp", "hipaa", "iso_27001", "pci_dss", "gdpr"\]/);
  assert.match(files['rust/src/lib.rs'], /pub enum AuditEngagementFramework/);
  assert.match(files['typescript/index.ts'], /status: "applied" \| "conflict" \| "gone" \| "invalid" \| "idempotency_key_reused";/);
  assert.match(files['typescript/index.ts'], /status: "queued" \| "analyzing" \| "ready" \| "failed";/);
});

test('camelCase JSON fields stay camelCase on the wire and idiomatic in Rust', () => {
  const files = build();
  assert.match(files['typescript/index.ts'], /protocolVersion: number;/);
  assert.match(files['typescript/index.ts'], /organizationName: string;/);
  assert.match(files['typescript/index.ts'], /quoteId: string;/);
  assert.match(files['rust/src/lib.rs'], /#\[serde\(rename = "protocolVersion"\)\]\n    pub protocol_version: i64,/);
  assert.match(files['rust/src/lib.rs'], /#\[serde\(rename = "organizationName"\)\]\n    pub organization_name: String,/);
  assert.match(files['rust/src/lib.rs'], /#\[serde\(rename = "quoteId"\)\]\n    pub quote_id: String,/);
  assert.match(files['go/interfaces.go'], /ProtocolVersion int64 `json:"protocolVersion"`/);
  assert.match(files['go/interfaces.go'], /OrganizationName string `json:"organizationName"`/);
  assert.match(files['go/interfaces.go'], /QuoteId string `json:"quoteId"`/);
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
  assert.match(files['typescript/index.ts'], /target_report_date\?: string;/);
  assert.match(files['typescript/index.ts'], /contextKey\?: string;/);
  assert.match(files['rust/src/lib.rs'], /pub target_report_date: Option<String>,/);
  assert.match(files['rust/src/lib.rs'], /pub context_key: Option<String>,/);
  assert.match(files['go/interfaces.go'], /json:"target_report_date,omitempty"/);
  assert.match(files['go/interfaces.go'], /json:"contextKey,omitempty"/);
  assert.match(files['dart/lib/quote_v1.dart'], /final String\? contextKey;/);
});

test('generated files on disk are up to date (run: npm run generate)', () => {
  execFileSync('node', ['src/generate-output.mjs', '--check'], { cwd: root });
});
