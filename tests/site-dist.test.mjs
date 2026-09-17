// Build-output ratchet: assert exactly what the site publishes.
//
// Everything in `public/` is copied verbatim into `dist/`, and every
// non-underscore-prefixed file in `src/pages/` becomes a live route. That has
// twice put internal developer docs on the public domain: `092ec55` removed a
// live `/README/` page and a `/README.md`, and `0479717` reintroduced both two
// days later. Nothing failed, because no test looked at the built output.
//
// This is that test. It runs after a build (npm run test:browser builds first),
// and it deliberately inspects EVERY file in dist/, not just `.html` — an
// HTML-only walk declares the site clean while `dist/README.md` is being served
// as text/markdown, which is exactly how the second regression hid.
import { readFile, readdir, stat } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";

const distDir = fileURLToPath(new URL("../dist", import.meta.url));

// Exact paths the site is allowed to publish, relative to dist/.
const ALLOWED_FILES = new Set([
  "404.html",
  "index.html",
  "investors/index.html",
  "use-cases/index.html",
  "CNAME",
  "company-facts.json",
  "favicon.svg",
]);

// Astro emits content-hashed stylesheets, so these match by shape, not name.
const ALLOWED_PATTERNS = [/^_astro\/[A-Za-z0-9_.-]+\.css$/];

async function walk(dir, prefix = "") {
  const found = [];
  for (const entry of await readdir(dir)) {
    const abs = path.join(dir, entry);
    const rel = prefix ? `${prefix}/${entry}` : entry;
    if ((await stat(abs)).isDirectory()) found.push(...(await walk(abs, rel)));
    else found.push(rel);
  }
  return found;
}

assert.ok(
  existsSync(distDir),
  "dist/ is missing — build first (npm run build), or run via npm run test:browser",
);
const files = (await walk(distDir)).sort();

test("dist publishes only the expected files", () => {
  const unexpected = files.filter(
    (f) => !ALLOWED_FILES.has(f) && !ALLOWED_PATTERNS.some((p) => p.test(f)),
  );
  assert.deepEqual(
    unexpected,
    [],
    `unexpected files in the published site: ${unexpected.join(", ")}. ` +
      "Anything in public/ is copied verbatim and any non-underscore file in " +
      "src/pages/ becomes a route. If this file is genuinely meant to be public, " +
      "add it to ALLOWED_FILES; otherwise move it out of public/ or prefix it " +
      "with an underscore.",
  );
});

test("no internal documentation is published", () => {
  // The specific regression this file exists to prevent, asserted by shape so
  // it also catches CHANGELOG.md, notes.md, or a stray /docs/ route.
  const docs = files.filter((f) => /\.mdx?$/i.test(f) || /(^|\/)readme(\/|$)/i.test(f));
  assert.deepEqual(
    docs,
    [],
    `internal docs are being served from the marketing domain: ${docs.join(", ")}`,
  );
});

test("every built page carries exactly one h1", async () => {
  // use-cases/ shipped with zero <h1> — the page's top-level heading was an
  // <h2> — which no source test could see and no browser test loaded.
  const pages = files.filter((f) => f.endsWith(".html"));
  assert.ok(pages.length >= 4, `expected the built pages, found ${pages.length}`);

  for (const page of pages) {
    const html = await readFile(path.join(distDir, page), "utf8");
    const count = (html.match(/<h1\b/gi) ?? []).length;
    assert.equal(count, 1, `${page}: expected exactly one <h1>, found ${count}`);
  }
});

test("no published file leaks a secret, private host, or local path", () => {
  const forbidden = [
    [/-----BEGIN [A-Z ]*PRIVATE KEY-----/, "private key"],
    [/\/Users\/[a-z0-9._-]+\//i, "local filesystem path"],
    [/\b10\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/, "private 10.x address"],
    [/\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/, "AWS access key id"],
  ];
  return Promise.all(
    files.map(async (file) => {
      // Text-bearing outputs only; the repo ships no binaries today.
      if (!/\.(html|css|js|json|svg|txt|md)$/i.test(file) && file !== "CNAME") return;
      const text = await readFile(path.join(distDir, file), "utf8");
      for (const [pattern, label] of forbidden) {
        assert.doesNotMatch(text, pattern, `${file}: published output contains a ${label}`);
      }
    }),
  );
});
