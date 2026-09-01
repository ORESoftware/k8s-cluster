import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  multicloudEvidenceDigest,
  verifyMulticloudEvidence,
} from '../../tools/multicloud-data-plane-evidence.mjs';

const example = JSON.parse(await readFile(new URL('../../argocd/multicloud-data-plane/evidence.example.json', import.meta.url), 'utf8'));

function copy() {
  return structuredClone(example);
}

test('accepts deterministic exact-revision canary evidence', () => {
  const result = verifyMulticloudEvidence(example, { expectedDigest: example.evidence_sha256 });
  assert.equal(result.ok, true, result.errors.join('\n'));
  assert.equal(result.evidence_sha256, example.evidence_sha256);

  const reordered = Object.fromEntries(Object.entries(copy()).reverse());
  assert.equal(multicloudEvidenceDigest(reordered), example.evidence_sha256);
});

test('production evidence requires clean-room restore and NATS account JWT isolation', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.mode = 'production';
  value.cockroachdb.restore.kind = 'point-in-time';
  value.nats.operator_account_jwt_isolation_verified = false;
  value.nats.subject_acl_verified = false;
  const result = verifyMulticloudEvidence(value);
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /clean-room for production evidence/u);
  assert.match(result.errors.join('\n'), /operator_account_jwt_isolation_verified/u);
  assert.match(result.errors.join('\n'), /subject_acl_verified/u);
});

test('rejects broad peer ranges, public exposure, and non-private DNS answers', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.providers[0].network.peer_cidrs[0] = '10.0.0.0/8';
  value.providers[0].network.cockroach_sql_publicly_reachable = true;
  value.providers[0].network.private_dns[0].addresses = ['8.8.8.8'];
  const result = verifyMulticloudEvidence(value);
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /broad RFC1918 placeholder/u);
  assert.match(result.errors.join('\n'), /cockroach_sql_publicly_reachable/u);
  assert.match(result.errors.join('\n'), /must be a private IP address/u);
});

test('rejects trust-anchor drift and a shared CockroachDB/NATS CA', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.providers[1].ca_fingerprints.cockroach_sha256 = '4'.repeat(64);
  value.providers[2].ca_fingerprints.nats_sha256 = value.providers[2].ca_fingerprints.cockroach_sha256;
  const result = verifyMulticloudEvidence(value);
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /cockroach_fingerprint must be identical/u);
  assert.match(result.errors.join('\n'), /separate trust anchors/u);
});

test('rejects unsafe CockroachDB health and restore evidence', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.cockroachdb.under_replicated_ranges = 2;
  value.cockroachdb.restore.source_artifact_sha256 = '5'.repeat(64);
  value.cockroachdb.restore.target_cluster_uid = value.providers[0].cluster_uid;
  const result = verifyMulticloudEvidence(value);
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /under_replicated_ranges: must equal 0/u);
  assert.match(result.errors.join('\n'), /must match the verified backup artifact digest/u);
  assert.match(result.errors.join('\n'), /isolated restore cluster/u);
});

test('rejects a stretched or incomplete NATS supercluster and Fiducia trust collapse', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.nats.regional_clusters[0].connected_gateways = ['gcp'];
  value.nats.regional_clusters[1].jetstream_domain = 'ORES_AWS';
  value.nats.cross_region_durability.strategies = ['snapshot-restore'];
  value.fiducia_boundary.direct_nats_gateway_member = true;
  value.fiducia_boundary.shared_nats_account = true;
  const result = verifyMulticloudEvidence(value);
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /exactly two peer providers/u);
  assert.match(result.errors.join('\n'), /unique JetStream domains/u);
  assert.match(result.errors.join('\n'), /transactional outbox or explicit JetStream mirrors\/sources/u);
  assert.match(result.errors.join('\n'), /direct_nats_gateway_member/u);
  assert.match(result.errors.join('\n'), /shared_nats_account/u);
});

test('rejects embedded credentials, certificate material, and digest drift', () => {
  const value = copy();
  value.runtime_secret = 'ghp_example-do-not-accept';
  value.evidence_sha256 = '0'.repeat(64);
  const result = verifyMulticloudEvidence(value, { expectedDigest: 'f'.repeat(64) });
  assert.equal(result.ok, false);
  assert.match(result.errors.join('\n'), /secret-bearing fields are forbidden/u);
  assert.match(result.errors.join('\n'), /forbidden credential or certificate material/u);
  assert.match(result.errors.join('\n'), /evidence_sha256: does not match/u);
  assert.match(result.errors.join('\n'), /expected_digest: does not match/u);
});
