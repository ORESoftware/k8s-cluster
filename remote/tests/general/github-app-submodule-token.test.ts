import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { generateKeyPairSync, verify } from 'node:crypto';
import { mkdtempSync, readFileSync, statSync } from 'node:fs';
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

test('mints a repository-restricted contents-read installation token without exposing it', async (t) => {
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
  assert.match(result.stdout, /contents-read installation token for example-org covering 2 repository\/repositories/);
});
