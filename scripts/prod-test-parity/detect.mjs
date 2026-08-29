#!/usr/bin/env node
/**
 * Read-only production/test GitHub org parity detector (DEN-3444).
 *
 * Live discovery uses `gh api` and never creates or mutates repositories.
 * Unit tests inject fixture responses and never touch the network.
 */

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = dirname(fileURLToPath(import.meta.url));
export const EXPECTED_PATH = join(ROOT, "expected-pairs.json");
export const SCHEMA_PATH = join(ROOT, "schema.json");

const ROLE_PATTERNS = [
  [/interfaces$/i, "interfaces"],
  [/(?:-lib-core|-libs|-lib)$/i, "lib"],
  [/-clients$/i, "clients"],
  [/-cli$/i, "cli"],
  [/(?:-web-server(?:\.rs)?|-web)$/i, "web"],
  [/(?:-api-server(?:\.rs)?|-api)$/i, "api"],
  [/-desktop/i, "desktop"],
  [/-e2e$/i, "e2e"],
  [/-infra$/i, "infra"],
  [/-monorepo$/i, "monorepo"],
];

const SECRET_SHAPE = /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|lin_api_[A-Za-z0-9]+|Bearer\s+[A-Za-z0-9._-]{20,}/i;

export function inferRole(name) {
  for (const [pattern, role] of ROLE_PATTERNS) {
    if (pattern.test(name)) return role;
  }
  return "other";
}

export function alignedObservation(expected, observedAt = "2026-08-26T00:00:00Z") {
  const orgs = {};
  for (const pair of expected.pairs) {
    const repos = Object.entries(pair.roles).map(([name]) => ({
      archived: false,
      copiesLiveCredentials: false,
      defaultBranch: pair.defaultBranch,
      hasZpkg: true,
      language: "Rust",
      name,
      visibility: pair.visibility,
    }));
    orgs[pair.productionOrg] = { repos, status: "ok" };
    orgs[pair.testOrg] = { repos: structuredClone(repos), status: "ok" };
  }
  return { observedAt, orgs };
}

export function loadExpected(path = EXPECTED_PATH) {
  const expected = JSON.parse(readFileSync(path, "utf8"));
  if (expected.schemaVersion !== 1) {
    throw new Error(`unsupported schemaVersion ${expected.schemaVersion}`);
  }
  if (!Array.isArray(expected.pairs) || expected.pairs.length < 10) {
    throw new Error("expected snapshot must declare at least ten org pairs");
  }
  return expected;
}

export function stableStringify(value) {
  return `${JSON.stringify(sortValue(value), null, 2)}\n`;
}

function sortValue(value) {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortValue(value[key])]),
    );
  }
  return value;
}

function exceptionKey(exception) {
  return [
    exception.kind,
    exception.productionOrg ?? "",
    exception.testOrg ?? "",
    exception.repository ?? "",
  ].join("|");
}

function coveredByException(exceptions, candidate) {
  const key = exceptionKey(candidate);
  return exceptions.some((exception) => exceptionKey(exception) === key);
}

