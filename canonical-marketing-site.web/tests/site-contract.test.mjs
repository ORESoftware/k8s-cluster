import { readFile } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

const page = await readFile(new URL("../src/pages/index.astro", import.meta.url), "utf8");
const layout = await readFile(new URL("../src/layouts/BaseLayout.astro", import.meta.url), "utf8");
const globalCss = await readFile(new URL("../src/styles/global.css", import.meta.url), "utf8");
const siteScript = await readFile(new URL("../public/site.js", import.meta.url), "utf8");

test("landing page keeps the compliance services visible", () => {
  for (const label of [
    "SOC 2 Attestation",
    "FedRAMP Authorization",
    "HIPAA Compliance",
    "vCISO & IT Advisory",
  ]) {
    assert.ok(
      page.includes(label) || page.includes(label.replace("&", "&amp;")),
      `missing service label: ${label}`,
    );
  }
});

test("landing page advertises every audited framework", () => {
  for (const framework of ["SOC 2", "FedRAMP", "HIPAA", "ISO 27001", "PCI DSS", "GDPR"]) {
    assert.ok(page.includes(framework), `missing framework: ${framework}`);
  }
});

test("primary calls to action stay internal / safe", () => {
  assert.match(page, /href="#contact"/);
  assert.match(page, /href="#services"/);
  assert.match(page, /href="mailto:compliance@canonical\.cloud"/);
  assert.doesNotMatch(page, /javascript:/i);
});

test("layout keeps production metadata and viewport controls", () => {
  assert.match(layout, /<html lang="en">/);
  assert.match(layout, /name="viewport"/);
  assert.match(layout, /name="description"/);
  assert.match(layout, /property="og:title"/);
  assert.match(layout, /property="og:description"/);
  assert.match(layout, /<title>\{title\} \| canonical\.cloud<\/title>/);
  assert.match(layout, /target="_blank" rel="noopener noreferrer"/);
});

test("static image supplies the same defensive headers as the server fallback", async () => {
  const dockerfile = await readFile(new URL("../Dockerfile", import.meta.url), "utf8");
  const nginx = await readFile(new URL("../nginx.conf", import.meta.url), "utf8");

  assert.match(dockerfile, /COPY nginx\.conf \/etc\/nginx\/conf\.d\/default\.conf/);
  for (const header of [
    "Content-Security-Policy",
    "X-Content-Type-Options",
    "Referrer-Policy",
    "X-Frame-Options",
    "Permissions-Policy",
    "Cross-Origin-Opener-Policy",
  ]) {
    assert.match(nginx, new RegExp(`add_header ${header}`));
  }
  assert.match(nginx, /script-src 'self';/);
  assert.doesNotMatch(nginx, /script-src 'self' 'unsafe-inline'/);
});

test("production image inputs are pinned and test browsers never enter the artifact", async () => {
  const dockerfile = await readFile(new URL("../Dockerfile", import.meta.url), "utf8");

  assert.match(dockerfile, /^FROM node:22-slim@sha256:[0-9a-f]{64} AS build$/m);
  assert.match(dockerfile, /^FROM nginx:1\.27-alpine@sha256:[0-9a-f]{64}$/m);
  assert.match(dockerfile, /PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1/);
  assert.match(dockerfile, /PUPPETEER_SKIP_DOWNLOAD=true/);
  assert.match(dockerfile, /^RUN npm ci$/m);
  assert.doesNotMatch(dockerfile, /npm ci\s*\|\|\s*npm install/);
});

test("layout keeps base-aware links so the site works behind a gateway prefix", () => {
  assert.match(layout, /import\.meta\.env\.BASE_URL/);
  assert.match(layout, /const homeHref = /);
  assert.match(layout, /const siteScriptHref = /);
});

test("source assets do not require third-party CSP exceptions", () => {
  assert.doesNotMatch(globalCss, /@import\s+(?:url\()?['"]?https?:\/\//i);
  assert.doesNotMatch(globalCss, /url\(\s*['"]?(?:https?:|\/\/|blob:)/i);
  assert.match(layout, /<script type="module" src=\{siteScriptHref\}><\/script>/);
  assert.doesNotMatch(layout, /\son[a-z]+\s*=/i);
});

test("nav exposes the section links", () => {
  for (const label of ["Services", "Process", "Frameworks", "About"]) {
    assert.ok(layout.includes(`>${label}<`), `missing nav link: ${label}`);
  }
});

test("skip navigation and named landmarks exist before JavaScript runs", () => {
  assert.match(layout, /<a class="skip-link" href="#main-content">Skip to main content<\/a>/);
  assert.match(layout, /<nav class="nav" id="main-nav" aria-label="Primary navigation">/);
  assert.match(layout, /<main id="main-content" tabindex="-1">/);
  assert.match(layout, /<nav class="footer__columns" aria-label="Footer navigation">/);
  assert.match(layout, /\.skip-link:focus-visible\s*\{/);
  assert.match(layout, /#main-content:focus-visible\s*\{/);
  assert.match(layout, /aria-hidden="true" focusable="false"/);
  assert.match(siteScript, /mainContent\.focus\(\{ preventScroll: true \}\)/);
});

test("mobile navigation has a real open state and synchronized accessibility state", () => {
  assert.match(globalCss, /\.nav \.nav__links\.nav__links--open\s*\{/);
  assert.match(globalCss, /min-height:\s*44px/);
  assert.match(globalCss, /:focus-visible/);
  assert.match(globalCss, /prefers-reduced-motion:\s*reduce/);

  assert.match(layout, /id="nav-toggle"[\s\S]*type="button"/);
  assert.match(layout, /id="nav-toggle"[\s\S]*aria-controls="nav-links"/);
  assert.match(layout, /id="nav-toggle"[\s\S]*aria-expanded="false"/);
  assert.match(layout, /id="nav-toggle"[\s\S]*aria-label="Open navigation"/);

  assert.match(siteScript, /setAttribute\('aria-controls', links\.id\)/);
  assert.match(siteScript, /setAttribute\('aria-expanded', String\(nextOpen\)\)/);
  assert.match(siteScript, /Open navigation/);
  assert.match(siteScript, /Close navigation/);
  assert.match(siteScript, /event\.key === 'Escape'/);
  assert.match(siteScript, /restoreFocus:\s*true/);
  assert.match(siteScript, /matchMedia\('\(max-width: 768px\)'\)/);
});

test("quote and sign-in links use the canonical application boundary", () => {
  assert.match(siteScript, /const APP_SCHEME = 'https'/);
  assert.match(siteScript, /const APP_HOST = 'app\.canonical\.plus'/);
  assert.match(siteScript, /\[APP_SCHEME, APP_HOST\]\.join\('\:\/\/'\)/);
  assert.match(siteScript, /const QUOTE_PATH = '\/u\/quote'/);
  assert.match(siteScript, /const signInUrl = new URL\(QUOTE_PATH, APP_ORIGIN\)/);
  assert.doesNotMatch(siteScript, /\/auth\/start|return_to/);
  assert.match(siteScript, /Get a quote · under 5 min/);
  assert.match(siteScript, /Get a quote in under 5 min/);
  assert.match(siteScript, /signIn\.textContent = 'Sign in'/);
  assert.doesNotMatch(siteScript, /access_token|refresh_token|id_token/i);
});
