import { spawn, spawnSync } from 'node:child_process';

const cliArgs = process.argv.slice(2);
const flags2envArgs = ['__dd_web_scraper__', ...cliArgs];
const resolved = spawnSync('flags2env', flags2envArgs, {
  cwd: process.cwd(),
  env: process.env,
  encoding: 'utf8',
  maxBuffer: 1_048_576,
});

if (resolved.error) {
  throw new Error(`flags2env failed to start: ${resolved.error.message}`);
}
if (resolved.status !== 0) {
  if (resolved.stdout) process.stdout.write(resolved.stdout);
  if (resolved.stderr) process.stderr.write(resolved.stderr);
  throw new Error(`flags2env exited with status ${resolved.status ?? 'unknown'}`);
}

if (cliArgs.some((arg) => arg === '--help' || arg === '-h')) {
  process.stdout.write(resolved.stdout);
  process.exit(0);
}

let overrides: Record<string, string>;
try {
  const parsed = JSON.parse(resolved.stdout) as unknown;
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('expected a JSON object');
  }
  overrides = Object.fromEntries(
    Object.entries(parsed).map(([key, value]) => [key, String(value)]),
  );
} catch (error) {
  throw new Error(
    `flags2env returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`,
  );
}

// The flags2env result already applies its declared source precedence. Secrets
// not declared as CLI flags remain inherited only from the process environment.
const child = spawn(process.execPath, ['dist/gateway.js'], {
  cwd: process.cwd(),
  env: { ...process.env, ...overrides },
  stdio: 'inherit',
});

for (const signal of ['SIGTERM', 'SIGINT'] as const) {
  process.on(signal, () => {
    if (!child.killed) child.kill(signal);
  });
}

child.on('error', (error) => {
  process.stderr.write(`dd-web-scraper entrypoint child error: ${error.message}\n`);
  process.exitCode = 1;
});
child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exitCode = code ?? 1;
});
