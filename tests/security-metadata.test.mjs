import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

const dist = path.resolve('dist');
const productionOrigin = 'https://sonusauris.app';
const enforcedDirectives = new Map([
  ['default-src', ["'self'"]],
  ['base-uri', ["'none'"]],
  ['object-src', ["'none'"]],
  ['script-src', ["'none'"]],
  ['style-src', ["'self'", "'unsafe-inline'"]],
  ['font-src', ["'self'"]],
  ['img-src', ["'self'", 'data:']],
  ['media-src', ["'self'", 'blob:']],
  ['connect-src', ["'none'"]],
  ['frame-src', ["'none'"]],
  ['worker-src', ["'none'"]],
  ['manifest-src', ["'self'"]],
  ['form-action', ["'self'"]],
  ['upgrade-insecure-requests', []],
]);

function read(relative) {
  const file = path.join(dist, relative);
  assert.ok(existsSync(file), `missing generated artifact ${relative}`);
  return readFileSync(file, 'utf8');
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
  const match = tag.match(new RegExp(`\\b${name}\\s*=\\s*(["'])([\\s\\S]*?)\\1`, 'i'));
  return match?.[2] ?? null;
}

function decodeAttribute(value) {
  return value
    .replace(/&#(?:39|x27);|&apos;/gi, "'")
    .replace(/&quot;/gi, '"')
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>')
    .replace(/&amp;/gi, '&');
}

function metaContent(html, selectorName, selectorValue) {
  const tag = [...html.matchAll(/<meta\b[^>]*>/gi)]
    .map((match) => match[0])
    .find(
      (candidate) =>
        (attribute(candidate, selectorName) ?? '').toLowerCase() ===
        selectorValue.toLowerCase(),
    );
  return tag ? attribute(tag, 'content') : null;
}

function directives(csp) {
  const out = new Map();
  for (const clause of csp.split(';')) {
    const tokens = clause.trim().split(/\s+/).filter(Boolean);
    if (tokens.length > 0) out.set(tokens[0].toLowerCase(), tokens.slice(1));
  }
  return out;
}

function assertPolicy(actual, expected, label) {
  for (const [name, sources] of expected) {
    assert.ok(actual.has(name), `${label}: missing CSP directive ${name}`);
    assert.deepEqual(actual.get(name), sources, `${label}: ${name} drifted`);
  }
}

test('every generated page enforces the no-script, no-connect static-site contract', () => {
  const htmlFiles = walk(dist).filter((file) => file.endsWith('.html'));
  assert.ok(htmlFiles.length >= 4, `expected at least four HTML pages, found ${htmlFiles.length}`);

  for (const file of htmlFiles) {
    const relative = path.relative(dist, file);
    const html = readFileSync(file, 'utf8');
    const encodedCsp = metaContent(html, 'http-equiv', 'Content-Security-Policy');
    assert.ok(encodedCsp, `${relative}: missing Content-Security-Policy meta element`);
    assertPolicy(directives(decodeAttribute(encodedCsp)), enforcedDirectives, relative);
    assert.doesNotMatch(
      html,
      /<script\b/i,
      `${relative}: script-src is none but a script element was generated`,
    );

    assert.equal(
      metaContent(html, 'name', 'referrer'),
      'strict-origin-when-cross-origin',
      `${relative}: referrer policy drifted`,
    );
    assert.equal(
      metaContent(html, 'name', 'robots'),
      'index,follow,max-image-preview:large',
      `${relative}: robots meta drifted`,
    );
  }
});

test('future response-header policy cannot silently widen the enforced meta CSP', () => {
  const headers = read('_headers');
  const headerCsp = headers.match(/^\s*Content-Security-Policy:\s*(.+)$/m)?.[1];
  assert.ok(headerCsp, '_headers is missing its future Content-Security-Policy');
  const headerPolicy = directives(headerCsp);
  assertPolicy(headerPolicy, enforcedDirectives, '_headers');
  assert.deepEqual(headerPolicy.get('frame-ancestors'), ["'none'"]);

  for (const requiredHeader of [
    'X-Content-Type-Options: nosniff',
    'Referrer-Policy: strict-origin-when-cross-origin',
    'X-Frame-Options: DENY',
    'Cross-Origin-Opener-Policy: same-origin',
    'Cross-Origin-Resource-Policy: same-origin',
    'X-Permitted-Cross-Domain-Policies: none',
  ]) {
    assert.ok(headers.includes(requiredHeader), `_headers omitted ${requiredHeader}`);
  }
});

test('crawler metadata points only at canonical public store-review routes', () => {
  const robots = read('robots.txt');
  assert.match(robots, /^User-agent:\s*\*/m);
  assert.match(robots, /^Allow:\s*\/$/m);
  assert.match(robots, /^Disallow:\s*\/_headers$/m, 'the inert _headers file should not be indexed');
  assert.match(robots, /^Sitemap:\s*https:\/\/sonusauris\.app\/sitemap\.xml$/m);

  const sitemap = read('sitemap.xml');
  const locations = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);
  assert.deepEqual(locations, [
    `${productionOrigin}/`,
    `${productionOrigin}/privacy/`,
    `${productionOrigin}/support/`,
    `${productionOrigin}/account-deletion/`,
  ]);
});

test('RFC 9116 security contact is canonical, usable, and not near expiry', () => {
  const security = read(path.join('.well-known', 'security.txt'));
  assert.match(security, /^Contact:\s*mailto:[^@\s]+@[^@\s]+\.[a-z]{2,}$/im);
  assert.match(
    security,
    /^Canonical:\s*https:\/\/sonusauris\.app\/\.well-known\/security\.txt$/im,
  );
  assert.match(security, /^Preferred-Languages:\s*en$/im);

  const expiresRaw = security.match(/^Expires:\s*(\S+)$/im)?.[1];
  assert.ok(expiresRaw, 'security.txt is missing Expires');
  const expiresAt = Date.parse(expiresRaw);
  assert.ok(Number.isFinite(expiresAt), `security.txt Expires is invalid: ${expiresRaw}`);

  const remainingMs = expiresAt - Date.now();
  assert.ok(remainingMs > 30 * 24 * 60 * 60 * 1_000, 'security.txt expires in 30 days or less');
  assert.ok(
    remainingMs <= 370 * 24 * 60 * 60 * 1_000,
    'security.txt Expires must stay within roughly one year',
  );
});
