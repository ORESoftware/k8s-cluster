// Regression net for the security hardening audited on 2026-07-29.
//
// Each assertion here corresponds to a defect that was actually found in this
// repo, not to a generic best practice. They exist because every one of these
// is a silent regression: nothing fails at deploy time if a `uses:` drifts back
// to a mutable tag, if a new gateway route forgets its auth gate, or if a
// password is pasted back into a manifest as a literal.
//
// Deliberately text-based, like every other test in this directory —
// remote/tests has no YAML dependency and CI installs --frozen-lockfile.

import assert from 'node:assert/strict';
import { existsSync, readdirSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function listWorkflows(): string[] {
  const dir = resolve(repoRoot, '.github/workflows');
  return readdirSync(dir)
    .filter((f) => f.endsWith('.yml') || f.endsWith('.yaml'))
    .sort();
}

/** Walk every manifest under remote/, yielding [relativePath, contents]. */
function listManifests(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(resolve(repoRoot, dir), { withFileTypes: true })) {
      const rel = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (entry.name === 'node_modules' || entry.name === '.git') continue;
        walk(rel);
      } else if (entry.name.endsWith('.yaml') || entry.name.endsWith('.yml')) {
        out.push(rel);
      }
    }
  };
  walk('remote');
  return out.sort();
}

