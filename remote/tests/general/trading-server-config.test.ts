import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/trading-server-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust trading server scores signals and emits gated order intents', async () => {
  const cargo = await readRepoFile('remote/trading-server-rs/Cargo.toml');
  const source = await readRepoFile('remote/trading-server-rs/src/main.rs');
  const readme = await readRepoFile('remote/trading-server-rs/readme.md');
  const appConfigSeed = await readRepoFile(
    'remote/databases/pg/seeds/trading-platform-app-config.sql',
  );
  // Single source of truth for shared table DDL; per-table dupes were retired.
  const appConfigTable = await readRepoFile('remote/libs/pg-defs/schema/schema.sql');

  assert.match(cargo, /name\s*=\s*"dd-trading-server"/);
  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  assert.match(cargo, /tokio-postgres/);
  assert.match(cargo, /tokio-postgres-rustls/);
  assert.match(cargo, /rustls-pemfile/);
  assert.match(source, /const SCHEMA_VERSION: &str = "trading\.decision\.v1"/);
  assert.match(source, /struct TradingPlatform/);
  assert.match(source, /struct TradingPlatformConfig/);
  assert.match(source, /struct DecisionRequest/);
  assert.match(source, /struct RiskLimits/);
  assert.match(source, /struct OrderIntent/);
  assert.match(source, /TRADING_APP_CONFIG_KEY/);
  assert.match(source, /trading\.platforms\.v1/);
  assert.match(source, /from app_config/);
  assert.match(source, /rds-us-east-1-bundle\.pem/);
  assert.match(source, /add_rds_root_certificates/);
  assert.match(source, /fetch_platform_config_from_app_config/);
  assert.match(source, /refresh_platform_config/);
  assert.match(source, /fn conservative_cap/);
  assert.match(source, /fn conservative_floor/);
  assert.match(source, /fn readyz/);
  assert.match(source, /public_platform_descriptors/);
  assert.match(source, /dd_trading_server_config_refresh_total/);
  assert.match(source, /target_platform/);
  assert.match(source, /platformConfigured/);
  assert.match(source, /platformModeSupported/);
  assert.match(source, /fn score_web_signals/);
  assert.match(source, /fn score_ml_features/);
  assert.match(source, /fn score_market_momentum/);
  assert.match(source, /fn score_mdp_policy/);
  assert.match(source, /fn evaluate_decision/);
  assert.match(source, /recommended_action/);
  assert.match(source, /final_action/);
  assert.match(source, /blocked_by_safety_gate/);
  assert.match(source, /TRADING_ALLOW_LIVE_ORDERS/);
  assert.match(source, /SERVER_AUTH_SECRET/);
  assert.match(source, /constant_time_equals/);
  assert.match(source, /queue_subscribe/);
  assert.match(source, /dd\.remote\.trading\.signals/);
  assert.match(source, /dd\.remote\.trading\.decisions/);
  assert.match(source, /dd\.remote\.trading\.order_intents/);
  assert.match(source, /dd_trading_server_order_intents_total/);
  assert.match(source, /\.route\("\/schema", get\(schema\)\)/);
  assert.match(source, /\.route\("\/example", get\(example\)\)/);
  assert.match(source, /\.route\("\/readyz", get\(readyz\)\)/);
  assert.match(source, /\.route\("\/decide", post\(decide_http\)\)/);

  assert.match(readme, /orchestrator, not an exchange executor/);
  assert.match(readme, /`POST \/decide`/);
  assert.match(readme, /`GET \/readyz`/);
  assert.match(readme, /TRADING_MODE=paper/);
  assert.match(readme, /TRADING_ALLOW_LIVE_ORDERS=false/);
  assert.match(readme, /trading-platform-app-config\.sql/);
  assert.match(readme, /constraints` can only tighten/);
  assert.match(readme, /interactive-brokers/);
  assert.match(readme, /alpaca/);
  assert.match(readme, /tradier/);
  assert.match(readme, /coinbase-advanced-trade/);
  assert.match(readme, /kraken/);
  assert.match(readme, /gemini/);
  assert.match(readme, /binance-us/);
  assert.match(readme, /polymarket/);
  assert.match(readme, /factmachine/);
  assert.match(readme, /dd\.remote\.trading\.order_intents/);
  assert.match(appConfigTable, /create table if not exists app_config/);
  assert.match(appConfigSeed, /insert into app_config \(scope, key, value, version, status, labels, meta_data\)/);
  assert.match(appConfigSeed, /'trading\.platforms\.v1'/);
  assert.match(appConfigSeed, /"slug": "interactive-brokers"/);
  assert.match(appConfigSeed, /"slug": "alpaca"/);
  assert.match(appConfigSeed, /"slug": "tradier"/);
  assert.match(appConfigSeed, /"slug": "coinbase-advanced-trade"/);
  assert.match(appConfigSeed, /"slug": "kraken"/);
  assert.match(appConfigSeed, /"slug": "gemini"/);
  assert.match(appConfigSeed, /"slug": "binance-us"/);
  assert.match(appConfigSeed, /"slug": "polymarket"[\s\S]*"status": "paused"/);
  assert.match(appConfigSeed, /"slug": "factmachine"[\s\S]*"endpointStatus": "not-configured"/);
  assert.match(appConfigSeed, /dd-trading-broker-secrets/);
  assert.doesNotMatch(appConfigSeed, /SECRET_KEY":\s*"[A-Za-z0-9+/=]{20,}/);
});

test('trading server is deployed through runtime manifests, gateway, and observability', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-trading-server.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-trading-server.service.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');

  assert.match(deployment, /name:\s*dd-trading-server/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8103'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /TRADING_MODE[\s\S]*value:\s*paper/);
  assert.match(deployment, /TRADING_ALLOW_LIVE_ORDERS[\s\S]*value:\s*'false'/);
  assert.match(deployment, /TRADING_ALLOW_UNAUTHENTICATED[\s\S]*value:\s*'false'/);
  assert.match(deployment, /TRADING_APP_CONFIG_SCOPE[\s\S]*value:\s*default/);
  assert.match(deployment, /TRADING_APP_CONFIG_KEY[\s\S]*value:\s*trading\.platforms\.v1/);
  assert.match(deployment, /TRADING_CONFIG_REFRESH_SECONDS[\s\S]*value:\s*'30'/);
  assert.match(deployment, /dd-remote-rest-api-secrets/);
  assert.doesNotMatch(deployment, /secretRef:\s*\n\s*name:\s*dd-trading-broker-secrets/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /SCRAPER_BASE_URL[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/);
  assert.match(deployment, /ML_PIPELINE_BASE_URL[\s\S]*dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099/);
  assert.match(deployment, /MDP_OPTIMIZER_BASE_URL[\s\S]*dd-mdp-optimizer\.default\.svc\.cluster\.local:8096/);
  assert.match(deployment, /TRADING_SIGNAL_SUBJECT[\s\S]*dd\.remote\.trading\.signals/);
  assert.match(deployment, /TRADING_ORDER_INTENT_SUBJECT[\s\S]*dd\.remote\.trading\.order_intents/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-trading-server/);
  assert.match(service, /port:\s*8103/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(kustomization, /dd-trading-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-trading-server\.service\.yaml/);
  assert.match(gateway, /location = \/trading[\s\S]*return 302 \/trading\//);
  assert.match(
    gateway,
    /location \/trading\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-trading-server\.default\.svc\.cluster\.local:8103\//,
  );
  assert.match(
    prometheus,
    /job_name:\s*dd-trading-server[\s\S]*dd-trading-server\.default\.svc\.cluster\.local:8103/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-trading-server[\s\S]*dd-trading-server\.default\.svc\.cluster\.local:8103/,
  );
  assert.match(home, /dd-trading-server/);
  assert.match(home, /POST \/trading\/decide/);
  assert.match(home, /dd\.remote\.trading\.order_intents/);
  assert.match(runtimeReadme, /dd-trading-server/);
  assert.match(runtimeReadme, /`POST \/trading\/decide`/);
  assert.match(remoteReadme, /trading-server-rs/);
});
