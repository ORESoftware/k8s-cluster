import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const runtimeConfig = readFileSync(
  'remote/deployments/dev-server/src/runtime-config.ts',
  'utf8',
);
const devServer = readFileSync(
  'remote/deployments/dev-server/src/server.ts',
  'utf8',
);
const gateway = readFileSync(
  'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  'utf8',
);
const browserMcp = readFileSync(
  'remote/deployments/browser-mcp-rs/src/main.rs',
  'utf8',
);

function boundedBlock(source, start, end) {
  const startIndex = source.indexOf(start);
  assert.notEqual(startIndex, -1, `missing start anchor: ${start}`);
  const endIndex = source.indexOf(end, startIndex + start.length);
  assert.notEqual(endIndex, -1, `missing end anchor after ${start}: ${end}`);
  return source.slice(startIndex, endIndex);
}

function nginxLocation(source, route) {
  const start = `      location ${route} {`;
  const startIndex = source.indexOf(start);
  assert.notEqual(startIndex, -1, `missing nginx location ${route}`);
  const nextIndex = source.indexOf('\n      location ', startIndex + start.length);
  assert.notEqual(nextIndex, -1, `missing next nginx location after ${route}`);
  return source.slice(startIndex, nextIndex);
}

test('runtime-config snapshot read is protected by its module-owned server secret', () => {
  const route = boundedBlock(
    runtimeConfig,
    "fastify.get('/internal/runtime-config'",
    "fastify.post('/internal/update-runtime-config'",
  );

  assert.match(route, /runtimeConfigStore\.requireServerAuth/);
  assert.match(route, /reply\.code\(401\)/);
  assert.match(route, /error: 'unauthorized'/);
  assert.doesNotMatch(
    route,
    /async \(\) => runtimeConfigStore\.snapshot\(\)/,
    'the full entries map must never be returned by an unauthenticated one-line handler',
  );
});

test('dev-server secret checks are constant-time and anonymous health is minimal', () => {
  assert.match(devServer, /timingSafeEqual/);
  assert.match(devServer, /function secretEquals\(/);
  assert.match(devServer, /return timingSafeEqual\(givenBuf, expectedBuf\)/);
  assert.doesNotMatch(
    devServer,
    /req\.headers\['x-server-auth'\] !== config\.serverAuthSecret/,
  );

  const route = boundedBlock(
    devServer,
    "fastify.get('/healthz'",
    "fastify.get('/metrics'",
  );
  assert.match(
    route,
    /const base = \{ ok: true as const, startedAt: serverStartedAt \}/,
  );
  assert.match(route, /return base;/);
  assert.match(route, /pinnedUserId/);
  assert.ok(
    route.indexOf('return base;') < route.indexOf('pinnedUserId'),
    'anonymous callers must return before owner and queue details are assembled',
  );
});

test('gateway rejects anonymous direct dev-server control-plane aliases', () => {
  for (const route of ['/tasks', '/status', '/agents']) {
    const location = nginxLocation(gateway, route);
    assert.match(location, /if \(\$dd_gateway_auth_ok = 0\)/);
    assert.match(location, /return 401;/);
    assert.match(
      location,
      /set \$dd_up_\d+ dd-dev-server-api\.default\.svc\.cluster\.local:8080;/,
    );
    assert.match(location, /proxy_pass http:\/\/\$dd_up_\d+;/);
  }
});

test('browser MCP fails closed unless local no-auth mode is explicit', () => {
  assert.match(
    browserMcp,
    /env_bool\("BROWSER_MCP_REQUIRE_AUTH", true\)/,
  );
  assert.doesNotMatch(
    browserMcp,
    /env_bool\("BROWSER_MCP_REQUIRE_AUTH", false\)/,
  );
  assert.match(browserMcp, /UNAUTHENTICATED write-capable browser control/);
  assert.match(browserMcp, /never expose this process publicly/);
});
