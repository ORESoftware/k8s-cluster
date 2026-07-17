import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/contract-service-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust solana contract service is deployed, scraped, and guarded', async () => {
  const cargo = await readRepoFile('remote/deployments/contract-service-rs/Cargo.toml');
  const source = await readRepoFile('remote/deployments/contract-service-rs/src/main.rs');
  const coordination = await readRepoFile(
    'remote/deployments/contract-service-rs/src/coordination.rs',
  );
  const solanaFeatures = await readRepoFile(
    'remote/deployments/contract-service-rs/src/solana_features.rs',
  );
  const readme = await readRepoFile('remote/deployments/contract-service-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-contract-service.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-contract-service.service.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-contract-service.networkpolicy.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(cargo, /name = "dd-contract-service"/);
  assert.match(cargo, /async-nats = "=0\.38\.0"/);
  // Source-of-truth NATS subject + queue group constants come from the
  // generated @dd/nats-subject-defs crate.
  assert.match(cargo, /dd-nats-subject-defs\s*=\s*\{\s*path/);
  assert.match(
    source,
    /use dd_nats_subject_defs::\{[\s\S]*?CONTRACTS_SOLANA_RESULTS_SUBJECT[\s\S]*?CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP[\s\S]*?CONTRACTS_SOLANA_VALIDATE_SUBJECT[\s\S]*?RUNTIME_EVENTS_SUBJECT[\s\S]*?\};/,
  );
  assert.match(cargo, /reqwest[\s\S]*rustls-tls/);
  assert.match(cargo, /sqlx[\s\S]*postgres/);
  assert.match(cargo, /bs58/);
  assert.match(source, /const SCHEMA_VERSION: &str = "solana\.contract\.v1"/);
  assert.match(source, /struct ContractRequest/);
  assert.match(source, /fn validate_contract_request/);
  assert.match(source, /fn validate_pubkey/);
  assert.match(source, /fn validate_solana_rpc_url/);
  assert.match(source, /fn normalize_request_cluster/);
  assert.match(source, /fn authorize_send/);
  assert.match(source, /simulateTransaction/);
  assert.match(source, /sendTransaction/);
  assert.match(source, /mod coordination;/);
  assert.match(source, /mod solana_features;/);
  assert.match(source, /MAX_RPC_RESPONSE_BYTES/);
  assert.match(source, /try_acquire_owned/);
  assert.match(source, /redirect\(reqwest::redirect::Policy::none\(\)\)/);
  assert.match(source, /SOLANA_SEND_ENABLED/);
  assert.match(source, /CONTRACT_SEND_AUTH_SECRET/);
  assert.match(source, /SOLANA_ALLOW_SKIP_PREFLIGHT/);
  assert.match(source, /SOLANA_ALLOW_PRIVATE_RPC/);
  assert.match(source, /skipPreflight is disabled by policy/);
  assert.match(source, /sigVerify and replaceRecentBlockhash cannot both be true/);
  assert.match(source, /CONTRACTS_SOLANA_VALIDATE_SUBJECT/);
  assert.match(source, /CONTRACTS_SOLANA_RESULTS_SUBJECT/);
  assert.match(source, /CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP/);
  assert.match(source, /dd_contract_service_rpc_requests_total/);
  assert.match(source, /dd_contract_service_send_blocked_total/);
  assert.match(source, /DefaultBodyLimit::max\(MAX_HTTP_BODY_BYTES\)/);
  assert.match(source, /payload\.len\(\) > MAX_NATS_PAYLOAD_BYTES/);
  assert.match(source, /\.route\("\/healthz", get\(healthz\)\)/);
  assert.match(source, /\.route\("\/readyz", get\(readyz\)\)/);
  assert.match(source, /\.merge\(solana_features::router\(\)\)/);
  assert.match(source, /\.route\("\/schema", get\(schema_http\)\)/);
  assert.match(source, /\.route\("\/validate", post\(validate_http\)\)/);
  assert.match(source, /\.route\("\/simulate", post\(simulate_http\)\)/);
  assert.match(readme, /GET \/schema/);
  assert.match(readme, /POST \/simulate/);
  assert.match(readme, /schemaVersion": "solana\.contract\.v1"/);
  assert.match(readme, /CONTRACT_SEND_AUTH_SECRET/);
  assert.match(readme, /SOLANA_ALLOW_PRIVATE_RPC=true/);
  assert.match(readme, /pg_try_advisory_xact_lock/);
  assert.match(readme, /github\.com\/fiducia-cloud/);

  assert.match(coordination, /pg_try_advisory_xact_lock/);
  assert.match(coordination, /\/v1\/idempotency\/claim/);
  assert.match(coordination, /\/v1\/idempotency\/complete/);
  assert.match(coordination, /\/v1\/idempotency\/abandon/);
  assert.match(coordination, /fencing_token/);
  assert.match(solanaFeatures, /\.route\("\/program\/inspect"/);
  assert.match(solanaFeatures, /\.route\("\/program\/verify"/);
  assert.match(solanaFeatures, /\.route\("\/escrow\/inspect"/);
  assert.match(solanaFeatures, /getSignaturesForAddress/);
  assert.match(solanaFeatures, /getRecentPrioritizationFees/);
  assert.match(solanaFeatures, /allowed_github_orgs/);
  assert.match(solanaFeatures, /"languages": \["rs"\]/);

  assert.match(deployment, /name:\s*dd-contract-service/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/contract-service-rs/);
  assert.match(deployment, /CARGO_HOME=\/var\/cache\/dd-contract-service\/cargo/);
  assert.match(deployment, /cargo run --release --locked/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8101'/);
  assert.match(deployment, /SOLANA_CLUSTER[\s\S]*value:\s*devnet/);
  assert.match(deployment, /SOLANA_RPC_URL[\s\S]*https:\/\/api\.devnet\.solana\.com/);
  assert.match(deployment, /SOLANA_ALLOW_PRIVATE_RPC[\s\S]*value:\s*'false'/);
  assert.match(deployment, /SOLANA_SEND_ENABLED[\s\S]*value:\s*'false'/);
  assert.match(deployment, /SOLANA_ALLOW_SKIP_PREFLIGHT[\s\S]*value:\s*'false'/);
  assert.match(deployment, /CONTRACT_FORMAL_METHODS_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /FORMAL_METHODS_URL[\s\S]*dd-formal-methods-server/);
  assert.match(deployment, /CONTRACT_FORMAL_METHODS_GITHUB_ORGS[\s\S]*fiducia-cloud/);
  assert.match(deployment, /CONTRACT_COORDINATION_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /CONTRACT_COORDINATION_REQUIRED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /RDS_DATABASE_URL[\s\S]*dd-remote-rest-api-secrets/);
  assert.match(deployment, /FIDUCIA_LOCK_URL[\s\S]*fiducia-load-balance\.fiducia/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /CONTRACT_VALIDATE_SUBJECT[\s\S]*dd\.remote\.contracts\.solana\.validate/);
  assert.match(deployment, /CONTRACT_QUEUE_GROUP[\s\S]*dd-contract-service/);
  assert.match(deployment, /CONTRACT_RESULT_SUBJECT[\s\S]*dd\.remote\.contracts\.solana\.results/);
  assert.match(deployment, /CONTRACT_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*-\s*ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /name:\s*build-cache[\s\S]*emptyDir:\s*\{\}/);
  assert.match(deployment, /name:\s*tmp[\s\S]*emptyDir:\s*\{\}/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-contract-service/);
  assert.match(service, /port:\s*8101/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(networkPolicy, /app:\s*dd-formal-methods-server[\s\S]*port:\s*8110/);
  assert.match(networkPolicy, /app:\s*fiducia-load-balance[\s\S]*port:\s*8088/);
  assert.match(networkPolicy, /cidr:\s*10\.0\.0\.0\/8[\s\S]*port:\s*5432/);
  assert.match(kustomization, /dd-contract-service\.deployment\.yaml/);
  assert.match(kustomization, /dd-contract-service\.service\.yaml/);
  assert.match(gateway, /location = \/contracts[\s\S]*return 302 \/contracts\//);
  assert.match(gateway, /location \/contracts\/[\s\S]*dd-contract-service\.default\.svc\.cluster\.local:8101\//);
  assert.match(gateway, /location \/contracts\/[\s\S]*if \(\$dd_gateway_auth_ok = 0\)/);
  assert.match(prometheus, /job_name:\s*dd-contract-service/);
  assert.match(prometheus, /dd-contract-service\.default\.svc\.cluster\.local:8101/);
  assert.match(otel, /job_name:\s*dd-contract-service/);
  assert.match(otel, /dd-contract-service\.default\.svc\.cluster\.local:8101/);
  assert.match(home, /Rust Solana contract service/);
  assert.match(home, /\/contracts\/schema/);
  // web-home-rs now sources the displayed NATS subject from the generated
  // `dd-nats-subject-defs` crate so the operator dashboard stays in
  // lockstep with the source-of-truth schema.
  assert.match(home, /label: CONTRACTS_SOLANA_VALIDATE_SUBJECT/);
  assert.match(runtimeReadme, /`dd-contract-service`/);
  assert.match(runtimeReadme, /\/contracts\/schema/);
});

test('blockchain feature suite is wired in, keyless, and off by default', async () => {
  const cargo = await readRepoFile('remote/deployments/contract-service-rs/Cargo.toml');
  const main = await readRepoFile('remote/deployments/contract-service-rs/src/main.rs');
  const mod = await readRepoFile('remote/deployments/contract-service-rs/src/blockchain/mod.rs');
  const evm = await readRepoFile('remote/deployments/contract-service-rs/src/blockchain/evm.rs');
  const readme = await readRepoFile('remote/deployments/contract-service-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-contract-service.deployment.yaml',
  );
  const contractsSchema = await readRepoFile(
    'remote/libs/nats/subject-defs/schema/contracts.schema.json',
  );
  const generatedRust = await readRepoFile(
    'remote/libs/nats/subject-defs/generated/rust/src/lib.rs',
  );

  // keccak dependency for EVM EIP-55 checksums; module is mounted + merged.
  assert.match(cargo, /sha3 = "0\.10"/);
  assert.match(main, /mod blockchain;/);
  assert.match(main, /\.merge\(blockchain::router\(\)\)/);
  assert.match(main, /blockchain::BlockchainState::from_env/);
  assert.match(main, /pub\(crate\) async fn publish_blockchain_event/);

  // Keyless, custody-ready seam: only an External signer exists.
  assert.match(mod, /enum SignerBackend \{\s*External,?\s*\}/);
  assert.match(mod, /CONTRACT_BLOCKCHAIN_AUTH_SECRET/);
  assert.match(mod, /CONTRACT_BLOCKCHAIN_MAINNET_ENABLED/);
  // Startup gate refuses execute/broadcast without the secret / mainnet flag.
  assert.match(mod, /requires CONTRACT_BLOCKCHAIN_AUTH_SECRET/);
  assert.match(mod, /requires CONTRACT_BLOCKCHAIN_MAINNET_ENABLED=true/);
  // EVM client is keyless read/relay only (no key material).
  assert.match(evm, /fn validate_evm_address/);
  assert.match(evm, /fn evm_rpc/);

  // Every feature flag is present in the deployment and defaults to false.
  for (const flag of [
    'BLOCKCHAIN_WALLET_ENABLED',
    'BLOCKCHAIN_EXECUTOR_ENABLED',
    'BLOCKCHAIN_EXECUTOR_EXECUTE_ENABLED',
    'BLOCKCHAIN_RELAYER_ENABLED',
    'BLOCKCHAIN_RELAYER_BROADCAST_ENABLED',
    'BLOCKCHAIN_MULTISIG_ENABLED',
    'BLOCKCHAIN_INDEXER_ENABLED',
    'BLOCKCHAIN_MEV_ENABLED',
    'BLOCKCHAIN_NFT_ENABLED',
    'BLOCKCHAIN_STAKING_ENABLED',
    'BLOCKCHAIN_STAKING_EXECUTE_ENABLED',
    'BLOCKCHAIN_BRIDGE_ENABLED',
    'BLOCKCHAIN_BRIDGE_BROADCAST_ENABLED',
    'CONTRACT_BLOCKCHAIN_MAINNET_ENABLED',
  ]) {
    assert.match(deployment, new RegExp(`${flag}[\\s\\S]*?value:\\s*'false'`));
  }
  assert.match(deployment, /CONTRACT_BLOCKCHAIN_AUTH_SECRET[\s\S]*?secretKeyRef/);
  assert.match(deployment, /EVM_RPC_URL/);

  // Publish-only subjects exist in the schema and generated constants.
  assert.match(contractsSchema, /dd\.remote\.blockchain\.index\.events/);
  assert.match(contractsSchema, /dd\.remote\.blockchain\.mev\.alerts/);
  assert.match(contractsSchema, /dd\.remote\.blockchain\.bridge\.attestations/);
  assert.match(generatedRust, /BLOCKCHAIN_INDEX_EVENTS_SUBJECT/);
  assert.match(generatedRust, /BLOCKCHAIN_MEV_ALERTS_SUBJECT/);
  assert.match(generatedRust, /BLOCKCHAIN_BRIDGE_ATTESTATIONS_SUBJECT/);

  // Readme documents the keyless stance and the MEV monitoring-only posture.
  assert.match(readme, /Blockchain Feature Suite/);
  assert.match(readme, /no private keys are stored/);
  assert.match(readme, /monitoring-only/);
});
