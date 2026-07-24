import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { basename, join, relative, resolve } from 'node:path';
import test from 'node:test';

type DependencyHit = {
  file: string;
  line: number;
  source: string;
};

const IGNORED_SOURCE_DIRECTORIES = new Set([
  '.git',
  'build',
  'dist',
  'node_modules',
  'target',
  'vendor',
]);

// Direct SQLx is being migrated in the source-of-truth upstream repositories.
// Keep this list exact: a new path fails immediately, and a migrated path fails
// as stale until its exception is removed here.
const KNOWN_DIRECT_SQLX_DEBT = new Map<string, string>([
  [
    'remote/deployments/ai-agent-bridge/Cargo.toml',
    'Replace the optional Postgres SQLx feature with SeaORM upstream.',
  ],
  [
    'remote/deployments/contract-service-rs/Cargo.toml',
    'Move advisory-lock coordination from direct SQLx to SeaORM.',
  ],
  [
    'remote/deployments/daedalus-monorepo/apps/daedalus-sync/rust/Cargo.toml',
    'Migrate the Daedalus sync durability adapter from direct SQLx to SeaORM upstream.',
  ],
  [
    'remote/deployments/fiducia-monorepo/apps/fiducia-interfaces/generated/rust-db/Cargo.toml',
    'Replace generated SQLx row bindings with the canonical SeaORM bindings upstream.',
  ],
  [
    'remote/deployments/mip-solver-node.rs/Cargo.toml',
    'Migrate the MIP solver persistence path to SeaORM upstream.',
  ],
  [
    'remote/deployments/mip-solver-node.rs/local/Cargo.toml',
    'Migrate the MIP solver local workspace persistence path to SeaORM upstream.',
  ],
  [
    'remote/deployments/scintilla-run-monorepo/apps/scintilla-sync/Cargo.toml',
    'Migrate the Scintilla sync durability adapter from direct SQLx to SeaORM upstream.',
  ],
]);
const KNOWN_DIRECT_SQLX_HIT_COUNTS = new Map(
  [...KNOWN_DIRECT_SQLX_DEBT.keys()].map((file) => [file, 1]),
);

