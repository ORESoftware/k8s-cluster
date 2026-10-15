import assert from "node:assert/strict";
import { existsSync, readFileSync, realpathSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const canonical = path.join(root, "agents.md");
const pointers = new Map([
  ["AGENTS.md", "# Canonical agent instructions\n\nRead and apply [`agents.md`](agents.md). This compatibility file must not duplicate repository instructions.\n"],
  [".claude/CLAUDE.md", "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n"],
  [".gemini/GEMINI.md", "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n"],
  [".openai/AGENTS.md", "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n"],
]);

assert.ok(existsSync(canonical), "missing lowercase agents.md");
const text = readFileSync(canonical, "utf8");
const canonicalStat = statSync(canonical);
for (const phrase of [
  "github.com/sonus-auris",
  "Linear project: `github.com/sonus-auris`",
  "Resolve conflicts semantically",
  "git fetch --all --prune",
  "<<<<<<<",
  "privacy",
  "account-deletion",
  "npm run check",
]) {
  assert.ok(text.includes(phrase), `agents.md missing required phrase: ${phrase}`);
}
for (const [relative, expected] of pointers) {
  const file = path.join(root, relative);
  assert.ok(existsSync(file), `missing pointer ${relative}`);
  const fileText = readFileSync(file, "utf8");
  const fileStat = statSync(file);
  const aliasesCanonicalFile = relative === "AGENTS.md"
    && fileStat.dev === canonicalStat.dev
    && fileStat.ino === canonicalStat.ino;
  if (aliasesCanonicalFile) {
    assert.equal(fileText, text, `${relative} must expose canonical instructions on a case-insensitive filesystem`);
  } else {
    assert.equal(fileText, expected, `${relative} duplicates or diverges from canonical instructions`);
  }
}

function discover(start) {
  let directory = realpathSync(start);
  const ancestors = [];
  while (true) {
    ancestors.push(directory);
    const parent = path.dirname(directory);
    if (parent === directory) break;
    directory = parent;
  }
  ancestors.reverse();
  const seen = new Set();
  const found = [];
  for (const ancestor of ancestors) {
    const candidate = path.join(ancestor, "agents.md");
    if (!existsSync(candidate)) continue;
    const resolved = realpathSync(candidate);
    readFileSync(candidate, "utf8");
    if (seen.has(resolved)) continue;
    seen.add(resolved);
    found.push(resolved);
  }
  return found;
}

const chain = discover(path.join(root, "src"));
assert.deepEqual(chain, [realpathSync(canonical)], `wrong root-to-leaf agents.md chain: ${chain.join(", ")}`);
console.log("agents.md chain for src/:");
for (const file of chain) console.log(`  - ${path.relative(root, file)}`);
