import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(
  new URL("../.github/workflows/ops-create-meta-agent-repo-selfhosted.yml", import.meta.url),
  "utf8",
);
const ciWorkflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const publisher = readFileSync(
  new URL("../scripts/ops-create-meta-agent-repo-selfhosted.py", import.meta.url),
  "utf8",
);

test("one-shot publisher has one trusted-login local execution carrier", () => {
  const requiredWorkflowTokens = [
    "github.event.issue.number == 15",
    "github.event.issue.pull_request != null",
    "github.event.comment.user.login == 'ORESoftware'",
    "github.event.comment.author_association == 'OWNER'",
    "github.event.comment.author_association == 'MEMBER'",
    "github.event.comment.body == 'ops-create-meta-agent-repo-selfhosted:2026-08-01'",
    "cancel-in-progress: false",
    "runs-on: canonical-browser",
    "ref: ${{ github.sha }}",
    "repository: ORESoftware/k8s-cluster",
    "ref: 55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
    "scripts/critical-org-fleet/assets",
    "persist-credentials: false",
    "python3 -m py_compile runner/scripts/ops-create-meta-agent-repo-selfhosted.py",
    "python3 runner/scripts/ops-create-meta-agent-repo-selfhosted.py",
  ];
  for (const token of requiredWorkflowTokens) {
    assert.ok(workflow.includes(token), `workflow must retain ${token}`);
  }

  assert.equal((workflow.match(/issue_comment:/g) ?? []).length, 1);
  assert.equal((workflow.match(/persist-credentials: false/g) ?? []).length, 2);
  assert.ok(!workflow.includes("workflow_dispatch:"));
  assert.ok(!workflow.includes("pull_request_target:"));
  assert.ok(!workflow.includes("raw.githubusercontent.com"));
  assert.ok(!workflow.includes("PUBLISHER_SCRIPT_URL"));
  assert.ok(!workflow.includes("pull-requests: write"));
});

test("publisher fails closed around identity, immutable refs, and bounded execution", () => {
  const requiredPublisherTokens = [
    'EXPECTED_LOGIN = "ORESoftware"',
    'TARGET = f"{ORG}/{NAME}"',
    'EXPECTED_MAIN = "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1"',
    'FEATURE_REF = "agent/den-1057-meta-agent-control-plane"',
    'EXPECTED_FEATURE = "789d48039da232faed985d4f8de176959f117e08"',
    'BUNDLE_SHA256 = "1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031"',
    "MAX_BUNDLE_BYTES = 64 * 1024 * 1024",
    "MAX_ENCODED_BUNDLE_BYTES = 90 * 1024 * 1024",
    "MAX_API_BODY_BYTES = 1_048_576",
    "MAX_ERROR_BODY_BYTES = 4_096",
    "MAX_COMMAND_DIAGNOSTIC_CHARS = 3_000",
    'ALLOWED_HTTPS_HOSTS = frozenset({"api.github.com", "github.com"})',
    "class NoRedirect(urllib.request.HTTPRedirectHandler):",
    "urllib.request.ProxyHandler({})",
    'print(f"::add-mask::{token}"',
    'membership.get("role"), membership.get("state")',
    '("admin", "active")',
    'allowed_error_statuses=frozenset({404})',
    'metadata.get("visibility") != "public"',
    'metadata.get("default_branch") != "main"',
    '"GITHUB_REPOSITORY_ADMIN_TOKEN": token',
    '"GIT_CONFIG_NOSYSTEM": "1"',
    '"GIT_CONFIG_GLOBAL": os.devnull',
    '"http.followRedirects=false"',
    '"credential.helper="',
    "repository_metadata(token)",
    "publish_exact_repository(owner_token)",
    "exc.read(MAX_ERROR_BODY_BYTES)",
  ];
  for (const token of requiredPublisherTokens) {
    assert.ok(publisher.includes(token), `publisher must retain ${token}`);
  }

  const mainStart = publisher.indexOf("def main() -> int:");
  const authorize = publisher.indexOf("owner_token = authorize(comment_token)", mainStart);
  const owner = publisher.indexOf("verify_owner(owner_token)", mainStart);
  const publish = publisher.indexOf("publish_exact_repository(owner_token)", mainStart);
  const verify = publisher.indexOf("verify_target(owner_token)", mainStart);
  assert.ok(mainStart >= 0 && authorize > mainStart);
  assert.ok(authorize < owner && owner < publish && publish < verify);

  for (const forbidden of [
    "urllib.request.urlopen",
    "os.environ.copy()",
    "publish_meta_control_plane.py",
    "PUBLISHER_SHA256",
    "HTTP {exc.code}: {body}",
    "--force",
    "force=True",
    "shell=True",
  ]) {
    assert.ok(!publisher.includes(forbidden), `publisher must not contain ${forbidden}`);
  }
});

test("normal CI executes publisher security tests", () => {
  const command = "python3 -m unittest discover -s tests/publisher -p 'test_*.py'";
  assert.ok(ciWorkflow.includes(command), `CI must execute ${command}`);
});
