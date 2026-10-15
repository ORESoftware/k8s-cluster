import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export const MULTICLOUD_EVIDENCE_SCHEMA_VERSION = 'dd.multicloud-data-plane.v1';
export const MULTICLOUD_EVIDENCE_REPOSITORY = 'ORESoftware/k8s-cluster';

const EXPECTED_PROVIDERS = Object.freeze({
  aws: { region: 'us-east-1', domain: 'ORES_AWS' },
  gcp: { region: 'us-central1', domain: 'ORES_GCP' },
  azure: { region: 'eastus', domain: 'ORES_AZURE' },
});
const EXPECTED_PROVIDER_NAMES = Object.freeze(Object.keys(EXPECTED_PROVIDERS));
const REQUIRED_PRIVATE_DNS_NAMES = Object.freeze([
  'dd-cockroachdb.cockroachdb.svc.aws.crdb.k8s.ores.internal',
  'dd-cockroachdb.cockroachdb.svc.gcp.crdb.k8s.ores.internal',
  'dd-cockroachdb.cockroachdb.svc.azure.crdb.k8s.ores.internal',
  'nats-gateway.aws.k8s.ores.internal',
  'nats-gateway.gcp.k8s.ores.internal',
  'nats-gateway.azure.k8s.ores.internal',
]);
const SHA40 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;
const FORBIDDEN_KEY = /(^|_)(?:secret|token|password|credential|private_key|client_key|tls_key|api_key|access_key|session_key|key_material|pem)(?:_|$)/iu;
const FORBIDDEN_VALUE = /(?:-----BEGIN (?:CERTIFICATE|(?:RSA |EC |OPENSSH )?PRIVATE KEY)-----|gh[pousr]_[A-Za-z0-9]+|lin_api_[A-Za-z0-9]+|AKIA[0-9A-Z]{16}|https?:\/\/[^/\s:@]+:[^@\s]+@)/u;
const BROAD_PRIVATE_CIDRS = new Set(['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']);
const ALLOWED_DURABILITY_STRATEGIES = new Set([
  'jetstream-mirrors',
  'jetstream-sources',
  'snapshot-restore',
  'transactional-outbox',
]);
const MAX_EVIDENCE_BYTES = 1_000_000;

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (!isRecord(value)) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, canonicalize(value[key])]),
  );
}

function digestInput(evidence) {
  if (!isRecord(evidence)) return evidence;
  const { evidence_sha256: _declared, ...rest } = evidence;
  return rest;
}

export function canonicalMulticloudEvidence(evidence) {
  return JSON.stringify(canonicalize(digestInput(evidence)));
}

export function multicloudEvidenceDigest(evidence) {
  return createHash('sha256').update(canonicalMulticloudEvidence(evidence)).digest('hex');
}

function add(errors, path, message) {
  errors.push(`${path}: ${message}`);
}

function requireRecord(value, errors, path) {
  if (!isRecord(value)) {
    add(errors, path, 'must be an object');
    return null;
  }
  return value;
}

function requireString(value, errors, path, { pattern = null, allowed = null } = {}) {
  const text = typeof value === 'string' ? value.trim() : '';
  if (!text) {
    add(errors, path, 'must be a non-empty string');
    return null;
  }
  if (pattern && !pattern.test(text)) add(errors, path, 'has an invalid format');
  if (allowed && !allowed.has(text)) add(errors, path, `must be one of ${[...allowed].join(', ')}`);
  return text;
}

function requireBoolean(value, expected, errors, path) {
  if (typeof value !== 'boolean') {
    add(errors, path, 'must be a boolean');
    return null;
  }
  if (expected !== null && value !== expected) add(errors, path, `must be ${expected}`);
  return value;
}

function requireInteger(value, errors, path, { minimum = 0, exact = null } = {}) {
  if (!Number.isSafeInteger(value) || value < minimum) {
    add(errors, path, `must be a safe integer >= ${minimum}`);
    return null;
  }
  if (exact !== null && value !== exact) add(errors, path, `must equal ${exact}`);
  return value;
}

function requireFiniteNumber(value, errors, path, { minimum = 0 } = {}) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < minimum) {
    add(errors, path, `must be a finite number >= ${minimum}`);
    return null;
  }
  return value;
}

