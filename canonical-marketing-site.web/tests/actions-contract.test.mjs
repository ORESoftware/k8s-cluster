import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const pagesWorkflow = await readFile(
  new URL("../.github/workflows/pages.yml", import.meta.url),
  "utf8",
);
const liveBrowserWorkflow = await readFile(
  new URL("../.github/workflows/live-browser-smoke.yml", import.meta.url),
  "utf8",
);

test("Pages deploys only a successful, tested main-branch commit", () => {
  assert.match(pagesWorkflow, /workflow_run:\s*\n\s+workflows: \[ci\]/);
  assert.match(pagesWorkflow, /types: \[completed\]/);
  assert.match(pagesWorkflow, /branches: \[main\]/);
  assert.doesNotMatch(pagesWorkflow, /\n\s+push:\s*\n/);
  assert.doesNotMatch(pagesWorkflow, /workflow_dispatch:/);
  assert.match(
    pagesWorkflow,
    /workflow_run\.conclusion == 'success'/,
  );
  assert.match(pagesWorkflow, /workflow_run\.event == 'push'/);
  assert.match(pagesWorkflow, /workflow_run\.head_branch == 'main'/);
  assert.match(
    pagesWorkflow,
    /ref: \$\{\{ github\.event\.workflow_run\.head_sha \}\}/,
  );
});

test("Pages build and deploy checks are uniquely named and least privilege", () => {
  assert.match(pagesWorkflow, /\n  pages-build:\n/);
  assert.match(pagesWorkflow, /\n  pages-deploy:\n/);
  assert.match(pagesWorkflow, /needs: pages-build/);
  assert.match(pagesWorkflow, /pages: write/);
  assert.match(pagesWorkflow, /id-token: write/);
  assert.match(pagesWorkflow, /permissions:\s*\n\s+contents: read/);
});

test("live browser smoke is external-signal isolation, not a PR gate", () => {
  assert.match(liveBrowserWorkflow, /workflow_dispatch:/);
  assert.match(liveBrowserWorkflow, /schedule:\s*\n\s+- cron:/);
  assert.doesNotMatch(liveBrowserWorkflow, /\n\s+pull_request:/);
  assert.doesNotMatch(liveBrowserWorkflow, /\n\s+push:/);
  assert.doesNotMatch(liveBrowserWorkflow, /workflow_run:/);
  assert.match(liveBrowserWorkflow, /permissions:\s*\n\s+contents: read/);
  assert.match(liveBrowserWorkflow, /group: live-browser-smoke/);
  assert.doesNotMatch(liveBrowserWorkflow, /\$\{\{\s*secrets(?:\.|\[)/);
});

test("live browser smoke is lockfile-strict and executes the deployed policy", () => {
  assert.match(liveBrowserWorkflow, /CANONICAL_SITE_TEST_URL: https:\/\/canonical\.cloud/);
  assert.match(liveBrowserWorkflow, /CANONICAL_REQUIRE_SECURITY_HEADERS: "1"/);
  assert.match(liveBrowserWorkflow, /PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD: "1"/);
  assert.match(liveBrowserWorkflow, /PUPPETEER_SKIP_DOWNLOAD: "true"/);
  assert.match(liveBrowserWorkflow, /run: npm ci/);
  assert.doesNotMatch(liveBrowserWorkflow, /npm install|npm i\b/);
  assert.match(liveBrowserWorkflow, /playwright install --with-deps chromium/);
  assert.match(
    liveBrowserWorkflow,
    /tests\/container-security-playwright\.test\.mjs/,
  );
});

test("live browser diagnostics are immutable, failure-only, and short-lived", () => {
  assert.match(
    liveBrowserWorkflow,
    /CANONICAL_BROWSER_ARTIFACT_DIR: artifacts\/live-browser-smoke/,
  );
  assert.match(
    liveBrowserWorkflow,
    /if: failure\(\)\s*\n\s*uses: actions\/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a/,
  );
  assert.doesNotMatch(liveBrowserWorkflow, /if: always\(\)/);
  assert.equal(
    liveBrowserWorkflow.match(/actions\/upload-artifact@/g)?.length,
    1,
  );
  assert.match(liveBrowserWorkflow, /path: artifacts\/live-browser-smoke/);
  assert.match(liveBrowserWorkflow, /if-no-files-found: error/);
  assert.match(liveBrowserWorkflow, /retention-days: 14/);
  assert.doesNotMatch(liveBrowserWorkflow, /include-hidden-files:\s*true/);
});
