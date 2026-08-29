import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { generateKeyPairSync, verify } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    try {
      if (statSync(resolve(candidate, 'scripts/ci/mint-github-app-installation-token.sh')).isFile()) {
        return candidate;
      }
    } catch {
      // Keep searching.
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

function readRequestBody(request: import('node:http').IncomingMessage): Promise<string> {
  return new Promise((resolveBody, reject) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk) => chunks.push(Buffer.from(chunk)));
    request.on('end', () => resolveBody(Buffer.concat(chunks).toString('utf8')));
    request.on('error', reject);
  });
}

function run(command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv }) {
  return new Promise<{ code: number | null; stdout: string; stderr: string }>((resolveRun, reject) => {
    const child = spawn(command, args, options);
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', (code) => resolveRun({ code, stdout, stderr }));
  });
}

const repoRoot = findRepoRoot();
const script = resolve(repoRoot, 'scripts/ci/mint-github-app-installation-token.sh');

test('mints an exact repository-scoped contents-read installation token without exposing it', async (t) => {
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const privateKeyPem = privateKey.export({ format: 'pem', type: 'pkcs8' }).toString();
  const workDir = mkdtempSync(join(tmpdir(), 'github-app-token-test-'));
  const tokenFile = join(workDir, 'installation-token');
  const installationToken = 'ghs_APPID_JWT_test-token-value-that-is-not-forty-characters';
  let installationLookupSeen = false;
  let tokenRequestSeen = false;

  const server = createServer(async (request, response) => {
    const authorization = request.headers.authorization ?? '';
    assert.match(authorization, /^Bearer [^.]+\.[^.]+\.[^.]+$/);
    assert.equal(request.headers['x-github-api-version'], '2026-03-10');

    const jwt = authorization.slice('Bearer '.length);
    const [encodedHeader, encodedPayload, encodedSignature] = jwt.split('.');
    const signingInput = `${encodedHeader}.${encodedPayload}`;
    const signature = Buffer.from(encodedSignature, 'base64url');
    assert.equal(verify('RSA-SHA256', Buffer.from(signingInput), publicKey, signature), true);

    const header = JSON.parse(Buffer.from(encodedHeader, 'base64url').toString('utf8'));
    const payload = JSON.parse(Buffer.from(encodedPayload, 'base64url').toString('utf8'));
    assert.deepEqual(header, { alg: 'RS256', typ: 'JWT' });
    assert.equal(payload.iss, 12345);
    assert.ok(payload.iat <= Math.floor(Date.now() / 1000));
    assert.ok(payload.exp > payload.iat);
    assert.ok(payload.exp - payload.iat <= 600);

    if (request.method === 'GET' && request.url === '/repos/example-org/repo-a/installation') {
      installationLookupSeen = true;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ id: 9876 }));
      return;
    }

    if (request.method === 'POST' && request.url === '/app/installations/9876/access_tokens') {
      tokenRequestSeen = true;
      const body = JSON.parse(await readRequestBody(request));
      assert.deepEqual(body, {
        repositories: ['repo-a', 'repo-b'],
        permissions: { contents: 'read' },
      });
      response.writeHead(201, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        token: installationToken,
        expires_at: '2026-07-27T21:00:00Z',
        permissions: { contents: 'read', metadata: 'read' },
        repositories: [
          { full_name: 'example-org/repo-b' },
          { full_name: 'example-org/repo-a' },
        ],
      }));
      return;
    }

    response.writeHead(404, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ message: 'not found' }));
  });

  await new Promise<void>((resolveListen) => server.listen(0, '127.0.0.1', resolveListen));
  t.after(() => new Promise<void>((resolveClose) => server.close(() => resolveClose())));
  const address = server.address();
  assert.ok(address && typeof address === 'object');

  const result = await run('bash', [script, 'example-org', tokenFile, 'repo-a', 'repo-b'], {
    cwd: repoRoot,
    env: {
      ...process.env,
      GITHUB_ACTIONS: '',
      GITHUB_API_URL: `http://127.0.0.1:${address.port}`,
      GITHUB_API_VERSION: '2026-03-10',
      K8S_SUBMODULE_APP_ID: '12345',
      K8S_SUBMODULE_APP_PRIVATE_KEY: privateKeyPem,
    },
  });

  assert.equal(result.code, 0, result.stderr);
  assert.equal(installationLookupSeen, true);
  assert.equal(tokenRequestSeen, true);
  assert.equal(readFileSync(tokenFile, 'utf8'), installationToken);
  assert.equal(statSync(tokenFile).mode & 0o777, 0o600);
  assert.doesNotMatch(result.stdout, new RegExp(installationToken));
  assert.doesNotMatch(result.stderr, new RegExp(installationToken));
  assert.match(
    result.stdout,
    /exact repository-scoped contents-read installation token for example-org covering 2 repository\/repositories/,
  );
});

