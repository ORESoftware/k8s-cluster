import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const release = await readFile(
  new URL("../.github/workflows/release.yml", import.meta.url),
  "utf8",
);

test("container release follows successful main CI and rejects stale commits", () => {
  assert.match(release, /workflow_run:\s*\n\s+workflows: \[ci\]/);
  assert.match(release, /workflow_run\.conclusion == 'success'/);
  assert.match(release, /workflow_run\.event == 'push'/);
  assert.match(release, /workflow_run\.head_branch == 'main'/);
  assert.match(release, /branches: \[main\]/);
  assert.doesNotMatch(release, /workflow_dispatch:/);
  assert.match(release, /git ls-remote .*refs\/heads\/main/);
  assert.match(release, /ref: \$\{\{ env\.RELEASE_SHA \}\}/);
});

test("release publishes both process images with immutable provenance", () => {
  assert.match(release, /target: web/);
  assert.match(release, /target: revoker/);
  assert.match(release, /canonical-web-server/);
  assert.match(release, /canonical-session-revoker/);
  assert.match(release, /\$\{\{ env\.RELEASE_SHA \}\}/);
  assert.match(release, /provenance: mode=max/g);
  assert.match(release, /sbom: true/g);
  assert.match(release, /attest-build-provenance@[0-9a-f]{40}/g);
  assert.doesNotMatch(release, /:main|:latest/);
});

test("release has no cluster credential or direct deployment path", () => {
  assert.match(release, /packages: write/);
  assert.match(release, /attestations: write/);
  assert.match(release, /id-token: write/);
  assert.doesNotMatch(release, /kubectl|kubeconfig|KUBECONFIG|MIGRATION_DATABASE_URL/);
  assert.match(release, /Argo CD is the only deployment writer/);
  assert.match(release, /remote\/argocd\/canonical-cloud\/promote-release\.mjs/);
});
