import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/dev-server/package.json'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('node thread workers can dispatch tasks through the warm container pool service', async () => {
  for (const root of ['remote/dev-server', 'remote/dev-server-local']) {
    const source = await readRepoFile(`${root}/src/server.ts`);
    const client = await readRepoFile(`${root}/src/container-pool.ts`);

    assert.match(source, /containerPoolConfigFromEnv/);
    assert.match(source, /containerPoolConfigured/);
    assert.match(source, /dispatchContainerPool/);
    assert.match(source, /containerPoolConfigured:\s*containerPoolConfigured\(config\.containerPool\)/);
    assert.match(source, /kind:\s*["']container_pool["']/);
    assert.match(source, /container-pool-dispatch:\$\{state\.containerPool\.pool\}/);
    assert.match(source, /ContainerPoolRequestSchema/);
    assert.match(source, /ContainerPoolTaskSchema/);
    assert.match(source, /containerPool:\s*ContainerPoolTaskSchema\.optional\(\)/);
    assert.match(source, /fastify\.post\(["']\/container-pools\/:pool\/dispatch["']/);
    assert.match(source, /poolSlug:\s*parsed\.data\.poolSlug \?\? params\.data\.pool/);
    assert.match(source, /poolSlug:\s*state\.containerPool\.request\.poolSlug \?\? state\.containerPool\.pool/);
    assert.doesNotMatch(source, /buildAgentEnv[\s\S]*CONTAINER_POOL_AUTH_SECRET/);

    assert.match(client, /CONTAINER_POOL_BASE_URL/);
    assert.match(client, /CONTAINER_POOL_URL/);
    assert.match(client, /CONTAINER_POOL_AUTH_SECRET/);
    assert.match(client, /SERVER_AUTH_SECRET/);
    assert.match(client, /CONTAINER_POOL_DISPATCH_TIMEOUT_MS/);
    assert.match(client, /\/pools\/\$\{encodeURIComponent\(pool\)\}\/dispatch/);
    assert.match(client, /x-server-auth/);
    assert.match(client, /AbortController/);
  }
});
