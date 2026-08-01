import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

const dist = path.resolve("dist");
const productionOrigin = "https://sonusauris.app";

function read(relative) {
  const file = path.join(dist, relative);
  assert.ok(existsSync(file), `missing generated artifact ${relative}`);
  return readFileSync(file, "utf8");
}

function walk(directory) {
  return readdirSync(directory).flatMap((entry) => {
    const file = path.join(directory, entry);
    return statSync(file).isDirectory() ? walk(file) : [file];
  });
}

function attribute(tag, name) {
  // Capture using the opening quote as a backreference. A generic
  // ["']... ["'] pattern truncates a double-quoted CSP at its first embedded
  // apostrophe (`'self'`) and can therefore make a correct build look weak.
  const match = tag.match(new RegExp(`\\b${name}\\s*=\\s*(["'])([\\s\\S]*?)\\1`, "i"));
  return match?.[2] ?? null;
}

function decodeAttribute(value) {
  return value
    .replace(/&#(?:39|x27);|&apos;/gi, "'")
    .replace(/&quot;/gi, '"')
    .replace(/&lt;/gi, "<")
    .replace(/&gt;/gi, ">")
    .replace(/&amp;/gi, "&");
}

function cspFrom(html) {
  const tag = [...html.matchAll(/<meta\b[^>]*>/gi)]
    .map((match) => match[0])
    .find((candidate) =>
      /^content-security-policy$/i.test(attribute(candidate, "http-equiv") ?? ""),
    );
  assert.ok(tag, "missing Content-Security-Policy meta element");
  const content = attribute(tag, "content");
  assert.ok(content, "Content-Security-Policy meta element has no content");
  return decodeAttribute(content);
}

function directives(csp) {
  const out = new Map();
  for (const clause of csp.split(";")) {
    const tokens = clause.trim().split(/\s+/).filter(Boolean);
    if (tokens.length > 0) out.set(tokens[0].toLowerCase(), tokens.slice(1));
  }
  return out;
}

test("every generated page enforces the no-script CSP contract", () => {
  const htmlFiles = walk(dist).filter((file) => file.endsWith(".html"));
  assert.ok(htmlFiles.length >= 4, `expected at least four HTML pages, found ${htmlFiles.length}`);

  for (const file of htmlFiles) {
    const relative = path.relative(dist, file);
    const html = readFileSync(file, "utf8");
    const policy = directives(cspFrom(html));

    for (const [name, required] of [
      ["default-src", "'self'"],
      ["base-uri", "'none'"],
      ["object-src", "'none'"],
      ["script-src", "'none'"],
      ["connect-src", "'none'"],
      ["frame-src", "'none'"],
      ["worker-src", "'none'"],
      ["form-action", "'self'"],
    ]) {
      assert.ok(policy.get(name)?.includes(required), `${relative}: ${name} must include ${required}`);
    }
    assert.ok(policy.has("upgrade-insecure-requests"), `${relative}: CSP must upgrade insecure requests`);
    assert.doesNotMatch(html, /<script\b/i, `${relative}: script-src is none but a script element was generated`);

    const referrer = [...html.matchAll(/<meta\b[^>]*>/gi)]
      .map((match) => match[0])
      .find((candidate) => /^referrer$/i.test(attribute(candidate, "name") ?? ""));
    assert.equal(
      attribute(referrer ?? "", "content"),
      "strict-origin-when-cross-origin",
      `${relative}: referrer policy drifted`,
    );
  }
});

test("crawler metadata points only at canonical public routes", () => {
  const robots = read("robots.txt");
  assert.match(robots, /^User-agent:\s*\*/m);
  assert.match(robots, /^Allow:\s*\/$/m);
  assert.match(robots, /^Disallow:\s*\/_headers$/m, "the inert _headers file should not be indexed");
  assert.match(robots, /^Sitemap:\s*https:\/\/sonusauris\.app\/sitemap\.xml$/m);

  const sitemap = read("sitemap.xml");
  const locations = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);
  assert.deepEqual(
    locations,
    [
      `${productionOrigin}/`,
      `${productionOrigin}/privacy/`,
      `${productionOrigin}/support/`,
      `${productionOrigin}/account-deletion/`,
    ],
  );
});

test("RFC 9116 security contact is canonical, usable, and not near expiry", () => {
  const security = read(path.join(".well-known", "security.txt"));
  assert.match(security, /^Contact:\s*mailto:[^@\s]+@[^@\s]+\.[a-z]{2,}$/im);
  assert.match(
    security,
    /^Canonical:\s*https:\/\/sonusauris\.app\/\.well-known\/security\.txt$/im,
  );
  assert.match(security, /^Preferred-Languages:\s*en$/im);

  const expiresRaw = security.match(/^Expires:\s*(\S+)$/im)?.[1];
  assert.ok(expiresRaw, "security.txt is missing Expires");
  const expiresAt = Date.parse(expiresRaw);
  assert.ok(Number.isFinite(expiresAt), `security.txt Expires is invalid: ${expiresRaw}`);

  const remainingMs = expiresAt - Date.now();
  assert.ok(remainingMs > 30 * 24 * 60 * 60 * 1_000, "security.txt expires in 30 days or less");
  assert.ok(remainingMs <= 370 * 24 * 60 * 60 * 1_000, "security.txt Expires must stay within roughly one year");
});