function findRepoRoot(): string {
  for (const candidate of [
    process.cwd(),
    resolve(process.cwd(), '..'),
    resolve(process.cwd(), '..', '..'),
  ]) {
    if (existsSync(resolve(candidate, '.gitmodules'))
      && existsSync(resolve(candidate, 'remote/deployments'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate k8s-cluster root from ${process.cwd()}`);
}

function runGit(repoRoot: string, args: string[]): string {
  return execFileSync('git', args, { cwd: repoRoot, encoding: 'utf8' }).trim();
}

function deploymentSubmodulePaths(repoRoot: string): string[] {
  return [...readFileSync(resolve(repoRoot, '.gitmodules'), 'utf8').matchAll(/^\s*path\s*=\s*(remote\/deployments\/\S+)\s*$/gm)]
    .map((match) => match[1])
    .sort();
}

function indexedGitlinks(repoRoot: string): Map<string, string> {
  const entries = runGit(repoRoot, ['ls-files', '--stage', 'remote/deployments'])
    .split('\n')
    .filter(Boolean);
  const gitlinks = new Map<string, string>();

  for (const entry of entries) {
    const match = entry.match(/^160000 ([0-9a-f]{40}) 0\t(.+)$/);
    if (match) gitlinks.set(match[2], match[1]);
  }

  return gitlinks;
}

function cargoManifests(directory: string): string[] {
  const manifests: string[] = [];

  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (IGNORED_SOURCE_DIRECTORIES.has(entry.name)) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      manifests.push(...cargoManifests(path));
    } else if (entry.isFile() && basename(path) === 'Cargo.toml') {
      manifests.push(path);
    }
  }

  return manifests;
}

function directSqlxDependencyLines(contents: string): Array<{ line: number; source: string }> {
  const hits: Array<{ line: number; source: string }> = [];
  let section = '';

  for (const [index, source] of contents.split(/\r?\n/).entries()) {
    const header = source.match(/^\s*\[([^\]]+)\]\s*$/);
    if (header) section = header[1];

    const dependencySection = /(?:^|\.)(?:dev-|build-)?dependencies(?:\.|$)/.test(section);
    if (!dependencySection) continue;

    const dependencyTable = section.match(
      /(?:^|\.)(?:dev-|build-)?dependencies\.(sqlx(?:-[A-Za-z0-9_-]+)?)$/,
    );
    if (header && dependencyTable) {
      hits.push({ line: index + 1, source: source.trim() });
      continue;
    }

    if (/^\s*sqlx(?:-[A-Za-z0-9_-]+)?\s*=/.test(source)
      || /^\s*package\s*=\s*['"]sqlx(?:-[A-Za-z0-9_-]+)?['"]/.test(source)
      || /^\s*[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bpackage\s*=\s*['"]sqlx(?:-[A-Za-z0-9_-]+)?['"]/.test(source)) {
      hits.push({ line: index + 1, source: source.trim() });
    }
  }

  return hits;
}

function directSqlxDependencies(repoRoot: string): DependencyHit[] {
  return cargoManifests(resolve(repoRoot, 'remote/deployments'))
    .flatMap((absolutePath) => directSqlxDependencyLines(readFileSync(absolutePath, 'utf8'))
      .map((hit) => ({
        file: relative(repoRoot, absolutePath),
        ...hit,
      })))
    .sort((left, right) => left.file.localeCompare(right.file) || left.line - right.line);
}

test('SeaORM driver features are allowed but direct SQLx dependencies are detected', () => {
  assert.deepEqual(
    directSqlxDependencyLines(`
[dependencies]
sea-orm = { version = "1", features = ["sqlx-postgres"] }
`),
    [],
  );
  assert.equal(directSqlxDependencyLines('[dependencies]\nsqlx = "0.8"').length, 1);
  assert.equal(directSqlxDependencyLines('[dependencies.database]\npackage = "sqlx"').length, 1);
  assert.equal(directSqlxDependencyLines('[dependencies]\ndatabase = { package = "sqlx", version = "0.8" }').length, 1);
  assert.equal(directSqlxDependencyLines('[target.\'cfg(unix)\'.dependencies.sqlx-postgres]\nversion = "0.8"').length, 1);
});

test('deployment repositories are initialized at their pinned gitlink commits', () => {
  const repoRoot = findRepoRoot();
  const declaredPaths = deploymentSubmodulePaths(repoRoot);
  const gitlinks = indexedGitlinks(repoRoot);

  assert.ok(declaredPaths.length >= 20, 'expected the backend deployment submodule fleet');
  assert.deepEqual(
    [...gitlinks.keys()].sort(),
    declaredPaths,
    'Every deployment submodule must be declared in .gitmodules and stored as a gitlink.',
  );

  const unavailable: string[] = [];
  const mismatched: string[] = [];
  for (const path of declaredPaths) {
    if (!existsSync(resolve(repoRoot, path, '.git'))) {
      unavailable.push(path);
      continue;
    }
    const checkout = runGit(repoRoot, ['-C', path, 'rev-parse', 'HEAD']);
    const pinned = gitlinks.get(path);
    if (checkout !== pinned) mismatched.push(`${path}: checkout=${checkout} pinned=${pinned}`);
  }

  assert.deepEqual(
    unavailable,
    [],
    'Initialize backend source with `git submodule update --init --depth 1 -- remote/deployments`.',
  );
  assert.deepEqual(mismatched, [], 'Backend submodule checkouts must match the superproject pins.');
});

test('Rust backends add no direct SQLx dependencies while migration debt burns down', () => {
  const repoRoot = findRepoRoot();
  const manifests = cargoManifests(resolve(repoRoot, 'remote/deployments'));
  const hits = directSqlxDependencies(repoRoot);
  const unexpected = hits.filter((hit) => !KNOWN_DIRECT_SQLX_DEBT.has(hit.file));

  assert.ok(manifests.length >= 100, 'expected initialized Rust backend/submodule manifests');
  assert.deepEqual(
    unexpected,
    [],
    `Direct SQLx is forbidden; use SeaORM instead:\n${unexpected
      .map((hit) => `  ${hit.file}:${hit.line} ${hit.source}`)
      .join('\n')}`,
  );

  const hitCounts = new Map<string, number>();
  for (const hit of hits) hitCounts.set(hit.file, (hitCounts.get(hit.file) ?? 0) + 1);
  const changedDebt = [...KNOWN_DIRECT_SQLX_DEBT.entries()]
    .filter(([file]) => hitCounts.get(file) !== KNOWN_DIRECT_SQLX_HIT_COUNTS.get(file));
  assert.deepEqual(
    changedDebt,
    [],
    `Update/remove changed direct-SQLx debt only as part of its SeaORM migration:\n${changedDebt
      .map(([file, reason]) => `  ${file}: expected ${KNOWN_DIRECT_SQLX_HIT_COUNTS.get(file)}, found ${hitCounts.get(file) ?? 0}; ${reason}`)
      .join('\n')}`,
  );
});
