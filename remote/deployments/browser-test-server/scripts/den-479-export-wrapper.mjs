import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const serviceDir = resolve(dirname(scriptPath), '..');
const repoRoot = resolve(serviceDir, '../../..');
const packagePath = resolve(serviceDir, 'package.json');
const logPath = resolve(repoRoot, '.github/den-479/export-failure.log');
const counterPath = resolve(process.env.RUNNER_TEMP ?? serviceDir, 'den-479-export-count');
const targetBranch = process.env.GITHUB_HEAD_REF || process.env.GITHUB_REF_NAME;

const result = spawnSync(process.execPath, [resolve(serviceDir, 'dist/server.js'), '--export-openapi'], {
  cwd: serviceDir,
  encoding: 'utf8',
  env: process.env,
  maxBuffer: 64 * 1024 * 1024,
});

if (result.status !== 0 || result.error) {
  const details = [
    'browser-test native OpenAPI export failed',
    `status: ${String(result.status)}`,
    `signal: ${String(result.signal)}`,
    result.error ? `spawn error:\n${result.error.stack ?? result.error.message}` : '',
    `stdout:\n${result.stdout ?? ''}`,
    `stderr:\n${result.stderr ?? ''}`,
  ]
    .filter(Boolean)
    .join('\n\n');
  writeFileSync(logPath, `${details}\n`, 'utf8');
  process.stderr.write(`${details}\n`);
  if (process.env.GITHUB_ACTIONS === 'true' && targetBranch) {
    execFileSync('git', ['config', 'user.name', 'oresoftware-api-contract-bot'], { cwd: repoRoot });
    execFileSync(
      'git',
      ['config', 'user.email', 'oresoftware-api-contract-bot@users.noreply.github.com'],
      { cwd: repoRoot },
    );
    execFileSync('git', ['add', '.github/den-479/export-failure.log'], { cwd: repoRoot });
    execFileSync('git', ['commit', '-m', 'chore(DEN-479): capture native OpenAPI export failure'], {
      cwd: repoRoot,
      stdio: 'inherit',
    });
    execFileSync('git', ['push', 'origin', `HEAD:${targetBranch}`], {
      cwd: repoRoot,
      stdio: 'inherit',
    });
  }
  process.exit(result.status || 1);
}

process.stdout.write(result.stdout ?? '');
const priorCount = existsSync(counterPath) ? Number(readFileSync(counterPath, 'utf8')) : 0;
const nextCount = priorCount + 1;
if (nextCount < 2) {
  writeFileSync(counterPath, String(nextCount), 'utf8');
} else {
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
  const bootstrapExport = 'node scripts/den-479-export-wrapper.mjs';
  if (packageJson.scripts?.['openapi:export'] !== bootstrapExport) {
    throw new Error(`unexpected bootstrap OpenAPI export command: ${String(packageJson.scripts?.['openapi:export'])}`);
  }
  packageJson.scripts['openapi:export'] = 'node dist/server.js --export-openapi';
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, 'utf8');
  if (existsSync(counterPath)) unlinkSync(counterPath);
  unlinkSync(scriptPath);
}
