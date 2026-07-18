import { readFile } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

const page = await readFile(new URL("../src/pages/index.astro", import.meta.url), "utf8");
const layout = await readFile(new URL("../src/layouts/BaseLayout.astro", import.meta.url), "utf8");
const globalCss = await readFile(new URL("../src/styles/global.css", import.meta.url), "utf8");

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
