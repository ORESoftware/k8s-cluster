import { readFile } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

const page = await readFile(new URL("../src/pages/index.astro", import.meta.url), "utf8");
const layout = await readFile(new URL("../src/layouts/BaseLayout.astro", import.meta.url), "utf8");

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
});

test("layout keeps base-aware links so the site works behind a gateway prefix", () => {
  assert.match(layout, /import\.meta\.env\.BASE_URL/);
  assert.match(layout, /const homeHref = /);
});

test("nav exposes the section links", () => {
  for (const label of ["Services", "Process", "Frameworks", "About"]) {
    assert.ok(layout.includes(`>${label}<`), `missing nav link: ${label}`);
  }
});
