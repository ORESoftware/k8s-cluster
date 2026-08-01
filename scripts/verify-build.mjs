import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

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
    const relative = target.replace(/^\/+/, "");
    const candidate = target.endsWith("/")
      ? path.join(dist, relative, "index.html")
      : path.join(dist, relative || "index.html");
    assert.ok(existsSync(candidate), `${path.relative(dist, file)}: broken internal link ${href}`);
  }
}

// Absolute same-origin asset references that live in `content=` attributes
// rather than `href=`/`src=`. The walk above only follows `href=`, and
// tests/build-security.test.mjs matches only <a|form|iframe|link|script ...
// action|href|src>, so a <meta property="og:image"> pointing at a deleted file
// is invisible to every other gate: the build succeeds and ships a 404 to every
// link unfurler. Verified by deleting public/og.svg — `npm run check` and all
// 48 Playwright tests passed while dist/index.html still advertised it.
for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  for (const match of html.matchAll(/<meta\b[^>]*\bcontent=["']([^"']+)["'][^>]*>/gi)) {
    const value = match[1];
    if (!value.startsWith(productionHost)) continue;
    const target = value.slice(productionHost.length).split(/[?#]/, 1)[0];
    if (!target || target === "/") continue;
    const relative = target.replace(/^\/+/, "");
    const candidate = target.endsWith("/")
      ? path.join(dist, relative, "index.html")
      : path.join(dist, relative);
    assert.ok(
      existsSync(candidate),
      `${path.relative(dist, file)}: <meta> references ${value}, which is not in the build`,
    );
  }
}

// Astro inlines PUBLIC_* variables at build time, so one the deploy workflow
// forgets to forward does not error — the component silently renders its
// unconfigured fallback and the site ships with dead CTAs. Nothing else in the
// repo reads a workflow file, so this is the only place the two can be kept
// honest with each other.
const deployWorkflow = path.resolve(".github/workflows/deploy.yml");
if (existsSync(deployWorkflow)) {
  const workflow = readFileSync(deployWorkflow, "utf8");
  const forwarded = new Set(
    [...workflow.matchAll(/^\s{8,}(PUBLIC_[A-Z0-9_]+):/gm)].map((m) => m[1]),
  );
  const srcDir = path.resolve("src");
  const referenced = new Set();
  for (const file of walk(srcDir)) {
    if (!/\.(astro|ts|tsx|js|mjs)$/.test(file)) continue;
    for (const match of readFileSync(file, "utf8").matchAll(/PUBLIC_[A-Z0-9_]+/g)) {
      referenced.add(match[0]);
    }
  }
  const missing = [...referenced].filter((name) => !forwarded.has(name)).sort();
  assert.deepEqual(
    missing,
    [],
    `.github/workflows/deploy.yml does not forward ${missing.join(", ")} to the build. ` +
      "src/ reads these, so production renders their unconfigured fallback. Add each to " +
      "the Build with Astro `env:` block (and define it as a repository variable).",
  );
}

console.log(`Verified ${htmlFiles.length} generated HTML pages for ${productionHost}.`);