function requireTimestamp(value, errors, path) {
  const text = typeof value === 'string' ? value.trim() : '';
  const milliseconds = Date.parse(text);
  if (!text || !Number.isFinite(milliseconds)) {
    add(errors, path, 'must be an ISO-8601 timestamp');
    return null;
  }
  return { text, milliseconds };
}

function requireArray(value, errors, path, { minimum = 0, maximum = 100 } = {}) {
  if (!Array.isArray(value)) {
    add(errors, path, 'must be an array');
    return [];
  }
  if (value.length < minimum) add(errors, path, `must contain at least ${minimum} item(s)`);
  if (value.length > maximum) add(errors, path, `must contain no more than ${maximum} item(s)`);
  return value;
}

function uniqueStrings(value, errors, path, { minimum = 0 } = {}) {
  const items = requireArray(value, errors, path, { minimum, maximum: 100 });
  const normalized = [];
  for (const [index, item] of items.entries()) {
    const text = requireString(item, errors, `${path}[${index}]`);
    if (text) normalized.push(text);
  }
  if (new Set(normalized).size !== normalized.length) add(errors, path, 'must contain unique values');
  return normalized;
}

function scanForSensitiveMaterial(value, errors, path = 'evidence', seen = new Set()) {
  if (typeof value === 'string') {
    if (FORBIDDEN_VALUE.test(value)) add(errors, path, 'contains forbidden credential or certificate material');
    return;
  }
  if (!value || typeof value !== 'object') return;
  if (seen.has(value)) {
    add(errors, path, 'contains a circular reference');
    return;
  }
  seen.add(value);
  if (Array.isArray(value)) {
    value.forEach((item, index) => scanForSensitiveMaterial(item, errors, `${path}[${index}]`, seen));
  } else {
    for (const [key, item] of Object.entries(value)) {
      if (FORBIDDEN_KEY.test(key)) add(errors, `${path}.${key}`, 'secret-bearing fields are forbidden; retain only redacted fingerprints and results');
      scanForSensitiveMaterial(item, errors, `${path}.${key}`, seen);
    }
  }
  seen.delete(value);
}

function parseIpv4(value) {
  const octets = value.split('.');
  if (octets.length !== 4) return null;
  const numbers = octets.map((part) => Number(part));
  if (numbers.some((part, index) => !Number.isInteger(part) || part < 0 || part > 255 || String(part) !== octets[index])) return null;
  return numbers;
}

function ipv4Number(octets) {
  return (((octets[0] * 256 + octets[1]) * 256 + octets[2]) * 256 + octets[3]) >>> 0;
}

function isPrivateIpv4(value) {
  const octets = parseIpv4(value);
  if (!octets) return false;
  return octets[0] === 10
    || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
    || (octets[0] === 192 && octets[1] === 168);
}

function isPrivateIp(value) {
  const text = String(value ?? '').trim().toLowerCase();
  return isPrivateIpv4(text) || text.startsWith('fc') || text.startsWith('fd') || text === '::1';
}

function validPrivateCidr(value) {
  const [address, prefixText, ...extra] = String(value ?? '').trim().split('/');
  if (extra.length > 0) return false;
  const octets = parseIpv4(address);
  const prefix = Number(prefixText);
  if (!octets || !Number.isInteger(prefix) || prefix < 16 || prefix > 32 || !isPrivateIpv4(address)) return false;
  const addressNumber = ipv4Number(octets);
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return (addressNumber & mask) === addressNumber;
}

function parseKubernetesVersion(value) {
  const match = /^v?(\d+)\.(\d+)(?:\.(\d+))?(?:[-+].*)?$/u.exec(String(value ?? '').trim());
  if (!match) return null;
  return { major: Number(match[1]), minor: Number(match[2]), patch: Number(match[3] ?? 0) };
}

function kubernetesAtLeast130(version) {
  return version && (version.major > 1 || (version.major === 1 && version.minor >= 30));
}

function exactSet(actual, expected) {
  return actual.length === expected.length
    && new Set(actual).size === actual.length
    && actual.every((item) => expected.includes(item));
}