test('rejects broader permissions and repository-scope substitution before writing a token', async (t) => {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const privateKeyPem = privateKey.export({ format: 'pem', type: 'pkcs8' }).toString();
  const scenarios = [
    {
      name: 'permission drift',
      permissions: { contents: 'read', metadata: 'read', issues: 'read' },
      repositories: [{ full_name: 'example-org/repo-a' }, { full_name: 'example-org/repo-b' }],
    },
    {
      name: 'repository substitution',
      permissions: { contents: 'read', metadata: 'read' },
      repositories: [{ full_name: 'example-org/repo-a' }, { full_name: 'example-org/repo-c' }],
    },
    {
      name: 'missing repository proof',
      permissions: { contents: 'read', metadata: 'read' },
      repositories: undefined,
    },
  ] as const;

  for (const scenario of scenarios) {
    await t.test(scenario.name, async (scenarioTest) => {
      const workDir = mkdtempSync(join(tmpdir(), 'github-app-token-reject-test-'));
      const tokenFile = join(workDir, 'installation-token');
      const installationToken = `ghs_${scenario.name.replaceAll(' ', '_')}_secret-token-value`;

      const server = createServer(async (request, response) => {
        if (request.method === 'GET' && request.url === '/repos/example-org/repo-a/installation') {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ id: 9876 }));
          return;
        }
        if (request.method === 'POST' && request.url === '/app/installations/9876/access_tokens') {
          await readRequestBody(request);
          response.writeHead(201, { 'content-type': 'application/json' });
          response.end(JSON.stringify({
            token: installationToken,
            expires_at: '2026-07-27T21:00:00Z',
            permissions: scenario.permissions,
            ...(scenario.repositories === undefined ? {} : { repositories: scenario.repositories }),
          }));
          return;
        }
        response.writeHead(404, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ message: 'not found' }));
      });

      await new Promise<void>((resolveListen) => server.listen(0, '127.0.0.1', resolveListen));
      scenarioTest.after(() => new Promise<void>((resolveClose) => server.close(() => resolveClose())));
      const address = server.address();
      assert.ok(address && typeof address === 'object');

      const result = await run('bash', [script, 'example-org', tokenFile, 'repo-a', 'repo-b'], {
        cwd: repoRoot,
        env: {
          ...process.env,
          GITHUB_ACTIONS: '',
          GITHUB_API_URL: `http://127.0.0.1:${address.port}`,
          GITHUB_API_VERSION: '2026-03-10',
          K8S_SUBMODULE_APP_ID: '12345',
          K8S_SUBMODULE_APP_PRIVATE_KEY: privateKeyPem,
        },
      });

      assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
      assert.equal(existsSync(tokenFile), false);
      assert.doesNotMatch(result.stdout, new RegExp(installationToken));
      assert.doesNotMatch(result.stderr, new RegExp(installationToken));
      assert.match(
        `${result.stdout}\n${result.stderr}`,
        /exact repository-scoped contents-read installation token/,
      );
    });
  }
});

test('rejects duplicate repository names case-insensitively before using credentials', async () => {
  const workDir = mkdtempSync(join(tmpdir(), 'github-app-token-duplicate-test-'));
  const tokenFile = join(workDir, 'installation-token');
  const result = await run('bash', [script, 'example-org', tokenFile, 'repo-a', 'REPO-A'], {
    cwd: repoRoot,
    env: {
      ...process.env,
      K8S_SUBMODULE_APP_ID: '',
      K8S_SUBMODULE_APP_PRIVATE_KEY: '',
    },
  });

  assert.equal(result.code, 64);
  assert.equal(existsSync(tokenFile), false);
  assert.match(`${result.stdout}\n${result.stderr}`, /requested more than once/);
  assert.doesNotMatch(`${result.stdout}\n${result.stderr}`, /App ID missing|private key missing/);
});
