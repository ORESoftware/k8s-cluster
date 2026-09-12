import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

import {
  publicEnvForNamedStep,
  publicKeysFromDotenv,
  resolveBuildPath,
  resolveSameOriginBuildTarget,
} from "./deploy-contract.mjs";

const dist = path.resolve("dist");
assert.ok(existsSync(dist), "dist/ is missing; run the Astro build first");

const productionHost = "https://sonusauris.app";
const legacyHost = "https://sonus-auris.github.io";
const legacyBase = "/sonus-auris-site.web";

const cnamePath = path.join(dist, "CNAME");
assert.ok(existsSync(cnamePath), "dist/CNAME is missing from the Pages artifact");
assert.equal(
  readFileSync(cnamePath, "utf8").trim(),
  "sonusauris.app",
  "dist/CNAME must declare the production custom domain",
);

function walk(directory) {
  return readdirSync(directory).flatMap((entry) => {
    const file = path.join(directory, entry);
    return statSync(file).isDirectory() ? walk(file) : [file];
  });
}

function htmlText(html) {
  return html
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&(?:nbsp|amp|quot|#39);/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

const htmlFiles = walk(dist).filter((file) => file.endsWith(".html"));
assert.ok(htmlFiles.length >= 4, `expected several generated pages, found ${htmlFiles.length}`);

const placeholders = [
  "<legal entity name>",
  "<privacy@yourdomain>",
  "<postal address>",
  "privacy@yourdomain",
  "yourdomain",
];

for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  const relative = path.relative(dist, file);
  assert.match(html, /<title>\s*[^<]+\s*<\/title>/i, `${relative}: missing title`);
  assert.match(
    html,
    /<meta\s+[^>]*name=["']description["'][^>]*content=["'][^"']+["']/i,
    `${relative}: missing meta description`,
  );
  assert.equal((html.match(/<h1\b/gi) ?? []).length, 1, `${relative}: expected exactly one h1`);
  assert.ok(!html.includes(legacyHost), `${relative}: references the legacy GitHub Pages host`);
  assert.ok(!html.includes(legacyBase), `${relative}: references the legacy project-site base path`);

  const canonical = html.match(/<link\s+[^>]*rel=["']canonical["'][^>]*href=["']([^"']+)["']/i)?.[1];
  if (canonical) {
    assert.ok(
      canonical === productionHost || canonical.startsWith(`${productionHost}/`),
      `${relative}: canonical URL is outside ${productionHost}: ${canonical}`,
    );
  }

  const lower = html.toLowerCase();
  for (const token of placeholders) {
    assert.ok(!lower.includes(token), `${relative}: unfilled launch placeholder ${token}`);
  }
}

function page(relative) {
  const file = path.join(dist, relative, "index.html");
  assert.ok(existsSync(file), `missing generated page ${relative}/index.html`);
  return readFileSync(file, "utf8");
}

const privacy = page("privacy");
const privacyText = htmlText(privacy);
for (const expected of [
  "ORESoftware",
  "alexander.d.mills@gmail.com",
  "165 24th Ave, Santa Cruz, CA 95062, United States",
  "What we collect and why",
  "Retention",
  "Your choices and rights",
  "Security",
  "never later than the approved 100-hour ceiling",
  "is outside Sonus Auris automatic retention",
  "Exporting does not delay deletion of the app-private copy",
]) {
  assert.ok(privacyText.includes(expected), `privacy policy missing ${expected}`);
}
assert.match(privacy, /mailto:alexander\.d\.mills@gmail\.com/i, "privacy contact is not a mailto link");

const deletion = page("account-deletion");
const deletionText = htmlText(deletion);
for (const expected of [
  "Delete your Sonus Auris account and data",
  "Settings → Delete account",
  "What gets deleted vs. retained",
  "within 30 days",
  "alexander.d.mills@gmail.com",
  "Local copies you deliberately exported",
  "Outside Sonus Auris app storage and automatic retention",
  "Exporting a local copy does not delay deletion",
]) {
  assert.ok(deletionText.includes(expected), `account deletion page missing ${expected}`);
}
assert.match(deletion, /mailto:alexander\.d\.mills@gmail\.com/i, "deletion contact is not a mailto link");

const home = readFileSync(path.join(dist, "index.html"), "utf8");
const homeText = htmlText(home);
for (const label of ["App Store", "Google Play", "Windows", "macOS", "Linux"]) {
  assert.ok(homeText.includes(label), `home page missing ${label} download affordance`);
}

for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  for (const match of html.matchAll(/href=["']([^"']+)["']/gi)) {
    const href = match[1];
    if (/^(?:https?:|mailto:|tel:|data:|javascript:|#)/i.test(href)) continue;
    const target = href.split(/[?#]/, 1)[0];
    if (!target.startsWith("/")) continue;
    const candidate = resolveBuildPath(target, dist);
    assert.ok(existsSync(candidate), `${path.relative(dist, file)}: broken internal link ${href}`);
  }
}

// Absolute same-origin asset references that live in `content=` attributes
// rather than `href=`/`src=`. The walk above only follows `href=`, and
// tests/build-security.test.mjs matches only <a|form|iframe|link|script ...
// action|href|src>, so a <meta property="og:image"> pointing at a deleted file
// is invisible to every other gate: the build succeeds and ships a 404 to every
// link unfurler. Resolve by exact origin and keep decoded paths inside dist/.
for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  for (const match of html.matchAll(/<meta\b[^>]*\bcontent=["']([^"']+)["'][^>]*>/gi)) {
    const value = match[1];
    const candidate = resolveSameOriginBuildTarget(value, productionHost, dist);
    if (candidate === null) continue;
    assert.ok(
      existsSync(candidate),
      `${path.relative(dist, file)}: <meta> references ${value}, which is not in the build`,
    );
  }
}

// Astro inlines PUBLIC_* variables at build time, so a value the deployment
// workflow forgets does not error — the component silently renders its
// unconfigured fallback and the site ships dead CTAs. The release workflow now
// has exactly one framework build; scope the variable contract to that uniquely
// named step and reject any return of a second Astro action/build path.
const deployWorkflow = path.resolve(".github/workflows/deploy.yml");
const envExample = path.resolve(".env.example");
assert.ok(existsSync(deployWorkflow), ".github/workflows/deploy.yml is missing");
assert.ok(existsSync(envExample), ".env.example is missing");

const workflow = readFileSync(deployWorkflow, "utf8");
const productionEnvironment = publicEnvForNamedStep(
  workflow,
  "Build and verify the exact production-root static site",
);
assert.equal(
  (workflow.match(/\bnpm run check\b/g) ?? []).length,
  1,
  "deploy workflow must run exactly one production framework build and verification path",
);
assert.equal(
  (workflow.match(/\bnpm run evidence:pages\b/g) ?? []).length,
  1,
  "deploy workflow must seal the verified Pages tree exactly once",
);
assert.ok(
  !workflow.includes("withastro/action@"),
  "deploy workflow must not rebuild after verification with withastro/action",
);

const forwarded = new Set(productionEnvironment.keys());
const documented = publicKeysFromDotenv(readFileSync(envExample, "utf8"));
const srcDir = path.resolve("src");
const referenced = new Set();
for (const file of walk(srcDir)) {
  if (!/\.(astro|ts|tsx|js|mjs)$/.test(file)) continue;
  for (const match of readFileSync(file, "utf8").matchAll(/PUBLIC_[A-Z0-9_]+/g)) {
    referenced.add(match[0]);
  }
}

const missingFromWorkflow = [...referenced].filter((name) => !forwarded.has(name)).sort();
assert.deepEqual(
  missingFromWorkflow,
  [],
  `.github/workflows/deploy.yml does not forward ${missingFromWorkflow.join(", ")} in the ` +
    "single production build. src/ reads these, so production would render an unconfigured fallback.",
);

const unreferencedWorkflowKeys = [...forwarded].filter((name) => !referenced.has(name)).sort();
assert.deepEqual(
  unreferencedWorkflowKeys,
  [],
  `deploy workflow forwards unreferenced variables: ${unreferencedWorkflowKeys.join(", ")}`,
);

const missingFromExample = [...referenced].filter((name) => !documented.has(name)).sort();
assert.deepEqual(
  missingFromExample,
  [],
  `.env.example does not document source variables: ${missingFromExample.join(", ")}`,
);

const staleExampleKeys = [...documented].filter((name) => !referenced.has(name)).sort();
assert.deepEqual(
  staleExampleKeys,
  [],
  `.env.example documents unreferenced variables: ${staleExampleKeys.join(", ")}`,
);

console.log(`Verified ${htmlFiles.length} generated HTML pages for ${productionHost}.`);