function normalizeProvider(provider, index, errors) {
  const path = `providers[${index}]`;
  if (!requireRecord(provider, errors, path)) return null;
  const name = requireString(provider.provider, errors, `${path}.provider`, { allowed: new Set(EXPECTED_PROVIDER_NAMES) });
  const contract = name ? EXPECTED_PROVIDERS[name] : null;
  const clusterUid = requireString(provider.cluster_uid, errors, `${path}.cluster_uid`, { pattern: UUID });
  const region = requireString(provider.region, errors, `${path}.region`);
  if (contract && region !== contract.region) add(errors, `${path}.region`, `must equal ${contract.region}`);
  const zones = uniqueStrings(provider.zones, errors, `${path}.zones`, { minimum: 3 });
  if (zones.length !== 3) add(errors, `${path}.zones`, 'must contain exactly three failure zones');
  const versionText = requireString(provider.kubernetes_version, errors, `${path}.kubernetes_version`);
  const version = versionText ? parseKubernetesVersion(versionText) : null;
  if (versionText && !version) add(errors, `${path}.kubernetes_version`, 'must be a Kubernetes semantic version');
  if (version && !kubernetesAtLeast130(version)) add(errors, `${path}.kubernetes_version`, 'must be at least Kubernetes 1.30');

  const storage = requireRecord(provider.storage, errors, `${path}.storage`);
  if (storage) {
    const storageClass = requireString(storage.storage_class, errors, `${path}.storage.storage_class`);
    if (storageClass && storageClass !== 'dd-block') add(errors, `${path}.storage.storage_class`, 'must equal dd-block');
    requireBoolean(storage.encrypted, true, errors, `${path}.storage.encrypted`);
    requireBoolean(storage.snapshot_restore_verified, true, errors, `${path}.storage.snapshot_restore_verified`);
  }

  const network = requireRecord(provider.network, errors, `${path}.network`);
  if (network) {
    const peerCidrs = uniqueStrings(network.peer_cidrs, errors, `${path}.network.peer_cidrs`, { minimum: 2 });
    if (peerCidrs.length !== 2) add(errors, `${path}.network.peer_cidrs`, 'must contain exactly two peer-cluster CIDRs');
    for (const [cidrIndex, cidr] of peerCidrs.entries()) {
      if (!validPrivateCidr(cidr)) add(errors, `${path}.network.peer_cidrs[${cidrIndex}]`, 'must be a canonical private IPv4 CIDR');
      if (BROAD_PRIVATE_CIDRS.has(cidr)) add(errors, `${path}.network.peer_cidrs[${cidrIndex}]`, 'must not use a broad RFC1918 placeholder range');
    }
    requireBoolean(network.cockroach_sql_publicly_reachable, false, errors, `${path}.network.cockroach_sql_publicly_reachable`);
    requireBoolean(network.nats_gateway_publicly_reachable, false, errors, `${path}.network.nats_gateway_publicly_reachable`);
    const records = requireArray(network.private_dns, errors, `${path}.network.private_dns`, {
      minimum: REQUIRED_PRIVATE_DNS_NAMES.length,
      maximum: REQUIRED_PRIVATE_DNS_NAMES.length,
    });
    const names = [];
    records.forEach((record, recordIndex) => {
      const recordPath = `${path}.network.private_dns[${recordIndex}]`;
      if (!requireRecord(record, errors, recordPath)) return;
      const recordName = requireString(record.name, errors, `${recordPath}.name`);
      if (recordName) names.push(recordName);
      requireBoolean(record.resolved, true, errors, `${recordPath}.resolved`);
      const addresses = uniqueStrings(record.addresses, errors, `${recordPath}.addresses`, { minimum: 1 });
      addresses.forEach((address, addressIndex) => {
        if (!isPrivateIp(address)) add(errors, `${recordPath}.addresses[${addressIndex}]`, 'must be a private IP address');
      });
    });
    if (!exactSet(names, REQUIRED_PRIVATE_DNS_NAMES)) add(errors, `${path}.network.private_dns`, 'must prove all six required private DNS names exactly once');
  }

  const fingerprints = requireRecord(provider.ca_fingerprints, errors, `${path}.ca_fingerprints`);
  const cockroachFingerprint = fingerprints
    ? requireString(fingerprints.cockroach_sha256, errors, `${path}.ca_fingerprints.cockroach_sha256`, { pattern: SHA256 })
    : null;
  const natsFingerprint = fingerprints
    ? requireString(fingerprints.nats_sha256, errors, `${path}.ca_fingerprints.nats_sha256`, { pattern: SHA256 })
    : null;
  if (cockroachFingerprint && natsFingerprint && cockroachFingerprint === natsFingerprint) {
    add(errors, `${path}.ca_fingerprints`, 'CockroachDB and NATS must use separate trust anchors');
  }

  return {
    provider: name,
    cluster_uid: clusterUid,
    region,
    zones,
    cockroach_fingerprint: cockroachFingerprint,
    nats_fingerprint: natsFingerprint,
  };
}

