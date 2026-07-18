import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

const distUrl = new URL("../dist/", import.meta.url);
const html = await readFile(new URL("index.html", distUrl), "utf8");

async function generatedCss() {
  const assetDirectory = new URL("_astro/", distUrl);
  const assetNames = await readdir(assetDirectory);
  const stylesheets = assetNames.filter((name) => name.endsWith(".css"));
  return Promise.all(
    stylesheets.map((name) => readFile(new URL(name, assetDirectory), "utf8")),
  );
}

test("production HTML has no inline executable script or event handler", () => {
  const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)];
  assert.ok(scripts.length > 0, "expected the same-origin site script");

  for (const [, attributes, body] of scripts) {
    assert.match(attributes, /\bsrc\s*=\s*["'][^"']+["']/i);
    assert.equal(body.trim(), "");
  }

  assert.doesNotMatch(html, /\son[a-z]+\s*=/i);
  assert.doesNotMatch(
    html,
    /<(?:script|img|source|iframe|audio|video)\b[^>]*(?:src|href)\s*=\s*["'](?:https?:|\/\/|blob:)/i,
  );
  assert.doesNotMatch(
    html,
    /<link\b(?=[^>]*\brel=["'](?:stylesheet|preload|modulepreload|icon|manifest)["'])(?=[^>]*\bhref=["'](?:https?:|\/\/|blob:))[^>]*>/i,
  );
});

test("production CSS needs no external style, font, or image origin", async () => {
  for (const css of await generatedCss()) {
    assert.doesNotMatch(css, /@import\s+(?:url\()?['"]?https?:\/\//i);
    assert.doesNotMatch(css, /url\(\s*['"]?(?:https?:|\/\/|blob:)/i);
  }
});

test("same-origin executable asset is present in the production build", async () => {
  assert.match(html, /<script\b[^>]*\bsrc=["']\/site\.js["'][^>]*><\/script>/i);
  const script = await readFile(new URL("site.js", distUrl), "utf8");
  assert.doesNotMatch(script, /(?:https?:\/\/|wss?:\/\/|blob:|data:)/i);
});
