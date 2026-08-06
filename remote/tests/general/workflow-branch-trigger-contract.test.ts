import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

// DEN-1321 bullets 1 and 3, as one machine-readable contract.
//
// Two failure modes look identical in a workflow file but need opposite remedies:
//
//   (1) DRIFT — a workflow on `main` still triggers on the feature branch that
//       produced it. Observed for real: before the 2026-07-31 main/dev
//       reconciliation, nats-http-ingress.yml on `main` triggered on
//       `agent/den-440-nats-http-ingress-clean` while dev's copy correctly used
//       [dev, main]. The main copy was an earlier draft. It arrived through
//       BRANCH DIVERGENCE, not through rotting in place — so the ratchet has to
//       run on the merge path, which is what this test does.
//
//   (2) EXECUTION CARRIER — a workflow that legitimately drives a long-running
//       agent branch. Fine to exist, but DEN-1321 notes they accumulate with no
//       lifecycle, so they must declare an owner and an expiry.
//
// A single "branches must be main/dev" check would mis-flag every (2) as (1).
// Carriers therefore opt in with a marker comment anywhere in the workflow:
//
//   # dd:execution-carrier owner=DEN-445 expires=2026-10-01
//
// and this fails when the marker is missing OR the expiry has passed, giving
// carriers a bounded life instead of an indefinite one.
//
// Parsing is line-based on purpose: remote/tests installs with
// --frozen-lockfile and has no YAML parser dependency. Adding one for a
// structural check this simple would be a heavier change than the check itself.

const ALLOWED_BRANCHES = new Set(['main', 'dev', 'master']);
const MARKER = /#\s*dd:execution-carrier\s+owner=(\S+)\s+expires=(\d{4}-\d{2}-\d{2})/;
const TRIGGER_EVENTS = new Set(['push', 'pull_request', 'pull_request_target']);

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/apps/dd-next-runtime.application.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const WORKFLOW_DIR = resolve(repoRoot, '.github/workflows');

interface Offender {
  file: string;
  event: string;
  branch: string;
}

function indentOf(line: string): number {
  return line.length - line.trimStart().length;
}

/**
 * Collect literal (non-glob) branch pins under push/pull_request triggers.
 *
 * Walks the `on:` mapping tracking indentation, so a `branches:` list belonging
 * to a job step or a `paths:` sibling is never mistaken for a trigger filter.
 */
function scanWorkflow(text: string): Array<{ event: string; branch: string }> {
  const lines = text.split('\n');
  const found: Array<{ event: string; branch: string }> = [];

  let onIndent: number | null = null;
  let event: string | null = null;
  let eventIndent = 0;
  let inBranches = false;
  let branchesIndent = 0;

  for (const line of lines) {
    if (!line.trim() || line.trim().startsWith('#')) continue;
    const indent = indentOf(line);
    const trimmed = line.trim();

    // Enter the `on:` mapping (top level only).
    if (onIndent === null) {
      if (/^on:\s*$/.test(trimmed) && indent === 0) onIndent = indent;
      continue;
    }
    // Any other top-level key ends the `on:` mapping.
    if (indent <= onIndent && !/^on:/.test(trimmed)) break;

    const eventMatch = trimmed.match(/^([a-z_]+):\s*$/);
    if (eventMatch && (event === null || indent <= eventIndent)) {
      event = TRIGGER_EVENTS.has(eventMatch[1]) ? eventMatch[1] : null;
      eventIndent = indent;
      inBranches = false;
      continue;
    }
    if (!event) continue;

    if (/^branches:\s*$/.test(trimmed) && indent > eventIndent) {
      inBranches = true;
      branchesIndent = indent;
      continue;
    }
    // Inline form: branches: [dev, main]
    const inline = trimmed.match(/^branches:\s*\[(.+)\]\s*$/);
    if (inline && indent > eventIndent) {
      for (const raw of inline[1].split(',')) {
        const b = raw.trim().replace(/^['"]|['"]$/g, '');
        if (b) found.push({ event, branch: b });
      }
      inBranches = false;
      continue;
    }
    if (inBranches) {
      if (indent <= branchesIndent && !trimmed.startsWith('- ')) {
        inBranches = false;
        continue;
      }
      const item = trimmed.match(/^-\s*(.+)$/);
      if (item) {
        const b = item[1].trim().replace(/^['"]|['"]$/g, '');
        if (b) found.push({ event, branch: b });
      }
    }
  }
  return found;
}

function audit(): { offenders: Offender[]; carriers: Map<string, string> } {
  const offenders: Offender[] = [];
  const carriers = new Map<string, string>();

  if (!existsSync(WORKFLOW_DIR)) return { offenders, carriers };

  for (const name of readdirSync(WORKFLOW_DIR).filter((f) => /\.ya?ml$/.test(f))) {
    const text = readFileSync(resolve(WORKFLOW_DIR, name), 'utf8');
    const marker = text.match(MARKER);
    if (marker) carriers.set(name, marker[2]);

    for (const { event, branch } of scanWorkflow(text)) {
      // Globs are patterns, not pins — they cannot go stale the same way.
      if (branch.includes('*') || ALLOWED_BRANCHES.has(branch)) continue;
      offenders.push({ file: name, event, branch });
    }
  }
  return { offenders, carriers };
}

test('workflow branch triggers are main/dev, or a declared execution carrier', () => {
  const { offenders, carriers } = audit();
  const undeclared = offenders.filter((o) => !carriers.has(o.file));

  assert.deepEqual(
    undeclared,
    [],
    'These workflows pin a literal feature branch without declaring themselves execution carriers. ' +
      'Either retarget them to main/dev, or add a marker comment:\n' +
      '  # dd:execution-carrier owner=DEN-<id> expires=YYYY-MM-DD\n' +
      `Offenders: ${JSON.stringify(undeclared)}`,
  );
});

test('declared execution carriers have not expired', () => {
  const { offenders, carriers } = audit();

  // Compare as plain YYYY-MM-DD strings: lexicographic order is chronological
  // for ISO dates, which sidesteps timezone ambiguity entirely.
  const today = new Date().toISOString().slice(0, 10);

  const expired = offenders
    .filter((o) => carriers.has(o.file))
    .map((o) => ({ ...o, expires: carriers.get(o.file) as string }))
    .filter((o) => o.expires < today);

  assert.deepEqual(
    expired,
    [],
    'These execution carriers are past their declared expiry. Retire the workflow and its branch, ' +
      'or extend the expiry with a reason recorded in the owning issue.\n' +
      `Expired: ${JSON.stringify(expired)}`,
  );
});
