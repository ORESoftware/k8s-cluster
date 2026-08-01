// Fast source-contract tests: assert (without a browser) that the landing page,
// layout, and CSS keep their key marketing content, CTAs, and responsive rules.
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

const page = await readFile(new URL("../src/pages/index.astro", import.meta.url), "utf8");
const layout = await readFile(new URL("../src/layouts/Layout.astro", import.meta.url), "utf8");
const css = await readFile(new URL("../src/styles/global.css", import.meta.url), "utf8");
const useCases = await readFile(new URL("../src/pages/use-cases.astro", import.meta.url), "utf8");
const investors = await readFile(new URL("../src/pages/investors.astro", import.meta.url), "utf8");

test("landing page keeps every coordination primitive visible", () => {
  for (const label of [
    "Locks & Semaphores",
    "Rate Limiting",
    "Cron & Scheduling",
    "Config KV & Watches",
    "Leader Election",
    "Service Discovery",
  ]) {
    assert.ok(
      page.includes(label) || page.includes(label.replace("&", "&amp;")),
      `missing service label: ${label}`,
    );
  }
});

test("landing page speaks directly to AI-agent fleet coordination", () => {
  assert.match(page, /AI-agent\s+fleets/);
  for (const phrase of [
    "AI agent coordination",
    "Claim work once",
    "Gate scarce tools",
    "Elect supervisors",
  ]) {
    assert.ok(page.includes(phrase), `missing agent positioning: ${phrase}`);
  }
  assert.match(layout, /AI-agent fleets/);
});

test("primary calls to action remain internal and deploy-prefix safe", () => {
  assert.match(page, /href="#start"/);
  assert.match(page, /href="#services"/);
  assert.match(page, /href=\{`\$\{base\}api\/info`\}/);
  assert.doesNotMatch(page, /javascript:/i);
});

test("layout keeps production metadata and viewport controls", () => {
  assert.match(layout, /<html lang="en">/);
  assert.match(layout, /name="viewport"/);
  assert.match(layout, /name="description"/);
  assert.match(layout, /property="og:title"/);
  assert.match(layout, /property="og:description"/);
});

test("responsive CSS protects mobile nav, grids, and terminal overflow", () => {
  assert.match(css, /@media \(max-width: 880px\)/);
  assert.match(css, /\.grid-3\s*\{\s*grid-template-columns: 1fr;/);
  assert.match(css, /\.nav__links\s*\{\s*display: none;/);
  assert.match(css, /overflow-x: auto;/);
});

test("CNAME and astro config agree on the canonical domain fiducia.cloud", async () => {
  const cname = (await readFile(new URL("../public/CNAME", import.meta.url), "utf8")).trim();
  assert.equal(cname, "fiducia.cloud", "GitHub Pages CNAME must stay the canonical domain");

  const { default: config } = await import("../astro.config.mjs");
  assert.equal(
    config.site,
    "https://fiducia.cloud",
    "astro `site` must match the Pages CNAME or emitted canonical/sitemap URLs break",
  );
  assert.equal(new URL(config.site).hostname, cname, "CNAME and astro site must be one domain");
});

test("consensus pitching carries the crash-fault (CFT, not Byzantine) qualifier", () => {
  // Language policy (fiducia-monorepo/docs/use-cases-exploration.md): Raft is
  // CFT — one operator's non-lying nodes. The site may sell consensus,
  // elections, and exactly-once coordination, but never Byzantine/trustless
  // security. If the pitch is present, the qualifier must be too.
  // Every page that pitches consensus must be in this map, not just the
  // landing page: use-cases.astro and investors.astro together carry most of
  // the consensus copy on the site, and were previously unchecked.
  const sources = {
    "src/pages/index.astro": page,
    "src/layouts/Layout.astro": layout,
    "src/pages/use-cases.astro": useCases,
    "src/pages/investors.astro": investors,
  };
  const pitches = /consensus|election|financial|payout|custody|exactly-once/i;
  const qualifier = /crash-fault\s+tolerant\s*\(CFT\)[^.]*not\s+Byzantine/is;

  const pitching = Object.entries(sources)
    .filter(([, text]) => pitches.test(text))
    .map(([name]) => name);
  assert.ok(pitching.length > 0, "expected the marketing site to pitch consensus somewhere");

  assert.ok(
    Object.values(sources).some((text) => qualifier.test(text)),
    `these sources pitch consensus/elections (${pitching.join(", ")}) but no page copy ` +
      "carries the crash-fault qualifier. Add language like “crash-fault tolerant " +
      "(CFT), not Byzantine” — see fiducia-monorepo/docs/use-cases-exploration.md " +
      "(“Raft is CFT, not BFT. Never claim otherwise.”)",
  );

  // And the site must never over-claim the opposite.
  for (const [name, text] of Object.entries(sources)) {
    assert.doesNotMatch(text, /trustless/i, `${name}: never pitch trustless security`);
    assert.doesNotMatch(
      text,
      /byzantine[\s-]+fault[\s-]+toleran|\bBFT\b/i,
      `${name}: never claim Byzantine fault tolerance`,
    );
  }
});