function normalizeCockroach(value, providers, timestamps, mode, errors) {
  const path = 'cockroachdb';
  if (!requireRecord(value, errors, path)) return null;
  const clusterId = requireString(value.cluster_id, errors, `${path}.cluster_id`, { pattern: UUID });
  requireInteger(value.live_nodes, errors, `${path}.live_nodes`, { exact: 9 });
  const localities = requireArray(value.localities, errors, `${path}.localities`, { minimum: 3, maximum: 3 });
  const localityProviders = [];
  localities.forEach((locality, index) => {
    const localityPath = `${path}.localities[${index}]`;
    if (!requireRecord(locality, errors, localityPath)) return;
    const provider = requireString(locality.provider, errors, `${localityPath}.provider`, { allowed: new Set(EXPECTED_PROVIDER_NAMES) });
    if (provider) localityProviders.push(provider);
    const region = requireString(locality.region, errors, `${localityPath}.region`);
    if (provider && region !== EXPECTED_PROVIDERS[provider].region) add(errors, `${localityPath}.region`, `must equal ${EXPECTED_PROVIDERS[provider].region}`);
    requireInteger(locality.live_nodes, errors, `${localityPath}.live_nodes`, { exact: 3 });
  });
  if (!exactSet(localityProviders, EXPECTED_PROVIDER_NAMES)) add(errors, `${path}.localities`, 'must contain AWS, GCP, and Azure exactly once');
  requireInteger(value.unavailable_ranges, errors, `${path}.unavailable_ranges`, { exact: 0 });
  requireInteger(value.under_replicated_ranges, errors, `${path}.under_replicated_ranges`, { exact: 0 });
  requireBoolean(value.sql_tls_verified, true, errors, `${path}.sql_tls_verified`);
  requireBoolean(value.wrong_ca_rejected, true, errors, `${path}.wrong_ca_rejected`);
  requireBoolean(value.region_survival_verified, true, errors, `${path}.region_survival_verified`);

  const backup = requireRecord(value.backup, errors, `${path}.backup`);
  const backupDigest = backup
    ? requireString(backup.artifact_sha256, errors, `${path}.backup.artifact_sha256`, { pattern: SHA256 })
    : null;
  const backupCompleted = backup ? requireTimestamp(backup.completed_at, errors, `${path}.backup.completed_at`) : null;
  if (backup) {
    if (backup.status !== 'success') add(errors, `${path}.backup.status`, 'must equal success');
    requireBoolean(backup.encrypted, true, errors, `${path}.backup.encrypted`);
  }

  const restore = requireRecord(value.restore, errors, `${path}.restore`);
  const restoreCompleted = restore ? requireTimestamp(restore.completed_at, errors, `${path}.restore.completed_at`) : null;
  const restoreKind = restore
    ? requireString(restore.kind, errors, `${path}.restore.kind`, { allowed: new Set(['point-in-time', 'clean-room']) })
    : null;
  const targetClusterUid = restore ? requireString(restore.target_cluster_uid, errors, `${path}.restore.target_cluster_uid`, { pattern: UUID }) : null;
  if (restore) {
    if (restore.status !== 'success') add(errors, `${path}.restore.status`, 'must equal success');
    const sourceDigest = requireString(restore.source_artifact_sha256, errors, `${path}.restore.source_artifact_sha256`, { pattern: SHA256 });
    if (backupDigest && sourceDigest && sourceDigest !== backupDigest) add(errors, `${path}.restore.source_artifact_sha256`, 'must match the verified backup artifact digest');
    requireFiniteNumber(restore.rpo_seconds, errors, `${path}.restore.rpo_seconds`);
    requireFiniteNumber(restore.rto_seconds, errors, `${path}.restore.rto_seconds`);
    if (targetClusterUid && providers.some((provider) => provider?.cluster_uid === targetClusterUid)) {
      add(errors, `${path}.restore.target_cluster_uid`, 'must identify an isolated restore cluster, not a source provider cluster');
    }
  }
  if (mode === 'production' && restoreKind && restoreKind !== 'clean-room') {
    add(errors, `${path}.restore.kind`, 'must be clean-room for production evidence');
  }
  if (backupCompleted && restoreCompleted && restoreCompleted.milliseconds < backupCompleted.milliseconds) {
    add(errors, `${path}.restore.completed_at`, 'must not precede the verified backup');
  }
  for (const [label, timestamp] of [['backup.completed_at', backupCompleted], ['restore.completed_at', restoreCompleted]]) {
    if (timestamp && timestamps.started && timestamp.milliseconds < timestamps.started.milliseconds) add(errors, `${path}.${label}`, 'must not precede evidence_started_at');
    if (timestamp && timestamps.completed && timestamp.milliseconds > timestamps.completed.milliseconds) add(errors, `${path}.${label}`, 'must not follow evidence_completed_at');
  }
  return { cluster_id: clusterId, restore_kind: restoreKind };
}

