import { spawnSync } from 'node:child_process';

import { initTelemetry } from '@dd/telemetry';

import { parseFlags2EnvOutput } from './flags-runtime.js';
import { assertTrustedProviderEndpoints } from './runtime-config-guard.js';

const cliArgs = process.argv.slice(2);
const parsed = spawnSync('flags2env', ['__dd_web_scraper__', ...cliArgs], {
  cwd: process.cwd(),
  env: process.env,
  encoding: 'utf8',
  maxBuffer: 1_048_576,
});

if (parsed.error) {
  throw new Error(`flags2env failed to start: ${parsed.error.message}`);
}
if (parsed.status !== 0) {
  if (parsed.stdout) process.stdout.write(parsed.stdout);
  if (parsed.stderr) process.stderr.write(parsed.stderr);
  throw new Error(`flags2env exited with status ${parsed.status ?? 'unknown'}`);
}

if (cliArgs.some((argument) => argument === '--help' || argument === '-h')) {
  process.stdout.write(parsed.stdout);
  process.exit(0);
}

const overrides = parseFlags2EnvOutput(parsed.stdout);

// flags2env has already applied the repository-owned precedence contract. Only
// declared non-secret keys can arrive here; secret-bearing variables are not
// represented in .cli-flags.toml and remain inherited from the pod environment.
for (const [key, value] of Object.entries(overrides)) {
  process.env[key] = value;
}

// Validate credential-bearing external endpoints before telemetry, network
// listeners, browser processes, or the local core can be started.
assertTrustedProviderEndpoints();

const telemetry = initTelemetry('dd-web-scraper-supervisor');
let telemetryClosing = false;

async function shutdownTelemetry(): Promise<void> {
  if (telemetryClosing) return;
  telemetryClosing = true;
  try {
    await telemetry.shutdown();
  } catch (error) {
    console.error(
      JSON.stringify({
        timestamp: new Date().toISOString(),
        level: 'error',
        service: 'dd-web-scraper-supervisor',
        event: 'telemetry_shutdown_failed',
        error: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

process.once('SIGTERM', () => void shutdownTelemetry());
process.once('SIGINT', () => void shutdownTelemetry());
process.once('beforeExit', () => void shutdownTelemetry());

try {
  await import('./scraper-supervisor.js');
} catch (error) {
  await shutdownTelemetry();
  throw error;
}
