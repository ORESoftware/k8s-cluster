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

test('rejects source drift, duplicate cluster identity, and weak provider prerequisites', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.source.repository = 'ORESoftware/not-the-authority';
  value.providers[1].cluster_uid = value.providers[0].cluster_uid;
  value.providers[0].region = 'us-west-2';
  value.providers[0].zones = ['us-east-1a', 'us-east-1a', 'us-east-1c'];
  value.providers[0].kubernetes_version = 'v1.29.9';
  value.providers[0].storage.encrypted = false;
  value.providers[0].storage.snapshot_restore_verified = false;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /source\.repository: must equal ORESoftware\/k8s-cluster/u);
  assert.match(errors, /three distinct Kubernetes clusters/u);
  assert.match(errors, /providers\[0\]\.region: must equal us-east-1/u);
  assert.match(errors, /providers\[0\]\.zones: must contain unique values/u);
  assert.match(errors, /at least Kubernetes 1\.30/u);
  assert.match(errors, /providers\[0\]\.storage\.encrypted: must be true/u);
  assert.match(errors, /providers\[0\]\.storage\.snapshot_restore_verified: must be true/u);
});

test('rejects noncanonical private routing and incomplete DNS proof', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.providers[0].network.peer_cidrs[0] = '10.20.1.2/16';
  value.providers[0].network.private_dns[5].name = value.providers[0].network.private_dns[4].name;
  value.providers[0].network.private_dns[5].resolved = false;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /must be a canonical private IPv4 CIDR/u);
  assert.match(errors, /must prove all six required private DNS names exactly once/u);
  assert.match(errors, /providers\[0\]\.network\.private_dns\[5\]\.resolved: must be true/u);
});

test('rejects evidence chronology and backup or restore observations outside the window', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.source.observed_at = '2026-09-01T12:05:00Z';
  value.evidence_completed_at = '2026-09-01T11:59:00Z';
  value.cockroachdb.backup.completed_at = '2026-09-01T11:30:00Z';
  value.cockroachdb.restore.completed_at = '2026-09-01T14:00:00Z';

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /evidence_completed_at: must not precede evidence_started_at/u);
  assert.match(errors, /source\.observed_at: must be observed before or at evidence_started_at/u);
  assert.match(errors, /backup\.completed_at: must not precede evidence_started_at/u);
  assert.match(errors, /restore\.completed_at: must not follow evidence_completed_at/u);
});

test('rejects CockroachDB quorum, locality, TLS, and region-survival regressions', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.cockroachdb.live_nodes = 8;
  value.cockroachdb.localities[2].provider = 'aws';
  value.cockroachdb.localities[2].region = 'us-east-1';
  value.cockroachdb.localities[1].live_nodes = 2;
  value.cockroachdb.unavailable_ranges = 1;
  value.cockroachdb.sql_tls_verified = false;
  value.cockroachdb.wrong_ca_rejected = false;
  value.cockroachdb.region_survival_verified = false;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /cockroachdb\.live_nodes: must equal 9/u);
  assert.match(errors, /must contain AWS, GCP, and Azure exactly once/u);
  assert.match(errors, /cockroachdb\.localities\[1\]\.live_nodes: must equal 3/u);
  assert.match(errors, /cockroachdb\.unavailable_ranges: must equal 0/u);
  assert.match(errors, /cockroachdb\.sql_tls_verified: must be true/u);
  assert.match(errors, /cockroachdb\.wrong_ca_rejected: must be true/u);
  assert.match(errors, /cockroachdb\.region_survival_verified: must be true/u);
});

test('rejects NATS supercluster gateway and transport regressions', () => {
  const value = copy();
  delete value.evidence_sha256;
  const aws = value.nats.regional_clusters[0];
  aws.connected_gateways = ['aws', 'gcp'];
  aws.route_tls_verified = false;
  aws.gateway_mtls_verified = false;
  aws.client_mtls_verified = false;
  aws.unknown_cluster_rejected = false;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /must contain gcp and azure/u);
  assert.match(errors, /route_tls_verified: must be true/u);
  assert.match(errors, /gateway_mtls_verified: must be true/u);
  assert.match(errors, /client_mtls_verified: must be true/u);
  assert.match(errors, /unknown_cluster_rejected: must be true/u);
});

test('rejects NATS durability, replay, and protected-effect regressions', () => {
  const value = copy();
  delete value.evidence_sha256;
  const durability = value.nats.cross_region_durability;
  durability.strategies = ['global-raft', 'snapshot-restore'];
  durability.redelivery_verified = false;
  durability.dlq_verified = false;
  durability.snapshot_restore_verified = false;
  durability.partition_recovery_verified = false;
  durability.duplicate_protected_effects = 1;
  durability.mirror_lag_seconds = -1;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /uses an unsupported strategy/u);
  assert.match(errors, /transactional outbox or explicit JetStream mirrors\/sources/u);
  assert.match(errors, /redelivery_verified: must be true/u);
  assert.match(errors, /dlq_verified: must be true/u);
  assert.match(errors, /snapshot_restore_verified: must be true/u);
  assert.match(errors, /partition_recovery_verified: must be true/u);
  assert.match(errors, /duplicate_protected_effects: must equal 0/u);
  assert.match(errors, /mirror_lag_seconds: must be a finite number >= 0/u);
});

test('rejects every direct Fiducia-to-supercluster trust shortcut', () => {
  const value = copy();
  delete value.evidence_sha256;
  value.fiducia_boundary.shared_nats_ca = true;
  value.fiducia_boundary.shared_jetstream_domain = true;
  value.fiducia_boundary.gateway_mode = 'direct-nats';
  value.fiducia_boundary.replay_protection_verified = false;
  value.fiducia_boundary.idempotency_verified = false;

  const result = verifyMulticloudEvidence(value);
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /shared_nats_ca: must be false/u);
  assert.match(errors, /shared_jetstream_domain: must be false/u);
  assert.match(errors, /gateway_mode: must equal authenticated-application-api/u);
  assert.match(errors, /replay_protection_verified: must be true/u);
  assert.match(errors, /idempotency_verified: must be true/u);
});

test('rejects malformed declared and reviewed evidence digests', () => {
  const value = copy();
  value.evidence_sha256 = 'not-a-digest';
  const result = verifyMulticloudEvidence(value, { expectedDigest: 'also-not-a-digest' });
  const errors = result.errors.join('\n');
  assert.equal(result.ok, false);
  assert.match(errors, /evidence_sha256: must be a lowercase SHA-256 digest/u);
  assert.match(errors, /expected_digest: must be a SHA-256 digest/u);
});