export function detect({ expected, observed, observedAt = "1970-01-01T00:00:00Z" }) {
  const drift = [];
  const unknown = [];
  const exceptions = expected.exceptions ?? [];
  const excluded = new Set();
  for (const exception of exceptions) {
    if (exception.kind === "excluded-organization") {
      if (exception.productionOrg) excluded.add(exception.productionOrg.toLowerCase());
      if (exception.testOrg) excluded.add(exception.testOrg.toLowerCase());
    }
  }

  const observedOrgs = observed.orgs ?? {};
  const declaredTest = new Set();

  for (const pair of expected.pairs) {
    if (excluded.has(pair.productionOrg.toLowerCase()) || excluded.has(pair.testOrg.toLowerCase())) {
      continue;
    }
    declaredTest.add(pair.testOrg.toLowerCase());
    const production = observedOrgs[pair.productionOrg];
    const test = observedOrgs[pair.testOrg];
    recordAccess(unknown, pair.productionOrg, production);
    recordAccess(unknown, pair.testOrg, test);

    if (!production || production.status === "unknown" || !test || test.status === "unknown") {
      continue;
    }
    if (test.status === "missing") {
      const candidate = {
        kind: "missing-test-org",
        productionOrg: pair.productionOrg,
        testOrg: pair.testOrg,
      };
      if (pair.canary === "required" && !coveredByException(exceptions, candidate)) {
        drift.push({
          class: "production-without-test-strategy",
          productionOrg: pair.productionOrg,
          testOrg: pair.testOrg,
        });
      }
      continue;
    }

    const productionRepos = indexRepos(production.repos ?? []);
    const testRepos = indexRepos(test.repos ?? []);

    for (const [name, repo] of Object.entries(productionRepos)) {
      const expectedRole = pair.roles?.[name] ?? inferRole(name);
      const counterpart = testRepos[name] ?? testRepos[`${name}-e2e`] ?? testRepos[`${name}-test`];
      if (!counterpart && pair.canary === "required" && expectedRole !== "other") {
        drift.push({
          class: "production-without-test-strategy",
          productionOrg: pair.productionOrg,
          testOrg: pair.testOrg,
          repository: name,
          role: expectedRole,
        });
      }
      if (counterpart) {
        compareRepoSettings(drift, pair, name, repo, counterpart, expectedRole);
      }
      if (!repo.hasZpkg && ["interfaces", "lib", "clients", "cli"].includes(expectedRole)) {
        drift.push({
          class: "missing-zpkg-toml",
          productionOrg: pair.productionOrg,
          repository: name,
          role: expectedRole,
        });
      }
    }

    for (const [name, repo] of Object.entries(testRepos)) {
      const productionOwner = productionRepos[name] ?? productionRepos[name.replace(/-e2e$|-test$/, "")];
      if (!productionOwner) {
        const candidate = {
          kind: "orphan-test-repository",
          testOrg: pair.testOrg,
          repository: name,
        };
        if (!coveredByException(exceptions, candidate)) {
          drift.push({
            class: "orphan-test-repository",
            productionOrg: pair.productionOrg,
            testOrg: pair.testOrg,
            repository: name,
            role: inferRole(name),
          });
        }
      }
      if (repo.copiesLiveCredentials) {
        drift.push({
          class: "live-credential-copy",
          testOrg: pair.testOrg,
          repository: name,
        });
      }
    }
  }

  for (const [login, org] of Object.entries(observedOrgs)) {
    if (!login.toLowerCase().endsWith("-test") && !login.toLowerCase().endsWith("-tesr")) continue;
    if (declaredTest.has(login.toLowerCase()) || excluded.has(login.toLowerCase())) continue;
    const candidate = { kind: "orphan-test-org", testOrg: login };
    if (!coveredByException(exceptions, candidate) && org.status === "ok") {
      drift.push({ class: "orphan-test-org", testOrg: login });
    }
  }

  drift.sort(compareDrift);
  unknown.sort((a, b) => a.org.localeCompare(b.org));
  const report = {
    classCounts: countClasses(drift),
    drift,
    observedAt,
    pairCount: expected.pairs.length,
    policyVersion: expected.policyVersion,
    schemaVersion: expected.schemaVersion,
    unknown,
  };
  const serialized = stableStringify(report);
  if (SECRET_SHAPE.test(serialized)) {
    throw new Error("parity report contained a credential-shaped value");
  }
  return report;
}

function recordAccess(unknown, org, observed) {
  if (!observed || observed.status === "unknown") {
    unknown.push({ org, status: observed?.status ?? "unknown" });
  }
}

function indexRepos(repos) {
  return Object.fromEntries(repos.map((repo) => [repo.name, repo]));
}

function compareRepoSettings(drift, pair, name, production, test, expectedRole) {
  if (production.defaultBranch && test.defaultBranch && production.defaultBranch !== test.defaultBranch) {
    drift.push({
      class: "mismatch-default-branch",
      productionOrg: pair.productionOrg,
      testOrg: pair.testOrg,
      repository: name,
      productionValue: production.defaultBranch,
      testValue: test.defaultBranch,
    });
  }
  if (production.visibility && test.visibility && production.visibility !== test.visibility) {
    drift.push({
      class: "mismatch-visibility",
      productionOrg: pair.productionOrg,
      testOrg: pair.testOrg,
      repository: name,
      productionValue: production.visibility,
      testValue: test.visibility,
    });
  }
  if (Boolean(production.archived) !== Boolean(test.archived)) {
    drift.push({
      class: "mismatch-archived",
      productionOrg: pair.productionOrg,
      testOrg: pair.testOrg,
      repository: name,
      productionValue: Boolean(production.archived),
      testValue: Boolean(test.archived),
    });
  }
  const productionRole = pair.roles?.[name] ?? inferRole(name);
  const testRole = inferRole(test.name);
  if (productionRole !== "other" && testRole !== "other" && productionRole !== testRole && testRole !== expectedRole) {
    drift.push({
      class: "mismatch-topology-role",
      productionOrg: pair.productionOrg,
      testOrg: pair.testOrg,
      repository: name,
      productionValue: productionRole,
      testValue: testRole,
    });
  }
}

