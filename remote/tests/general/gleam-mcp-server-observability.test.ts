import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/gleam-mcp-server/gleam.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const mcpCwd = resolve(repoRoot, 'remote/deployments/gleam-mcp-server');

function sleep(ms: number): Promise<void> {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

async function openPort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createNetServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      assert.ok(address && typeof address === 'object');
      const port = address.port;
      server.close((error) => {
        if (error) {
          reject(error);
        } else {
          resolvePort(port);
        }
      });
    });
  });
}

function writeMockResponse(req: IncomingMessage, res: ServerResponse): void {
  const path = new URL(req.url ?? '/', 'http://127.0.0.1').pathname;
  const responses: Record<string, { contentType: string; body: string; status?: number }> = {
    '/-/healthy': { contentType: 'text/plain', body: 'Prometheus Server is Healthy.\n' },
    '/api/v1/targets': {
      contentType: 'application/json',
      body: JSON.stringify({ status: 'success', data: { activeTargets: [{ job: 'dd-gleam-mcp-server' }] } }),
    },
    '/ready': { contentType: 'text/plain', body: 'ready\n' },
    '/api/health': { contentType: 'application/json', body: JSON.stringify({ database: 'ok' }) },
    '/api/datasources': {
      contentType: 'application/json',
      body: JSON.stringify([
        { name: 'Prometheus', type: 'prometheus', secureJsonData: { apiToken: 'grafana-secret-token' } },
        { name: 'Loki', type: 'loki' },
      ]),
    },
    '/api/search': {
      contentType: 'application/json',
      body: JSON.stringify([{ title: 'Gleam MCP Runtime', type: 'dash-db' }]),
    },
    '/api/services': {
      contentType: 'application/json',
      body: JSON.stringify({ data: ['dd-gleam-mcp-server', 'dd-container-pool'] }),
    },
    '/loki/api/v1/labels': {
      contentType: 'application/json',
      body: JSON.stringify({ status: 'success', data: ['app', 'namespace', 'pod'] }),
    },
    '/varz': {
      contentType: 'application/json',
      body: JSON.stringify({ server_id: 'mock-nats', connections: 3, jetstream: true }),
    },
    '/metrics': {
      contentType: 'text/plain',
      body: [
        '# HELP gnatsd_varz_connections Number of active NATS connections.',
        'gnatsd_varz_connections 3',
        '# HELP otelcol_receiver_accepted_metric_points Accepted metric points.',
        'otelcol_receiver_accepted_metric_points 9',
        '',
      ].join('\n'),
    },
    '/apis/apps/v1/deployments': {
      contentType: 'application/json',
      body: JSON.stringify({
        apiVersion: 'meta.k8s.io/v1',
        kind: 'PartialObjectMetadataList',
        items: [
          { metadata: { name: 'dd-dev-server-api', namespace: 'default', labels: { token: 'k8s-secret-token' } } },
          { metadata: { name: 'dd-gleam-mcp-server', namespace: 'default' } },
        ],
      }),
    },
    '/api/v1/pods': {
      contentType: 'application/json',
      body: JSON.stringify({
        apiVersion: 'meta.k8s.io/v1',
        kind: 'PartialObjectMetadataList',
        items: [
          { metadata: { name: 'dd-dev-server-api-abc', namespace: 'default' } },
          { metadata: { name: 'dd-gleam-mcp-server-def', namespace: 'default' } },
        ],
      }),
    },
    '/api/v1/namespaces': {
      contentType: 'application/json',
      body: JSON.stringify({
        apiVersion: 'meta.k8s.io/v1',
        kind: 'PartialObjectMetadataList',
        items: [{ metadata: { name: 'default' } }, { metadata: { name: 'observability' } }],
      }),
    },
  };
  const response = responses[path] ?? {
    status: 404,
    contentType: 'application/json',
    body: JSON.stringify({ error: 'not found', path }),
  };
  res.writeHead(response.status ?? 200, {
    'content-type': response.contentType,
    'content-length': Buffer.byteLength(response.body),
  });
  res.end(response.body);
}

