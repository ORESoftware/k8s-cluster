import assert from 'node:assert/strict';
import test from 'node:test';

import {
  clusterMcpAuthHeadersFromEnv,
  clusterMcpConnectTimeoutMs,
  clusterMcpUrlFromEnv,
} from './cluster-mcp.js';

test('cluster MCP client fails closed when no scoped credential is present', () => {
  assert.equal(clusterMcpAuthHeadersFromEnv({}), undefined);
  assert.equal(clusterMcpAuthHeadersFromEnv({ AGENT_MCP_AUTH_SECRET: '   ' }), undefined);
});

test('cluster MCP client sends the scoped secret only in X-Server-Auth', () => {
  assert.deepEqual(
    clusterMcpAuthHeadersFromEnv({ AGENT_MCP_AUTH_SECRET: '  cluster-secret  ' }),
    { 'x-server-auth': 'cluster-secret' },
  );
});

test('cluster MCP endpoint and timeout settings remain bounded', () => {
  assert.equal(
    clusterMcpUrlFromEnv({ AGENT_MCP_URL: ' http://dd-cluster-mcp-rs:8091/mcp ' }),
    'http://dd-cluster-mcp-rs:8091/mcp',
  );
  assert.equal(
    clusterMcpUrlFromEnv({
      AGENT_MCP_ENABLED: 'false',
      AGENT_MCP_URL: 'http://dd-cluster-mcp-rs:8091/mcp',
    }),
    undefined,
  );
  assert.equal(clusterMcpConnectTimeoutMs({ AGENT_MCP_CONNECT_TIMEOUT_MS: '999999' }), 30_000);
  assert.equal(clusterMcpConnectTimeoutMs({ AGENT_MCP_CONNECT_TIMEOUT_MS: 'invalid' }), 3000);
});
