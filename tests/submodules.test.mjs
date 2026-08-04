import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";

const EXPECTED_COMPONENTS = new Map([
  ["apps/fabrication-server.rs", "daedalus-fab/fabrication-server.rs"],
  ["apps/daedalus-api-server.rs", "daedalus-fab/daedalus-api-server.rs"],
  ["apps/daedalus-web-server.rs", "daedalus-fab/daedalus-web-server.rs"],
  ["apps/daedalus-infra", "daedalus-fab/daedalus-infra"],
  ["apps/daedalus-clients", "daedalus-fab/daedalus-clients"],
  ["apps/daedalus-interfaces", "daedalus-fab/daedalus-interfaces"],
  ["apps/daedalus-ui.dart", "daedalus-fab/daedalus-ui.dart"],
  ["apps/daedalus-fab-mcp-server.rs", "daedalus-fab/daedalus-fab-mcp-server.rs"],
  ["apps/daedalus-sync", "daedalus-fab/daedalus-sync"],
]);

function parseGitmodules(raw) {
  const entries = new Map();
  let current;

  for (const sourceLine of raw.split(/\r?\n/)) {
    const line = sourceLine.trim();
    if (!line || line.startsWith("#")) continue;

    const section = line.match(/^\[submodule "([^"]+)"\]$/);
    if (section) {
      current = { name: section[1] };
      assert.ok(!entries.has(current.name), `duplicate submodule section: ${current.name}`);
      entries.set(current.name, current);
      continue;
    }

    assert.ok(current, `property appears before a submodule section: ${line}`);
    const property = line.match(/^([A-Za-z][A-Za-z0-9_-]*)\s*=\s*(.+)$/);
    assert.ok(property, `invalid .gitmodules line: ${line}`);
    current[property[1]] = property[2].trim();
  }

  return entries;
}

function indexedGitlinks() {
  const output = execFileSync("git", ["ls-files", "--stage"], {
    encoding: "utf8",
  });
  const links = new Map();

  for (const line of output.trim().split(/\r?\n/)) {
    const match = line.match(/^160000 ([0-9a-f]{40}) \d+\t(.+)$/);
    if (match) links.set(match[2], match[1]);
  }
  return links;
}

test("the component manifest contains exactly the canonical Daedalus fleet", async () => {
  const raw = await readFile(new URL("../.gitmodules", import.meta.url), "utf8");
  const modules = parseGitmodules(raw);

  assert.deepEqual(
    [...modules.keys()].sort(),
    [...EXPECTED_COMPONENTS.keys()].sort(),
    "adding/removing a product component requires updating the explicit fleet contract",
  );

  const seenRepositories = new Set();
  for (const [path, repository] of EXPECTED_COMPONENTS) {
    const entry = modules.get(path);
    assert.ok(entry, `missing submodule for ${path}`);
    assert.equal(entry.name, path, `${path}: section name must equal checkout path`);
    assert.equal(entry.path, path, `${path}: configured path drifted`);
    assert.equal(entry.branch, "main", `${path}: release branch must remain main`);
    assert.equal(
      entry.url,
      `git@github.com:${repository}.git`,
      `${path}: upstream repository drifted`,
    );
    assert.ok(!seenRepositories.has(repository), `${repository} is pinned more than once`);
    seenRepositories.add(repository);
  }
});

test("every .gitmodules entry has one immutable gitlink in the index", async () => {
  const raw = await readFile(new URL("../.gitmodules", import.meta.url), "utf8");
  const modules = parseGitmodules(raw);
  const gitlinks = indexedGitlinks();

  assert.deepEqual(
    [...gitlinks.keys()].sort(),
    [...modules.keys()].sort(),
    ".gitmodules and the committed gitlinks must describe the same fleet",
  );

  for (const [path, sha] of gitlinks) {
    assert.match(sha, /^[0-9a-f]{40}$/, `${path}: invalid pinned commit`);
    assert.notEqual(sha, "0".repeat(40), `${path}: zero SHA is not a release pin`);
  }
});

test("the aggregator cannot recursively pin itself", async () => {
  const raw = await readFile(new URL("../.gitmodules", import.meta.url), "utf8");
  const modules = parseGitmodules(raw);
  const urls = [...modules.values()].map((entry) => entry.url);

  assert.ok(
    urls.every((url) => !url.endsWith("/daedalus-monorepo.git")),
    "the fleet aggregator must not contain a recursive self-submodule",
  );
});