async function withMockTelemetryServer(callback: (baseUrl: string) => Promise<void>): Promise<void> {
  const port = await openPort();
  const server = createHttpServer(writeMockResponse);
  await new Promise<void>((resolveListen) => server.listen(port, '127.0.0.1', resolveListen));
  try {
    await callback(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolveClose, reject) =>
      server.close((error) => (error ? reject(error) : resolveClose())),
    );
  }
}

async function fetchJson(port: number, path: string, init?: RequestInit): Promise<any> {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, init);
  const text = await response.text();
  return JSON.parse(text);
}

async function stopProcess(processHandle: ChildProcessWithoutNullStreams): Promise<void> {
  if (processHandle.exitCode !== null || processHandle.signalCode !== null) {
    return;
  }
  const pid = processHandle.pid;
  if (!pid) {
    return;
  }
  try {
    process.kill(-pid, 'SIGTERM');
  } catch {
    processHandle.kill('SIGTERM');
  }
  await Promise.race([
    new Promise((resolveExit) => processHandle.once('exit', resolveExit)),
    sleep(5_000).then(() => {
      try {
        process.kill(-pid, 'SIGKILL');
      } catch {
        processHandle.kill('SIGKILL');
      }
    }),
  ]);
}

async function withMcpServer(baseUrl: string, callback: (port: number) => Promise<void>): Promise<void> {
  const port = await openPort();
  const tempDir = await mkdtemp(join(tmpdir(), 'dd-mcp-test-'));
  const tokenPath = join(tempDir, 'token');
  await writeFile(tokenPath, 'mock-kubernetes-token\n');
  const processHandle = spawn('gleam', ['run'], {
    cwd: mcpCwd,
    detached: true,
    env: {
      ...process.env,
      HOST: '127.0.0.1',
      PORT: String(port),
      MCP_PROMETHEUS_URL: baseUrl,
      MCP_LOKI_URL: baseUrl,
      MCP_GRAFANA_URL: baseUrl,
      MCP_TEMPO_URL: baseUrl,
      MCP_JAEGER_URL: baseUrl,
      MCP_OTEL_COLLECTOR_URL: baseUrl,
      MCP_NATS_MONITOR_URL: baseUrl,
      MCP_NATS_METRICS_URL: baseUrl,
      MCP_KUBERNETES_API_URL: baseUrl,
      MCP_KUBERNETES_TOKEN_PATH: tokenPath,
      MCP_OBSERVABILITY_TIMEOUT_MS: '500',
      MCP_OBSERVABILITY_BODY_LIMIT_BYTES: '4096',
      MCP_KUBERNETES_TIMEOUT_MS: '500',
      MCP_KUBERNETES_BODY_LIMIT_BYTES: '4096',
      MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES: '4096',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  processHandle.stdout.on('data', (chunk) => {
    output += chunk.toString();
  });
  processHandle.stderr.on('data', (chunk) => {
    output += chunk.toString();
  });

  try {
    const startedAt = Date.now();
    while (Date.now() - startedAt < 30_000) {
      assert.equal(processHandle.exitCode, null, `MCP server exited early:\n${output}`);
      try {
        const health = await fetchJson(port, '/healthz');
        if (health.ok === true) {
          await callback(port);
          return;
        }
      } catch {
        // Server is still starting.
      }
      await sleep(250);
    }
    assert.fail(`MCP server did not become healthy:\n${output}`);
  } finally {
    await stopProcess(processHandle);
    await rm(tempDir, { recursive: true, force: true });
  }
}

function rpcBody(toolName: string): string {
  return JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/call',
    params: { name: toolName, arguments: {} },
  });
}

test('Gleam MCP server reads bounded telemetry from observability and NATS endpoints', { timeout: 60_000 }, async () => {
  await withMockTelemetryServer(async (baseUrl) => {
    await withMcpServer(baseUrl, async (port) => {
      const listed = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: JSON.stringify({ jsonrpc: '2.0', id: 42, method: 'tools/list' }),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(listed.id, 42);
      const toolNames = listed.result.tools.map((tool: { name: string }) => tool.name);
      assert.ok(toolNames.includes('telemetry_summary'));
      assert.ok(toolNames.includes('kubernetes_inventory'));
      assert.ok(toolNames.includes('kubernetes_deployments'));
      assert.ok(toolNames.includes('human_access_policy'));
      assert.ok(toolNames.includes('grafana_inventory'));
      assert.ok(toolNames.includes('nats_metrics'));

      const pingWithToolText = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 43,
          method: 'ping',
          params: { note: 'tools/list' },
        }),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(pingWithToolText.id, 43);
      assert.deepEqual(pingWithToolText.result, {});

      const misplacedToolName = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 44,
          method: 'tools/call',
          params: { arguments: { name: 'kubernetes_inventory' } },
        }),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(misplacedToolName.id, 44);
      assert.equal(misplacedToolName.error.code, -32602);

      const summary = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: rpcBody('telemetry_summary'),
        headers: { 'content-type': 'application/json' },
      });
      const sources = summary.result.structuredContent.sources;
      const sourceByName = new Map(sources.map((source: { name: string }) => [source.name, source]));
      for (const name of [
        'prometheusTargets',
        'lokiLabels',
        'grafanaHealth',
        'grafanaDatasources',
        'grafanaDashboards',
        'tempoReady',
        'jaegerServices',
        'otelCollectorMetrics',
        'natsVarz',
        'natsExporterMetrics',
      ]) {
        const source = sourceByName.get(name) as { result?: { ok?: boolean; sample?: string } } | undefined;
        assert.equal(source?.result?.ok, true, `${name} should be reachable`);
      }
      assert.match((sourceByName.get('natsExporterMetrics') as any).result.sample, /gnatsd_varz_connections/);
      assert.match((sourceByName.get('grafanaDashboards') as any).result.sample, /Gleam MCP Runtime/);

      const grafana = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: rpcBody('grafana_inventory'),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(grafana.result.structuredContent.datasources.ok, true);
      assert.equal(grafana.result.structuredContent.dashboards.ok, true);
      assert.match(grafana.result.structuredContent.datasources.sample, /Prometheus/);
      assert.doesNotMatch(grafana.result.structuredContent.datasources.sample, /grafana-secret-token/);
      assert.match(grafana.result.structuredContent.datasources.sample, /<redacted>/);
      assert.match(grafana.result.structuredContent.dashboards.sample, /Gleam MCP Runtime/);

      const nats = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: rpcBody('nats_metrics'),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(nats.result.structuredContent.monitor.ok, true);
      assert.equal(nats.result.structuredContent.metrics.ok, true);
      assert.match(nats.result.structuredContent.monitor.sample, /mock-nats/);
      assert.match(nats.result.structuredContent.metrics.sample, /gnatsd_varz_connections/);

      const deployments = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: rpcBody('kubernetes_deployments'),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(deployments.result.structuredContent.readOnly, true);
      assert.equal(deployments.result.structuredContent.response.ok, true);
      assert.match(deployments.result.structuredContent.response.sample, /dd-dev-server-api/);
      assert.doesNotMatch(deployments.result.structuredContent.response.sample, /k8s-secret-token/);

      const inventory = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: rpcBody('kubernetes_inventory'),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(inventory.result.structuredContent.readOnly, true);
      assert.equal(inventory.result.structuredContent.metadataOnlyRequest, true);
      assert.match(JSON.stringify(inventory.result.structuredContent.resources), /dd-dev-server-api-abc/);
      assert.match(JSON.stringify(inventory.result.structuredContent.excluded), /secrets/);

      const accessPolicy = await fetchJson(port, '/mcp', {
        method: 'POST',
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 'mcp-client-check',
          method: 'tools/call',
          params: { name: 'human_access_policy', arguments: {} },
        }),
        headers: { 'content-type': 'application/json' },
      });
      assert.equal(accessPolicy.id, 'mcp-client-check');
      assert.equal(accessPolicy.result.structuredContent.elevatedMcpToolsEnabled, false);
      assert.match(accessPolicy.result.structuredContent.recommendedHumanProof, /TOTP/);
    });
  });
});
