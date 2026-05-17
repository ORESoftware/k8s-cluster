import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/libs/pg-defs/schema/schema.sql'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

// Single source of truth for "every Gleam project the dd_pg_defs path-dep
// is wired into". When you add a new Gleam service that talks to RDS, add
// it here so this drift test catches breakages before the deployment ships.
const GLEAM_PROJECTS: ReadonlyArray<{
  readonly name: string;
  readonly dir: string;
  // Whether the project must list `dd_pg_defs = { path = ... }` in its
  // top-level [dependencies]. The dd_pg_defs package itself is the source
  // and obviously doesn't depend on itself.
  readonly requiresPathDep: boolean;
}> = [
  {
    name: 'dd_pg_defs',
    dir: 'remote/libs/pg-defs/generated/gleam',
    requiresPathDep: false,
  },
  {
    name: 'gleam_lambda_runner',
    dir: 'remote/gleam-lambda-runner',
    requiresPathDep: true,
  },
  {
    name: 'gleam_mcp_server',
    dir: 'remote/gleam-mcp-server',
    requiresPathDep: true,
  },
  {
    name: 'gleamlang_server',
    dir: 'remote/gleamlang-server',
    requiresPathDep: true,
  },
];

function hasGleamToolchain(): boolean {
  const probe = spawnSync('gleam', ['--version'], { encoding: 'utf8' });
  return probe.status === 0;
}

function runGleam(args: ReadonlyArray<string>, cwd: string): {
  status: number | null;
  stdout: string;
  stderr: string;
} {
  const result = spawnSync('gleam', args as string[], {
    cwd,
    encoding: 'utf8',
    env: {
      ...process.env,
      // Some hex/build cache settings need a writable HOME; leave the
      // real env intact for CI runners and local devs alike.
    },
    timeout: 240_000,
  });
  return {
    status: result.status,
    stdout: result.stdout ?? '',
    stderr: result.stderr ?? '',
  };
}

test('every Gleam project lists dd_pg_defs as a path dependency where required', async () => {
  for (const project of GLEAM_PROJECTS) {
    if (!project.requiresPathDep) {
      continue;
    }
    const gleamToml = await readFile(
      resolve(repoRoot, project.dir, 'gleam.toml'),
      'utf8',
    );
    assert.match(
      gleamToml,
      /dd_pg_defs\s*=\s*\{\s*path\s*=\s*"\.\.\/libs\/pg-defs\/generated\/gleam"\s*\}/,
      `${project.name} (${project.dir}/gleam.toml) is missing the dd_pg_defs path dependency. Without it the service can't import pg_defs and the schema source-of-truth fragments.`,
    );
  }
});

test('every consumer exposes a pg_contract module that re-exports dd_pg_defs', async () => {
  for (const project of GLEAM_PROJECTS) {
    if (!project.requiresPathDep) {
      continue;
    }
    // Convention: every Gleam service that consumes dd_pg_defs ships a
    // `<service>/pg_contract.gleam` module so reads go through one local
    // import site (and so wiring is easy to grep for + test).
    const pgContractPath = resolve(
      repoRoot,
      project.dir,
      'src',
      project.name,
      'pg_contract.gleam',
    );
    assert.ok(
      existsSync(pgContractPath),
      `${project.name} is missing ${pgContractPath}. See remote/gleam-lambda-runner/src/gleam_lambda_runner/pg_contract.gleam for the reference pattern.`,
    );
    const source = await readFile(pgContractPath, 'utf8');
    assert.match(
      source,
      /^import pg_defs/m,
      `${project.name}/pg_contract.gleam must import pg_defs so the path-dep is exercised at compile time.`,
    );
  }
});

test('gleam test passes for every Gleam project', { timeout: 600_000 }, async (t) => {
  if (!hasGleamToolchain()) {
    t.skip(
      'gleam is not on PATH. Install via `brew install gleam` (macOS) or use the ' +
        '`ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine` image. CI installs Gleam ' +
        'before running this test, so this skip is local-only.',
    );
    return;
  }

  for (const project of GLEAM_PROJECTS) {
    const projectDir = resolve(repoRoot, project.dir);
    assert.ok(
      existsSync(resolve(projectDir, 'gleam.toml')),
      `${project.dir}/gleam.toml is missing`,
    );

    // `gleam test` builds + runs gleeunit. If any consumer can't compile
    // against dd_pg_defs (e.g. path-dep dropped, re-export renamed, schema
    // regenerated incompatibly) it fails here loudly.
    const result = runGleam(['test'], projectDir);
    const combinedOutput = `${result.stdout}\n${result.stderr}`;
    assert.equal(
      result.status,
      0,
      `${project.name}: \`gleam test\` failed (exit ${result.status}).\n` +
        `STDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
    );
    assert.match(
      combinedOutput,
      /no failures/,
      `${project.name}: \`gleam test\` did not print "no failures":\n${combinedOutput}`,
    );
    if (project.requiresPathDep) {
      // After a successful build, two artifacts must exist regardless of
      // build cache state:
      //   1. manifest.toml records dd_pg_defs as a local path-dep, and
      //   2. build/dev/erlang/dd_pg_defs/ holds the compiled BEAM output.
      // Either signal alone is sufficient to prove the path-dep is wired
      // and exercised; checking both gives us defense-in-depth against
      // half-broken manifests (path-dep declared but never compiled) and
      // stale build trees (compiled artifacts orphaned from a deleted dep).
      const manifestPath = resolve(projectDir, 'manifest.toml');
      assert.ok(
        existsSync(manifestPath),
        `${project.name}: manifest.toml is missing after \`gleam test\`.`,
      );
      const manifestText = await readFile(manifestPath, 'utf8');
      assert.match(
        manifestText,
        /name\s*=\s*"dd_pg_defs"[\s\S]*?source\s*=\s*"local"[\s\S]*?path\s*=\s*"\.\.\/libs\/pg-defs\/generated\/gleam"/,
        `${project.name}: manifest.toml does not record dd_pg_defs as a local path-dep. Wiring is broken; \`gleam deps download\` may have silently picked up a hex version instead.\n${manifestText}`,
      );
      const builtDepDir = resolve(projectDir, 'build', 'dev', 'erlang', 'dd_pg_defs');
      assert.ok(
        existsSync(builtDepDir),
        `${project.name}: ${builtDepDir} does not exist after \`gleam test\`. The path-dep was declared but never compiled into the consumer's build tree.`,
      );
    }
  }
});