function normalizeNats(value, mode, errors) {
  const path = 'nats';
  if (!requireRecord(value, errors, path)) return null;
  const clusters = requireArray(value.regional_clusters, errors, `${path}.regional_clusters`, { minimum: 3, maximum: 3 });
  const providers = [];
  const domains = [];
  let directedConnections = 0;
  clusters.forEach((cluster, index) => {
    const clusterPath = `${path}.regional_clusters[${index}]`;
    if (!requireRecord(cluster, errors, clusterPath)) return;
    const provider = requireString(cluster.provider, errors, `${clusterPath}.provider`, { allowed: new Set(EXPECTED_PROVIDER_NAMES) });
    if (provider) providers.push(provider);
    const domain = requireString(cluster.jetstream_domain, errors, `${clusterPath}.jetstream_domain`);
    if (domain) domains.push(domain);
    if (provider && domain !== EXPECTED_PROVIDERS[provider].domain) add(errors, `${clusterPath}.jetstream_domain`, `must equal ${EXPECTED_PROVIDERS[provider].domain}`);
    requireInteger(cluster.servers, errors, `${clusterPath}.servers`, { exact: 3 });
    requireInteger(cluster.jetstream_replica_factor, errors, `${clusterPath}.jetstream_replica_factor`, { exact: 3 });
    const gateways = uniqueStrings(cluster.connected_gateways, errors, `${clusterPath}.connected_gateways`, { minimum: 2 });
    if (gateways.length !== 2) add(errors, `${clusterPath}.connected_gateways`, 'must contain exactly two peer providers');
    if (provider) {
      const expectedPeers = EXPECTED_PROVIDER_NAMES.filter((name) => name !== provider);
      if (!exactSet(gateways, expectedPeers)) add(errors, `${clusterPath}.connected_gateways`, `must contain ${expectedPeers.join(' and ')}`);
    }
    directedConnections += gateways.length;
    requireBoolean(cluster.route_tls_verified, true, errors, `${clusterPath}.route_tls_verified`);
    requireBoolean(cluster.gateway_mtls_verified, true, errors, `${clusterPath}.gateway_mtls_verified`);
    requireBoolean(cluster.client_mtls_verified, true, errors, `${clusterPath}.client_mtls_verified`);
    requireBoolean(cluster.unknown_cluster_rejected, true, errors, `${clusterPath}.unknown_cluster_rejected`);
  });
  if (!exactSet(providers, EXPECTED_PROVIDER_NAMES)) add(errors, `${path}.regional_clusters`, 'must contain AWS, GCP, and Azure exactly once');
  if (new Set(domains).size !== domains.length) add(errors, `${path}.regional_clusters`, 'must use unique JetStream domains');
  if (directedConnections !== 6) add(errors, `${path}.regional_clusters`, 'must prove six directed gateway connections');

  const durability = requireRecord(value.cross_region_durability, errors, `${path}.cross_region_durability`);
  if (durability) {
    const strategies = uniqueStrings(durability.strategies, errors, `${path}.cross_region_durability.strategies`, { minimum: 1 });
    strategies.forEach((strategy, index) => {
      if (!ALLOWED_DURABILITY_STRATEGIES.has(strategy)) add(errors, `${path}.cross_region_durability.strategies[${index}]`, 'uses an unsupported strategy');
    });
    if (!strategies.includes('transactional-outbox') && !strategies.some((strategy) => strategy.startsWith('jetstream-'))) {
      add(errors, `${path}.cross_region_durability.strategies`, 'must declare a transactional outbox or explicit JetStream mirrors/sources');
    }
    requireBoolean(durability.redelivery_verified, true, errors, `${path}.cross_region_durability.redelivery_verified`);
    requireBoolean(durability.dlq_verified, true, errors, `${path}.cross_region_durability.dlq_verified`);
    requireBoolean(durability.snapshot_restore_verified, true, errors, `${path}.cross_region_durability.snapshot_restore_verified`);
    requireBoolean(durability.partition_recovery_verified, true, errors, `${path}.cross_region_durability.partition_recovery_verified`);
    requireInteger(durability.duplicate_protected_effects, errors, `${path}.cross_region_durability.duplicate_protected_effects`, { exact: 0 });
    requireFiniteNumber(durability.mirror_lag_seconds, errors, `${path}.cross_region_durability.mirror_lag_seconds`);
  }
  requireBoolean(value.legacy_bootstrap_remains_authoritative, true, errors, `${path}.legacy_bootstrap_remains_authoritative`);
  const jwtVerified = requireBoolean(value.operator_account_jwt_isolation_verified, null, errors, `${path}.operator_account_jwt_isolation_verified`);
  requireBoolean(value.subject_acl_verified, mode === 'production' ? true : null, errors, `${path}.subject_acl_verified`);
  if (mode === 'production' && jwtVerified !== true) add(errors, `${path}.operator_account_jwt_isolation_verified`, 'must be true for production evidence');
  return { providers, domains };
}