test('every third-party GitHub Action is pinned to a commit SHA', async () => {
  // A tag is repointable by the action owner, or by anyone who takes over that
  // account. These workflows carry AWS OIDC credentials, ECR push rights and
  // GHCR packages:write, so a moved tag is a direct path into the cluster's
  // registries. Only a 40-hex commit is immutable.
  const offenders: string[] = [];

  for (const file of listWorkflows()) {
    const contents = await readRepoFile(`.github/workflows/${file}`);
    contents.split('\n').forEach((line, index) => {
      const match = /^\s*-?\s*uses:\s*(\S+)/.exec(line);
      if (!match) return;
      const ref = match[1].replace(/^['"]|['"]$/g, '');

      // Local composite actions (./.github/actions/...) have no ref to pin.
      if (ref.startsWith('./') || ref.startsWith('../')) return;
      // Container actions pin by image digest instead of a git SHA.
      if (ref.startsWith('docker://')) {
        if (!/@sha256:[0-9a-f]{64}$/.test(ref)) {
          offenders.push(`${file}:${index + 1} ${ref} (docker ref not digest-pinned)`);
        }
        return;
      }

      if (!/@[0-9a-f]{40}$/.test(ref)) {
        offenders.push(`${file}:${index + 1} ${ref}`);
      }
    });
  }

  assert.deepEqual(
    offenders,
    [],
    `These action refs are on mutable tags; pin them to the commit the tag ` +
      `points at and keep the version as a trailing comment:\n  ${offenders.join('\n  ')}`,
  );
});

test('dtolnay/rust-toolchain pins pass an explicit toolchain input', async () => {
  // This action reads the toolchain from its own ref name (@stable, @1.75.0).
  // Pinning it to a commit — which the rule above requires — makes that name a
  // hex string, so the toolchain has to be named explicitly or the setup step
  // resolves a toolchain called "4cda84d5...".
  for (const file of listWorkflows()) {
    const contents = await readRepoFile(`.github/workflows/${file}`);
    const lines = contents.split('\n');

    lines.forEach((line, index) => {
      if (!/uses:\s*dtolnay\/rust-toolchain@/.test(line)) return;
      const following = lines.slice(index + 1, index + 6).join('\n');
      assert.match(
        following,
        /^\s*with:\s*$[\s\S]*?toolchain:\s*\S+/m,
        `${file}:${index + 1} pins dtolnay/rust-toolchain to a SHA but does not ` +
          `pass a "toolchain:" input, so the toolchain resolves to the commit id`,
      );
    });
  }
});

test('gateway routes into the dev-server control plane carry the operator auth gate', async () => {
  // dd-dev-server-api is the agent control plane (list/create tasks, threads).
  // /tasks, /status and /agents shipped without the gate that every sibling
  // route has. Only two routes may stay open, both for a stated reason.
  const OPEN_BY_DESIGN = new Set([
    '/healthz', // kubelet + external liveness probing; body is non-sensitive
    '/stream/', // browser-direct SSE authenticated by a per-task HMAC ?token=
  ]);

  const contents = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const lines = contents.split('\n');
  const ungated: string[] = [];

  for (let i = 0; i < lines.length; i += 1) {
    const open = /^\s*location\s+(.+?)\s*\{\s*$/.exec(lines[i]);
    if (!open) continue;

    let depth = 1;
    const body: string[] = [];
    let j = i + 1;
    while (j < lines.length && depth > 0) {
      depth += (lines[j].match(/\{/g) ?? []).length;
      depth -= (lines[j].match(/\}/g) ?? []).length;
      if (depth > 0) body.push(lines[j]);
      j += 1;
    }

    const block = body.join('\n');
    if (!block.includes('dd-dev-server-api')) continue;

    const spec = open[1].trim();
    if (OPEN_BY_DESIGN.has(spec)) continue;
    if (!/\$dd_gateway_auth_ok\s*=\s*0/.test(block)) {
      ungated.push(`line ${i + 1}: location ${spec}`);
    }
  }

  assert.deepEqual(
    ungated,
    [],
    `These gateway locations proxy to dd-dev-server-api without the ` +
      `"if ($dd_gateway_auth_ok = 0) { return 401; }" gate:\n  ${ungated.join('\n  ')}`,
  );
});

test('no manifest carries a credential as an inline env value', async () => {
  // gcs-rabbitmq shipped RABBITMQ_DEFAULT_PASS and RABBITMQ_ERLANG_COOKIE as
  // literals, and the same login was duplicated into the observability
  // exporter. Credentials belong in an ExternalSecret + secretKeyRef.
  //
  // Config knobs legitimately share these words (COOKIE_SECURE, *_TTL_SECONDS,
  // *_COOKIE_NAME), so a value is only accepted when it is plainly not a
  // secret: empty, boolean, numeric, or an explicitly-named identifier.
  const CREDENTIAL_NAME =
    /(PASSWORD|_PASS$|_PASS_|^PASS$|SECRET|TOKEN|COOKIE|APIKEY|API_KEY|CREDENTIAL|PRIVATE_KEY)/;
  const NOT_A_SECRET = /^(|true|false|\d+(\.\d+)?)$/i;
  const RUNTIME_SECRET_PATH = /^\/var\/run\/[A-Za-z0-9._/-]+$/;
  const offenders: string[] = [];

  for (const file of listManifests()) {
    const contents = await readRepoFile(file);
    const lines = contents.split('\n');

    lines.forEach((line, index) => {
      const nameMatch = /^\s*-\s*name:\s*([A-Z][A-Z0-9_]*)\s*$/.exec(line);
      if (!nameMatch) return;
      const envName = nameMatch[1];
      if (!CREDENTIAL_NAME.test(envName)) return;
      // A name-valued knob is a label, not a credential.
      if (envName.endsWith('_NAME')) return;

      const next = lines[index + 1];
      if (next === undefined) return;
      const valueMatch = /^\s*value:\s*(.*)$/.exec(next);
      if (!valueMatch) return; // valueFrom/secretKeyRef — the correct shape.

      const value = valueMatch[1].trim().replace(/^['"]|['"]$/g, '');
      if (NOT_A_SECRET.test(value)) return;
      // A *_PATH variable may point to a projected Secret file. Keep this
      // exception deliberately narrow: an absolute runtime mount only, with no
      // parent traversal. The secret bytes themselves remain forbidden here.
      if (
        (envName.endsWith('_PATH') || envName.endsWith('_SECRET_ROOT')) &&
        RUNTIME_SECRET_PATH.test(value) &&
        !value.includes('..')
      ) {
        return;
      }

      offenders.push(`${file}:${index + 1} ${envName} = ${JSON.stringify(value)}`);
    });
  }

  assert.deepEqual(
    offenders,
    [],
    `These env vars look like credentials but are inline literals. Move them ` +
      `to an ExternalSecret and reference them with secretKeyRef:\n  ${offenders.join('\n  ')}`,
  );
});

test('the runtime-config snapshot route authenticates before returning entries', async () => {
  // The global preHandler in server.ts deliberately skips
  // /internal/runtime-config* so this module can enforce its own
  // RUNTIME_CONFIG_SERVER_SECRET. The GET handler previously did not, which
  // left the whole pushed-config map world-readable inside the cluster.
  const source = await readRepoFile('remote/deployments/dev-server/src/runtime-config.ts');

  // Bound the slice at the NEXT route registration. A lazy match to the first
  // "\n  });" runs straight past a one-line handler into the POST below it and
  // finds that route's requireServerAuth instead — which is exactly how the
  // original defect would sneak back in unnoticed.
  const start = source.indexOf("fastify.get('/internal/runtime-config'");
  assert.notEqual(start, -1, 'could not locate the GET /internal/runtime-config handler');
  const nextRoute = source.slice(start + 1).search(/\n\s*fastify\.(get|post|put|delete|all)\(/);
  const getHandler = nextRoute === -1 ? source.slice(start) : source.slice(start, start + 1 + nextRoute);

  assert.match(
    getHandler,
    /requireServerAuth/,
    'GET /internal/runtime-config must call requireServerAuth before returning the snapshot ' +
      `(handler body examined:\n${getHandler}\n)`,
  );
  assert.match(getHandler, /401/, 'the snapshot route must answer 401 when auth fails');

  // The preHandler exemption is what makes the check above load-bearing; if the
  // exemption ever goes away this test should be revisited, not deleted.
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  assert.match(server, /startsWith\('\/internal\/runtime-config'\)/);
});

test('dev-server compares shared secrets in constant time', async () => {
  // A byte-wise `===` on a long shared secret leaks its match prefix through
  // response timing. token.ts already used timingSafeEqual for the stream HMAC;
  // the header checks did not.
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');

  assert.match(server, /function secretEquals\(/, 'expected a constant-time secretEquals helper');
  assert.match(server, /timingSafeEqual/);
  assert.doesNotMatch(
    server,
    /headers\['x-server-auth'\]\s*!==/,
    'x-server-auth must be compared through headerMatches/secretEquals, not !==',
  );
});

test('the public browser MCP fails closed on authentication', async () => {
  // /browser-mcp is publicly routed and write-capable — browser_act drives a
  // real browser. Authentication is enforced by BROWSER_MCP_REQUIRE_AUTH, which
  // used to default to false in code and be switched on only by the production
  // manifest. Any deployment path that missed the variable served anonymous
  // write access, and validate_config never checked it (unlike
  // SERVER_AUTH_SECRET and the domain allowlist, which hard-error).
  const main = await readRepoFile('remote/deployments/browser-mcp-rs/src/main.rs');
  assert.match(
    main,
    /require_auth:\s*env_bool\("BROWSER_MCP_REQUIRE_AUTH",\s*true\)/,
    'BROWSER_MCP_REQUIRE_AUTH must default to true so an unconfigured process ' +
      'stays authenticated',
  );

  // Belt and braces: the manifest ArgoCD actually deploys must still say so.
  const deployment = await readRepoFile(
    'remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.deployment.yaml',
  );
  assert.match(
    deployment,
    /-\s*name:\s*BROWSER_MCP_REQUIRE_AUTH\s*\n\s*value:\s*'?"?true'?"?/,
    'the deployed browser-mcp manifest must set BROWSER_MCP_REQUIRE_AUTH=true',
  );

  const app = await readRepoFile('remote/argocd/apps/dd-browser-mcp-rs.application.yaml');
  assert.match(
    app,
    /path:\s*remote\/deployments\/browser-mcp-rs\/k8s\/ec2/,
    'if the Argo source path moves, the manifest asserted above is no longer ' +
      'the one being deployed — re-point this test at the new path',
  );
});

test('Erlang distribution cookies are never shipped as a usable value', async () => {
  // These files sit in a numbered, apply-in-order directory, so `kubectl apply
  // -f k8s/` used to install a cookie whose value is printed in this repo. The
  // cookie is RCE-equivalent to anyone who can reach EPMD/distribution.
  const cookieManifests = [
    'remote/deployments/gleamlang-presence-server/k8s/20-secret-cookie.yaml',
    'remote/deployments/gleamlang-ws-server/k8s/cluster/20-secret-cookie.yaml',
    'remote/deployments/gleamlang-ws-server/gleamlang-presence-server/k8s/20-secret-cookie.yaml',
  ];

  for (const file of cookieManifests) {
    const contents = await readRepoFile(file);
    const active = contents
      .split('\n')
      .filter((line) => !/^\s*#/.test(line) && line.trim() !== '');

    assert.deepEqual(
      active,
      [],
      `${file} must not declare an applyable Secret — the cookie is created ` +
        `out of band. Active (non-comment) lines found:\n  ${active.join('\n  ')}`,
    );
    assert.match(
      contents,
      /kubectl -n presence create secret generic presence-cookie/,
      `${file} should document the out-of-band creation command`,
    );
  }
});