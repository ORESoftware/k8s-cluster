#!/usr/bin/env node

import { promises as fs } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

import { materializePlan } from './reaper-core.mjs';
import { createGitHubClient } from './reaper-github.mjs';
import { createLinearClient } from './reaper-linear.mjs';
import { integerFromEnv, uniqueSorted } from './reaper-utils.mjs';

export { materializePlan } from './reaper-core.mjs';
export { fetchJsonWithRetry } from './reaper-http.mjs';
export { buildContentFreeLinearSection } from './reaper-provenance.mjs';
export { redactSensitiveText, safeIssueTitle } from './reaper-utils.mjs';

const DEFAULT_TEAM_ID = 'eb8ab169-5afe-4b6f-9cab-3f2aa3e887dc';
const DEFAULT_PARENT_ISSUE = 'DEN-3473';
const DEFAULT_ALLOWED_GITHUB_OWNERS = ['ORESoftware'];
const DEFAULT_MAX_CREATES = 25;

function usage() {
  return `Usage:
  node tools/google-chat-space-export/reaper-materializer.mjs export-index \\
    --json <private-linear-index.json>

  node tools/google-chat-space-export/reaper-materializer.mjs materialize \\
    --plan <private-import-plan.json> \\
    --evidence <private-coverage-evidence.json> \\
    --summary <content-free-summary.json>

Required environment for live mutation:
  LINEAR_API_KEY       Personal API key stored as a protected Actions secret.

Optional environment:
  LINEAR_TEAM_ID       Defaults to the Denman team.
  LINEAR_PARENT_ISSUE  Defaults to DEN-3473.
  GITHUB_TOKEN         Enables pull-request/default-branch evidence lookup.
  GITHUB_ALLOWED_OWNERS Comma-separated fallback owners; defaults to ORESoftware.
  REAPER_MAX_LINEAR_CREATES  Per-run circuit breaker; defaults to 25.
`;
}

function parseArgs(argv) {
  const options = {
    command: null,
    json: null,
    plan: null,
    evidence: null,
    summary: null,
    help: false,
  };
  if (argv[0] && !argv[0].startsWith('-')) options.command = argv.shift();
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith('--')) {
        throw new Error(`Missing value for ${arg}`);
      }
      return argv[index];
    };
    switch (arg) {
      case '--json': options.json = next(); break;
      case '--plan': options.plan = next(); break;
      case '--evidence': options.evidence = next(); break;
      case '--summary': options.summary = next(); break;
      case '--help':
      case '-h': options.help = true; break;
      default: throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}


async function readJson(pathname) {
  try {
    return JSON.parse(await fs.readFile(pathname, 'utf8'));
  } catch (error) {
    throw new Error(`Could not read JSON ${pathname}: ${error.message}`);
  }
}

async function writeJson(destination, value, mode = 0o600) {
  const resolved = path.resolve(destination);
  await fs.mkdir(path.dirname(resolved), { recursive: true, mode: 0o700 });
  await fs.writeFile(resolved, `${JSON.stringify(value, null, 2)}\n`, { encoding: 'utf8', mode });
  await fs.chmod(resolved, mode);
}

function clientsFromEnvironment(env = process.env) {
  const apiKey = env.LINEAR_API_KEY || env.LINEAR_API_TOKEN;
  const teamId = env.LINEAR_TEAM_ID || DEFAULT_TEAM_ID;
  const parentIssue = env.LINEAR_PARENT_ISSUE || DEFAULT_PARENT_ISSUE;
  const allowedOwners = uniqueSorted(
    (env.GITHUB_ALLOWED_OWNERS || DEFAULT_ALLOWED_GITHUB_OWNERS.join(','))
      .split(',')
      .map((item) => item.trim()),
  );
  return {
    linear: createLinearClient({ apiKey, teamId, parentIssue }),
    github: createGitHubClient({ token: env.GITHUB_TOKEN, allowedOwners }),
    maxCreates: integerFromEnv(env.REAPER_MAX_LINEAR_CREATES, DEFAULT_MAX_CREATES, 'REAPER_MAX_LINEAR_CREATES', 0, 250),
  };
}

export async function main(argv = process.argv.slice(2), env = process.env) {
  const options = parseArgs([...argv]);
  if (options.help || !options.command) {
    process.stdout.write(usage());
    return;
  }
  const clients = clientsFromEnvironment(env);
  if (options.command === 'export-index') {
    if (!options.json) throw new Error('--json is required for export-index');
    const index = await clients.linear.exportIndex();
    await writeJson(options.json, index);
    process.stderr.write(`${JSON.stringify({ issues: index.issues.length })}\n`);
    return;
  }
  if (options.command === 'materialize') {
    if (!options.plan || !options.evidence || !options.summary) {
      throw new Error('--plan, --evidence, and --summary are required for materialize');
    }
    const plan = await readJson(options.plan);
    const result = await materializePlan(plan, clients, { maxCreates: clients.maxCreates });
    await writeJson(options.evidence, result.evidence);
    await writeJson(options.summary, result.summary, 0o644);
    process.stderr.write(`${JSON.stringify(result.summary.counts)}\n`);
    return;
  }
  throw new Error(`Unknown command: ${options.command}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exitCode = 1;
  });
}
