import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

// Ratchet on BRIDGE_MAX_IN_FLIGHT.
//
// The nats-bridge crate ships TWO binaries with different concurrency profiles:
//   src/main.rs                  — older bridge, documents/defaults to 64
//   src/bin/nats_http_ingress.rs — DEN-440 external HTTP ingress, defaults to 128
//
// The deployment execs `--bin nats_http_ingress`, so its ceiling must track that
// binary's DEFAULT_MAX_IN_FLIGHT, not the older bridge's 64.
//
// This exists because a 2026-07-31 main/dev merge conflict (both branches added
// the key independently, five minutes apart) was resolved to 64 on the reasoning
// that only that side shipped a resilience suite. That applied the wrong binary's
// tuning and silently halved the ingress ceiling, and nothing caught it because
// no test asserted either value. Now one does.

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/apps/dd-next-runtime.application.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const DEPLOYMENT = 'remote/argocd/messaging/nats-bridge.deployment.yaml';
const INGRESS_SRC = 'remote/nats-bridge/src/bin/nats_http_ingress.rs';

function read(rel: string): string | null {
  const p = resolve(repoRoot, rel);
  return existsSync(p) ? readFileSync(p, 'utf8') : null;
}

test('nats-bridge in-flight ceiling matches the binary it actually runs', () => {
  const manifest = read(DEPLOYMENT);
  const src = read(INGRESS_SRC);
  if (!manifest || !src) return; // submodule not initialised

  // The manifest must still be running the ingress binary; if that ever changes,
  // the expected default changes with it and this assertion should be revisited.
  assert.match(
    manifest,
    /--bin\s+nats_http_ingress/,
    `${DEPLOYMENT} no longer execs nats_http_ingress — re-derive the expected in-flight default.`,
  );

  const codeDefault = src.match(/const\s+DEFAULT_MAX_IN_FLIGHT:\s*usize\s*=\s*(\d+)/)?.[1];
  assert.ok(codeDefault, `could not read DEFAULT_MAX_IN_FLIGHT from ${INGRESS_SRC}`);

  const deployed = manifest.match(
    /- name:\s*BRIDGE_MAX_IN_FLIGHT\s*\n\s*value:\s*'?(\d+)'?/,
  )?.[1];
  assert.ok(deployed, `${DEPLOYMENT} must declare BRIDGE_MAX_IN_FLIGHT`);

  assert.equal(
    deployed,
    codeDefault,
    `BRIDGE_MAX_IN_FLIGHT is ${deployed} but ${INGRESS_SRC} defaults to ${codeDefault}. ` +
      'The older src/main.rs bridge uses 64 — do not apply its tuning to the ingress binary. ' +
      'If the ceiling is being changed deliberately, change the code default too so the two stay aligned.',
  );
});