function compareDrift(left, right) {
  return [left.class, left.productionOrg ?? "", left.testOrg ?? "", left.repository ?? ""]
    .join("|")
    .localeCompare([right.class, right.productionOrg ?? "", right.testOrg ?? "", right.repository ?? ""].join("|"));
}

function countClasses(drift) {
  const counts = {};
  for (const item of drift) {
    counts[item.class] = (counts[item.class] ?? 0) + 1;
  }
  return counts;
}

export function formatMarkdown(report) {
  const lines = [
    `# Production/test parity drift`,
    ``,
    `policy: \`${report.policyVersion}\` · pairs: ${report.pairCount} · observed: ${report.observedAt}`,
    ``,
  ];
  if (report.unknown.length) {
    lines.push(`## Unknown access`);
    for (const item of report.unknown) lines.push(`- \`${item.org}\` (${item.status})`);
    lines.push("");
  }
  if (!report.drift.length) {
    lines.push("No drift.");
    return `${lines.join("\n")}\n`;
  }
  lines.push("## Drift");
  for (const item of report.drift) {
    const target = [item.productionOrg, item.testOrg, item.repository].filter(Boolean).join("/");
    lines.push(`- **${item.class}** \`${target}\``);
  }
  return `${lines.join("\n")}\n`;
}

export function ghApiClient({ runner } = {}) {
  const run = runner ?? ((args) => execFileSync("gh", ["api", ...args], { encoding: "utf8" }));
  return async function ghApi(path) {
    try {
      const stdout = run([path, "--paginate"]);
      return { status: 200, body: JSON.parse(stdout || "[]") };
    } catch (error) {
      const message = String(error.stderr ?? error.message ?? error);
      if (/\b404\b/.test(message)) return { status: 404, body: null };
      if (/\b403\b/.test(message) || /\b401\b/.test(message)) return { status: 403, body: null };
      throw error;
    }
  };
}

export async function observeFromGitHub({ expected, ghApi, logins }) {
  const orgs = {};
  const unique = [...new Set(logins ?? expected.pairs.flatMap((pair) => [pair.productionOrg, pair.testOrg]))];
  for (const login of unique) {
    const org = await ghApi(`/orgs/${encodeURIComponent(login)}`);
    if (org.status === 403 || org.status === 401) {
      orgs[login] = { status: "unknown" };
      continue;
    }
    if (org.status === 404) {
      orgs[login] = { status: "missing" };
      continue;
    }
    const repos = await ghApi(`/orgs/${encodeURIComponent(login)}/repos?per_page=100`);
    if (repos.status !== 200) {
      orgs[login] = { status: "unknown" };
      continue;
    }
    const items = Array.isArray(repos.body) ? repos.body : [];
    orgs[login] = {
      status: "ok",
      repos: items.map((repo) => ({
        archived: Boolean(repo.archived),
        defaultBranch: repo.default_branch ?? "main",
        language: repo.language ?? "",
        name: repo.name,
        visibility: repo.visibility ?? (repo.private ? "private" : "public"),
      })),
    };
  }
  return { orgs };
}

function parseArgs(argv) {
  const args = { format: "json", fixture: null, live: false, write: null };
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === "--markdown") args.format = "markdown";
    else if (token === "--json") args.format = "json";
    else if (token === "--live") args.live = true;
    else if (token === "--fixture") args.fixture = argv[++index];
    else if (token === "--write") args.write = argv[++index];
    else if (token === "--help" || token === "-h") args.help = true;
  }
  return args;
}

async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.help) {
    process.stderr.write(
      "Usage: node detect.mjs [--fixture <file>] [--live] [--json|--markdown] [--write <file>]\n",
    );
    process.exitCode = 0;
    return;
  }
  const expected = loadExpected();
  const observed = args.fixture
    ? JSON.parse(readFileSync(args.fixture, "utf8"))
    : args.live
      ? await observeFromGitHub({ expected, ghApi: ghApiClient() })
      : JSON.parse(readFileSync(join(ROOT, "fixtures/aligned.json"), "utf8"));
  const report = detect({
    expected,
    observed,
    observedAt: observed.observedAt ?? "1970-01-01T00:00:00Z",
  });
  const output = args.format === "markdown" ? formatMarkdown(report) : stableStringify(report);
  if (args.write) writeFileSync(args.write, output);
  process.stdout.write(output);
  process.exitCode = report.drift.length ? 1 : 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  await main();
}

export { main };
