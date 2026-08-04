import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const expectedApps = [
  "gleam-lambda-runner",
  "scintilla-app-rs",
  "scintilla-backend.rs",
  "scintilla-clients",
  "scintilla-interfaces",
  "scintilla-mcp-server.rs",
  "scintilla-run-infra",
  "scintilla-sync",
  "scintilla-ui.dart",
].sort();

function parseGitmodules(source) {
  const entries = [];
  let current;
  for (const rawLine of source.split(/\r?\n/)) {
    const section = /^\[submodule "([^"]+)"\]$/.exec(rawLine.trim());
    if (section) {
      current = { name: section[1] };
      entries.push(current);
      continue;
    }
    const property = /^\s*(path|url|branch)\s*=\s*(.+?)\s*$/.exec(rawLine);
    if (property && current) current[property[1]] = property[2];
  }
  return entries;
}

test("deployment app inventory is exact and uniquely pinned", () => {
  const entries = parseGitmodules(read(".gitmodules"));
  assert.equal(entries.length, expectedApps.length + 1, "unexpected submodule count");

  const names = new Set();
  const paths = new Set();
  const urls = new Set();
  for (const entry of entries) {
    assert.ok(entry.name && entry.path && entry.url && entry.branch, "incomplete submodule entry");
    assert.equal(entry.name, entry.path, `${entry.name} section/path drifted`);
    assert.ok(!names.has(entry.name), `duplicate submodule section: ${entry.name}`);
    assert.ok(!paths.has(entry.path), `duplicate submodule path: ${entry.path}`);
    assert.ok(!urls.has(entry.url), `duplicate submodule URL: ${entry.url}`);
    names.add(entry.name);
    paths.add(entry.path);
    urls.add(entry.url);
    assert.equal(entry.branch, "main", `${entry.name} must track main`);
  }

  const appEntries = entries.filter(({ path }) => path.startsWith("apps/"));
  assert.deepEqual(
    appEntries.map(({ path }) => path.slice("apps/".length)).sort(),
    expectedApps,
  );
  for (const { path, url } of appEntries) {
    const repo = path.slice("apps/".length);
    assert.equal(url, `git@github.com:scintilla-run/${repo}.git`);
  }

  const tools = entries.filter(({ path }) => path.startsWith("tools/"));
  assert.deepEqual(tools, [
    {
      name: "tools/flags-2-env",
      path: "tools/flags-2-env",
      url: "https://github.com/ORESoftware/flags-2-env.git",
      branch: "main",
    },
  ]);
});

test("every declared submodule path is stored as a gitlink", () => {
  const entries = parseGitmodules(read(".gitmodules"));
  const stage = execFileSync("git", ["ls-files", "--stage"], { encoding: "utf8" });
  const gitlinks = new Map(
    stage
      .trim()
      .split(/\r?\n/)
      .map((line) => /^(\d+) ([0-9a-f]{40}) \d+\t(.+)$/.exec(line))
      .filter(Boolean)
      .map((match) => [match[3], { mode: match[1], sha: match[2] }]),
  );

  for (const { path } of entries) {
    const entry = gitlinks.get(path);
    assert.ok(entry, `${path} is absent from the git index`);
    assert.equal(entry.mode, "160000", `${path} is not a gitlink`);
    assert.match(entry.sha, /^[0-9a-f]{40}$/);
    assert.notEqual(entry.sha, "0".repeat(40), `${path} has a null commit`);
  }
});

test("README documents every deployable app and excludes non-workloads", () => {
  const readme = read("README.md");
  for (const repo of expectedApps) {
    assert.match(readme, new RegExp(`\\b${repo.replaceAll(".", "\\.")}\\b`));
  }
  assert.match(readme, /scintilla-run\.github\.io[^\n]+outside this monorepo/i);
  assert.doesNotMatch(read(".gitmodules"), /scintilla-run\.github\.io|scintilla-run-e2e/);
});
