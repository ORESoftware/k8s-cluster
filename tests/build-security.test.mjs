import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { relative, resolve, sep } from "node:path";
import test from "node:test";

const dist = resolve("dist");
const site = new URL(
  process.env.SONUS_AURIS_SITE_URL ?? "https://sonus-auris.github.io",
);
const base = (process.env.SONUS_AURIS_SITE_BASE ?? "/sonus-auris-site.web")
  .replace(/\/+$/, "");

function walk(directory) {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

const pages = walk(dist)
  .filter((path) => path.endsWith(".html"))
  .map((path) => ({ path, html: readFileSync(path, "utf8") }));

test("every page has one route-matching HTTPS canonical URL", () => {
  assert.ok(pages.length > 0, "build did not produce any HTML pages");
  for (const page of pages) {
    const matches = [
      ...page.html.matchAll(
        /<link\b[^>]*\brel=["']canonical["'][^>]*\bhref=["']([^"']+)["'][^>]*>/gi,
      ),
    ];
    assert.equal(matches.length, 1, `${relative(dist, page.path)} canonical count`);

    const route = relative(dist, page.path)
      .split(sep)
      .join("/")
      .replace(/(?:^|\/)index\.html$/, "");
    const expected = new URL(`${base}/${route}${route ? "/" : ""}`, site);
    assert.equal(matches[0][1], expected.href);
    assert.equal(expected.protocol, "https:");
    assert.equal(expected.search, "");
    assert.equal(expected.hash, "");
  }
});

test("external links use HTTPS and isolate their opener", () => {
  for (const page of pages) {
    for (const match of page.html.matchAll(/<a\b([^>]*)>/gi)) {
      const attributes = match[1];
      const href = /\bhref=["']([^"']+)["']/i.exec(attributes)?.[1];
      if (!href || !/^https?:/i.test(href)) continue;

      const url = new URL(href);
      assert.equal(url.protocol, "https:", `${relative(dist, page.path)}: ${href}`);
      if (url.origin === site.origin) continue;

      const rel = /\brel=["']([^"']+)["']/i.exec(attributes)?.[1] ?? "";
      const tokens = new Set(rel.toLowerCase().split(/\s+/));
      assert.ok(tokens.has("noopener"), `${href} is missing rel=noopener`);
      assert.ok(tokens.has("noreferrer"), `${href} is missing rel=noreferrer`);
    }
  }
});

test("navigation and resource attributes reject active or mixed-content URLs", () => {
  for (const page of pages) {
    for (const match of page.html.matchAll(
      /<(?:a|form|iframe|img|link|script)\b[^>]*\b(?:action|href|src)=["']([^"']+)["']/gi,
    )) {
      const value = match[1].trim();
      assert.doesNotMatch(
        value,
        /^(?:javascript|vbscript|data):/i,
        `${relative(dist, page.path)} contains an active URL`,
      );
      assert.doesNotMatch(
        value,
        /^http:/i,
        `${relative(dist, page.path)} contains mixed content`,
      );
    }
  }
});