function normalizeFiducia(value, errors) {
  const path = 'fiducia_boundary';
  if (!requireRecord(value, errors, path)) return null;
  requireBoolean(value.direct_nats_gateway_member, false, errors, `${path}.direct_nats_gateway_member`);
  requireBoolean(value.shared_nats_ca, false, errors, `${path}.shared_nats_ca`);
  requireBoolean(value.shared_nats_account, false, errors, `${path}.shared_nats_account`);
  requireBoolean(value.shared_jetstream_domain, false, errors, `${path}.shared_jetstream_domain`);
  const mode = requireString(value.gateway_mode, errors, `${path}.gateway_mode`);
  if (mode && mode !== 'authenticated-application-api') add(errors, `${path}.gateway_mode`, 'must equal authenticated-application-api');
  requireBoolean(value.replay_protection_verified, true, errors, `${path}.replay_protection_verified`);
  requireBoolean(value.idempotency_verified, true, errors, `${path}.idempotency_verified`);
  return { gateway_mode: mode };
}

export function verifyMulticloudEvidence(evidence, { expectedDigest = null } = {}) {
  const errors = [];
  if (!isRecord(evidence)) {
    return { ok: false, errors: ['evidence: must be an object'], evidence_sha256: null, summary: null };
  }
  scanForSensitiveMaterial(evidence, errors);
  if (evidence.schema_version !== MULTICLOUD_EVIDENCE_SCHEMA_VERSION) {
    add(errors, 'schema_version', `must equal ${MULTICLOUD_EVIDENCE_SCHEMA_VERSION}`);
  }
  const source = requireRecord(evidence.source, errors, 'source');
  const repository = source ? requireString(source.repository, errors, 'source.repository') : null;
  if (repository && repository !== MULTICLOUD_EVIDENCE_REPOSITORY) add(errors, 'source.repository', `must equal ${MULTICLOUD_EVIDENCE_REPOSITORY}`);
  const gitSha = source ? requireString(source.git_sha, errors, 'source.git_sha', { pattern: SHA40 }) : null;
  const observedAt = source ? requireTimestamp(source.observed_at, errors, 'source.observed_at') : null;
  const mode = requireString(evidence.mode, errors, 'mode', { allowed: new Set(['canary', 'production']) });
  const started = requireTimestamp(evidence.evidence_started_at, errors, 'evidence_started_at');
  const completed = requireTimestamp(evidence.evidence_completed_at, errors, 'evidence_completed_at');
  if (started && completed && completed.milliseconds < started.milliseconds) add(errors, 'evidence_completed_at', 'must not precede evidence_started_at');
  if (observedAt && started && observedAt.milliseconds > started.milliseconds) add(errors, 'source.observed_at', 'must be observed before or at evidence_started_at');

  const providerValues = requireArray(evidence.providers, errors, 'providers', { minimum: 3, maximum: 3 });
  const providers = providerValues.map((provider, index) => normalizeProvider(provider, index, errors)).filter(Boolean);
  const providerNames = providers.map((provider) => provider.provider).filter(Boolean);
  if (!exactSet(providerNames, EXPECTED_PROVIDER_NAMES)) add(errors, 'providers', 'must contain AWS, GCP, and Azure exactly once');
  const clusterUids = providers.map((provider) => provider.cluster_uid).filter(Boolean);
  if (new Set(clusterUids).size !== clusterUids.length) add(errors, 'providers', 'must identify three distinct Kubernetes clusters');
  for (const fingerprintKey of ['cockroach_fingerprint', 'nats_fingerprint']) {
    const fingerprints = providers.map((provider) => provider[fingerprintKey]).filter(Boolean);
    if (fingerprints.length === 3 && new Set(fingerprints).size !== 1) add(errors, 'providers.ca_fingerprints', `${fingerprintKey} must be identical in all three providers`);
  }

  const cockroach = normalizeCockroach(evidence.cockroachdb, providers, { started, completed }, mode, errors);
  const nats = normalizeNats(evidence.nats, mode, errors);
  const fiducia = normalizeFiducia(evidence.fiducia_boundary, errors);

  const digest = multicloudEvidenceDigest(evidence);
  const declared = evidence.evidence_sha256 === undefined ? null : String(evidence.evidence_sha256).trim().toLowerCase();
  if (declared !== null && !SHA256.test(declared)) add(errors, 'evidence_sha256', 'must be a lowercase SHA-256 digest');
  else if (declared !== null && declared !== digest) add(errors, 'evidence_sha256', 'does not match the canonical evidence digest');
  const expected = expectedDigest === null || expectedDigest === undefined ? null : String(expectedDigest).trim().toLowerCase();
  if (expected !== null && !SHA256.test(expected)) add(errors, 'expected_digest', 'must be a SHA-256 digest');
  else if (expected !== null && expected !== digest) add(errors, 'expected_digest', 'does not match the canonical evidence digest');

  return {
    ok: errors.length === 0,
    errors,
    evidence_sha256: digest,
    summary: {
      schema_version: evidence.schema_version,
      repository,
      git_sha: gitSha,
      mode,
      providers: providerNames,
      cockroach_cluster_id: cockroach?.cluster_id ?? null,
      nats_domains: nats?.domains ?? [],
      fiducia_gateway_mode: fiducia?.gateway_mode ?? null,
    },
  };
}

export async function readMulticloudEvidence(path, { maxBytes = MAX_EVIDENCE_BYTES } = {}) {
  const content = await readFile(path);
  if (content.length > maxBytes) throw new Error(`Evidence exceeds ${maxBytes} bytes`);
  try {
    return JSON.parse(content.toString('utf8'));
  } catch {
    throw new Error('Evidence is not valid JSON');
  }
}

async function main() {
  const path = String(process.env.DD_MULTICLOUD_EVIDENCE_PATH ?? '').trim();
  if (!path) throw new Error('DD_MULTICLOUD_EVIDENCE_PATH is required');
  const evidence = await readMulticloudEvidence(path);
  const result = verifyMulticloudEvidence(evidence, {
    expectedDigest: process.env.DD_MULTICLOUD_EXPECTED_SHA256 ?? null,
  });
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  if (!result.ok) process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
