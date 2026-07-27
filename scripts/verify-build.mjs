import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

const dist = path.resolve("dist");
assert.ok(existsSync(dist), "dist/ is missing; run the Astro build first");

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
]) {
  assert.ok(deletionText.includes(expected), `account deletion page missing ${expected}`);
}
assert.match(deletion, /mailto:alexander\.d\.mills@gmail\.com/i, "deletion contact is not a mailto link");

const home = readFileSync(path.join(dist, "index.html"), "utf8");
const homeText = htmlText(home);
for (const label of ["App Store", "Google Play", "Windows", "macOS", "Linux"]) {
  assert.ok(homeText.includes(label), `home page missing ${label} download affordance`);
}

const basePrefixes = ["/sonus-auris-site.web", ""];
for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  for (const match of html.matchAll(/href=["']([^"']+)["']/gi)) {
    const href = match[1];
    if (/^(?:https?:|mailto:|tel:|data:|javascript:|#)/i.test(href)) continue;
    let target = href.split(/[?#]/, 1)[0];
    for (const prefix of basePrefixes) {
      if (prefix && target.startsWith(prefix)) target = target.slice(prefix.length);
    }
    if (!target.startsWith("/")) continue;
    const relative = target.replace(/^\/+/, "");
    const candidate = target.endsWith("/")
      ? path.join(dist, relative, "index.html")
      : path.join(dist, relative || "index.html");
    assert.ok(existsSync(candidate), `${path.relative(dist, file)}: broken internal link ${href}`);
  }
}

console.log(`Verified ${htmlFiles.length} generated HTML pages.`);
