import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../../..');
const registerScript = path.join(root, 'scripts/ops/register_gha_clone_budget_webhook.sh');
const canaryScript = path.join(root, 'scripts/ops/canary_gha_clone_budget_webhook.py');

function writeExecutable(file, content) {
  fs.writeFileSync(file, content, { mode: 0o700 });
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    ...options,
  });
}

test('operator scripts parse before any credentialed operation', () => {
  const bash = run('bash', ['-n', registerScript]);
  assert.equal(bash.status, 0, bash.stderr);
  const python = run('python3', ['-m', 'py_compile', canaryScript], {
    env: { ...process.env, PYTHONPYCACHEPREFIX: fs.mkdtempSync(path.join(os.tmpdir(), 'gha-canary-pyc-')) },
  });
  assert.equal(python.status, 0, python.stderr);
});

test('registration uses a normalized secret file and rejects ambiguous duplicate hooks', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'gha-hook-tool-'));
  const bin = path.join(temp, 'bin');
  fs.mkdirSync(bin);
  const argsLog = path.join(temp, 'args.log');
  const payload = path.join(temp, 'payload.json');
  const secret = '0123456789abcdef0123456789abcdef';
  const url = 'https://ci.example.test/gha-webhooks/github';

  writeExecutable(
    path.join(bin, 'gh'),
    `#!/usr/bin/env bash
set -Eeuo pipefail
printf '%s\\n' "$*" >>"$GH_MOCK_ARGS"
[[ -z "\${GH_DEBUG:-}" ]]
[[ -z "\${DEBUG:-}" ]]
[[ "$*" == *"--hostname github.com"* ]]
if [[ "$*" == *"?per_page=100"* ]]; then
  if [[ "\${GH_MOCK_DUPLICATE:-0}" == 1 ]]; then
    printf '[{"id":11,"config":{"url":"%s"}},{"id":12,"config":{"url":"%s"}}]\\n' "$GH_MOCK_URL" "$GH_MOCK_URL"
  else
    printf '[]\\n'
  fi
  exit 0
fi
input=''
while (($#)); do
  if [[ "$1" == --input ]]; then input="$2"; break; fi
  shift
done
[[ -n "$input" ]]
cp "$input" "$GH_MOCK_PAYLOAD"
jq -e --arg url "$GH_MOCK_URL" --arg secret "$GH_MOCK_SECRET" '.events == ["workflow_run"] and .active == true and .config.url == $url and .config.secret == $secret and .config.insecure_ssl == "0"' "$input" >/dev/null
printf '{"id":77,"active":true,"events":["workflow_run"],"config":{"url":"%s","content_type":"json","insecure_ssl":"0"}}\\n' "$GH_MOCK_URL"
`,
  );

  const secretFile = path.join(temp, 'secret');
  fs.writeFileSync(secretFile, `${secret}\n`, { mode: 0o600 });
  const env = {
    ...process.env,
    GH_DEBUG: 'api',
    DEBUG: '1',
    PATH: `${bin}:${process.env.PATH}`,
    GH_MOCK_ARGS: argsLog,
    GH_MOCK_PAYLOAD: payload,
    GH_MOCK_URL: url,
    GH_MOCK_SECRET: secret,
  };

  const created = run(
    registerScript,
    ['--repository', 'example/repository', '--url', url, '--secret-file', secretFile],
    { env },
  );
  assert.equal(created.status, 0, created.stderr);
  assert.match(created.stdout, /created webhook id=77/);
  const recordedArgs = fs.readFileSync(argsLog, 'utf8');
  assert.doesNotMatch(recordedArgs, new RegExp(secret));
  assert.match(recordedArgs, /--hostname github\.com/);
  assert.equal(JSON.parse(fs.readFileSync(payload, 'utf8')).config.secret, secret);

  const duplicate = run(
    registerScript,
    ['--repository', 'example/repository', '--url', url, '--secret-file', secretFile],
    { env: { ...env, GH_MOCK_DUPLICATE: '1' } },
  );
  assert.notEqual(duplicate.status, 0);
  assert.match(duplicate.stderr, /multiple hooks already use the exact callback URL/);
});

test('registration validates normalized bytes and owner-only file metadata', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'gha-hook-secret-'));
  const bin = path.join(temp, 'bin');
  fs.mkdirSync(bin);
  writeExecutable(path.join(bin, 'gh'), '#!/usr/bin/env sh\nexit 99\n');
  const url = 'https://ci.example.test/gha-webhooks/github';
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}` };

  const shortSecret = path.join(temp, 'short');
  fs.writeFileSync(shortSecret, `${'x'.repeat(31)}\n`, { mode: 0o600 });
  const short = run(
    registerScript,
    ['--repository', 'example/repository', '--url', url, '--secret-file', shortSecret],
    { env },
  );
  assert.notEqual(short.status, 0);
  assert.match(short.stderr, /32 to 4096 bytes/);

  const publicSecret = path.join(temp, 'public');
  fs.writeFileSync(publicSecret, 'x'.repeat(32), { mode: 0o644 });
  fs.chmodSync(publicSecret, 0o644);
  const publicResult = run(
    registerScript,
    ['--repository', 'example/repository', '--url', url, '--secret-file', publicSecret],
    { env },
  );
  assert.notEqual(publicResult.status, 0);
  assert.match(publicResult.stderr, /must not be group\/world accessible/);

  const realSecret = path.join(temp, 'real');
  fs.writeFileSync(realSecret, 'x'.repeat(32), { mode: 0o600 });
  const link = path.join(temp, 'link');
  fs.symlinkSync(realSecret, link);
  const symlink = run(
    registerScript,
    ['--repository', 'example/repository', '--url', url, '--secret-file', link],
    { env },
  );
  assert.notEqual(symlink.status, 0);
  assert.match(symlink.stderr, /non-symlink regular file/);
});

test('canary opens owner-only regular secret files without following symlinks', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'gha-canary-secret-'));
  const valid = path.join(temp, 'valid');
  const publicSecret = path.join(temp, 'public');
  const link = path.join(temp, 'link');
  fs.writeFileSync(valid, `${'v'.repeat(32)}\n`, { mode: 0o600 });
  fs.writeFileSync(publicSecret, 'p'.repeat(32), { mode: 0o644 });
  fs.chmodSync(publicSecret, 0o644);
  fs.symlinkSync(valid, link);

  const probe = String.raw`
import importlib.util
import sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("gha_canary", sys.argv[1])
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
value = module.read_secret(Path(sys.argv[2]), "test secret")
assert value == b"v" * 32
`;
  const validResult = run('python3', ['-c', probe, canaryScript, valid]);
  assert.equal(validResult.status, 0, validResult.stderr);

  const publicResult = run('python3', ['-c', probe, canaryScript, publicSecret]);
  assert.notEqual(publicResult.status, 0);
  assert.match(publicResult.stderr, /must not be group\/world accessible/);

  const symlinkResult = run('python3', ['-c', probe, canaryScript, link]);
  assert.notEqual(symlinkResult.status, 0);
  assert.match(symlinkResult.stderr, /cannot securely open test secret file/);
});
