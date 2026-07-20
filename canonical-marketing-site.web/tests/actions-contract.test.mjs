import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const pagesWorkflow = await readFile(
  new URL("../.github/workflows/pages.yml", import.meta.url),
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
