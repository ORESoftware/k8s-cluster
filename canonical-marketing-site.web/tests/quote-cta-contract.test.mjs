import { readFile } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

const layout = await readFile(
  new URL("../src/layouts/BaseLayout.astro", import.meta.url),
  "utf8",
);

test("marketing navigation exposes the authenticated canonical.plus quote entry point", () => {
  assert.match(layout, /const quoteHref = 'https:\/\/app\.canonical\.plus\/u\/quote';/);
  assert.match(layout, /id="nav-sign-in">Sign in<\/a>/);
  assert.match(layout, /id="nav-quote">Get a quote · under 5 min<\/a>/);
});

test("quote links use one exact HTTPS destination and never carry tokens", () => {
  const destinations = [...layout.matchAll(/href=\{quoteHref\}/g)];
  assert.ok(destinations.length >= 4, "expected nav and footer quote links");
  assert.doesNotMatch(layout, /app\.canonical\.plus\/u\/quote\?[^'"\s]*(?:token|jwt|access_token)=/i);
});
