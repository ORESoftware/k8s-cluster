import assert from "node:assert/strict";
import { access, readdir, readFile } from "node:fs/promises";

const root = new URL("../", import.meta.url);
const workflowsDirectory = new URL(".github/workflows/", root);
const allowedReadOnlyReusableWorkflows = new Set([
  "canonical-cloud/canonical.cloud/.github/workflows/agents-hierarchy.yml@202c89a988a9adaa43f5113d9d0d1d009bf60e3b",
]);

const publisherSignals = [
  ["write-all permissions", /\bpermissions\s*:\s*["']?write-all["']?/i],
  [
    "contents write permission",
    /(?:^\s*|[{,]\s*)["']?contents["']?\s*:\s*["']?write["']?/im,
  ],
  [
    "package write permission",
    /(?:^\s*|[{,]\s*)["']?packages["']?\s*:\s*["']?write["']?/im,
  ],
  [
    "attestation write permission",
    /(?:^\s*|[{,]\s*)["']?attestations["']?\s*:\s*["']?write["']?/im,
  ],
  [
    "OIDC write permission",
    /(?:^\s*|[{,]\s*)["']?id-token["']?\s*:\s*["']?write["']?/im,
  ],
  ["secret-backed credential", /\$\{\{\s*secrets(?:\.|\[)/i],
  [
    "inherited reusable-workflow secrets",
    /^\s*secrets\s*:\s*["']?inherit["']?\s*$/im,
  ],
  ["Docker registry login action", /\buses\s*:\s*["']?docker\/login-action@/i],
  [
    "Docker publishing action",
    /\buses\s*:\s*["']?docker\/build-push-action@/i,
  ],
  [
    "registry attestation upload",
    /^\s*push-to-registry\s*:\s*["']?true["']?\s*(?:#.*)?$/im,
  ],
  [
    "publishing action input",
    /^\s*push\s*:\s*["']?true["']?\s*(?:#.*)?$/im,
  ],
  ["Docker push command", /\bdocker\b[^\r\n]*\b(?:image\s+)?push\b/i],
  ["Docker registry login command", /\bdocker\b[^\r\n]*\blogin\b/i],
  ["Docker build push flag", /\bdocker\b[^\r\n]*\s--push(?:[=\s]|$)/i],
  ["OCI publishing tool", /\b(?:oras|crane|skopeo)\b/i],
  [
    "alternate container push command",
    /\b(?:podman|buildah|nerdctl)\b[^\r\n]*\bpush\b/i,
  ],
  ["language package publishing command", /\b(?:npm|cargo)\s+publish\b/i],
  ["Python package upload command", /\btwine\s+upload\b/i],
  ["inbound reusable workflow", /^\s*["']?workflow_call["']?\s*:/im],
];

function executableWorkflowText(workflow) {
  return workflow.replace(/^\s*#.*$/gm, "");
}

function hasReadOnlyTopLevelPermissions(workflow) {
  const lines = workflow.split(/\r?\n/);
  const permissionLines = lines
    .map((line, index) => [line, index])
    .filter(([line]) => /^permissions:\s*(?:#.*)?$/.test(line));
  if (permissionLines.length !== 1) return false;

  const [, start] = permissionLines[0];
  const entries = [];
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === "" || /^\s*#/.test(line)) continue;
    if (!/^\s/.test(line)) break;
    entries.push(line.trim().replace(/\s+#.*$/, ""));
  }
  return entries.length === 1 && entries[0] === "contents: read";
}

function outboundReusableWorkflowViolations(workflow) {
  const pattern =
    /^\s*uses\s*:\s*["']?([^\s"'#]+\.github\/workflows\/[^\s"'#]+@[^\s"'#]+)["']?\s*(?:#.*)?$/gim;
  return [...workflow.matchAll(pattern)]
    .map((match) => match[1])
    .filter((target) => !allowedReadOnlyReusableWorkflows.has(target))
    .map(() => "outbound reusable workflow");
}

function workflowViolations(workflow) {
  const executable = executableWorkflowText(workflow);
  const violations = publisherSignals
    .filter(([, pattern]) => pattern.test(executable))
    .map(([description]) => description);
  violations.push(...outboundReusableWorkflowViolations(executable));
  if (!hasReadOnlyTopLevelPermissions(workflow)) {
    violations.push("top-level permissions are not exactly contents: read");
  }
  return violations;
}

const workflowNames = (await readdir(workflowsDirectory))
  .filter((name) => /\.ya?ml$/i.test(name))
  .sort();
assert.ok(workflowNames.length > 0, "expected GitHub Actions workflows");

const violations = [];
for (const name of workflowNames) {
  const workflow = await readFile(new URL(name, workflowsDirectory), "utf8");
  for (const violation of workflowViolations(workflow)) {
    violations.push(`${name}: ${violation}`);
  }
}

await assert.rejects(
  access(new URL("release/current-containers.env", root)),
  /ENOENT/,
  "application repositories must not declare deployable releases",
);

assert.deepEqual(
  violations,
  [],
  `canonical-monorepo is the sole release publisher:\n${violations.join("\n")}`,
);

const safePreamble = "permissions:\n  contents: read\n";
const safeValidationWorkflow = `${safePreamble}jobs:\n  validate:\n    uses: canonical-cloud/canonical.cloud/.github/workflows/agents-hierarchy.yml@202c89a988a9adaa43f5113d9d0d1d009bf60e3b`;
assert.deepEqual(
  workflowViolations(safeValidationWorkflow),
  [],
  "the immutable read-only hierarchy validator is not a release publisher",
);

const adversarialFixtures = [
  ["write-all permissions", "permissions: write-all"],
  [
    "contents write permission",
    "jobs:\n  release:\n    permissions: { contents: write }",
  ],
  [
    "package write permission",
    "jobs:\n  release:\n    permissions:\n      packages: write",
  ],
  [
    "attestation write permission",
    "jobs:\n  release:\n    permissions: { attestations: write }",
  ],
  [
    "OIDC write permission",
    "jobs:\n  release:\n    permissions:\n      id-token: write",
  ],
  ["secret-backed credential", "env:\n  TOKEN: ${{ secrets.REGISTRY_TOKEN }}"],
  [
    "inherited reusable-workflow secrets",
    "jobs:\n  release:\n    secrets: inherit",
  ],
  [
    "Docker registry login action",
    "steps:\n  - uses: docker/login-action@deadbeef",
  ],
  [
    "Docker publishing action",
    "steps:\n  - uses: docker/build-push-action@deadbeef",
  ],
  ["registry attestation upload", "with:\n  push-to-registry: true"],
  ["publishing action input", "with:\n  push: true"],
  [
    "Docker push command",
    "run: docker --config /tmp/config image push ghcr.io/x/app:sha",
  ],
  [
    "Docker registry login command",
    "run: docker --config /tmp/config login ghcr.io",
  ],
  [
    "Docker build push flag",
    "run: docker buildx build --push -t ghcr.io/x/app:sha .",
  ],
  ["OCI publishing tool", "run: crane append --base source --new_layer layer"],
  [
    "alternate container push command",
    "run: buildah push app ghcr.io/x/app:sha",
  ],
  [
    "outbound reusable workflow",
    "jobs:\n  release:\n    uses: example/publisher/.github/workflows/release.yml@main",
  ],
  ["inbound reusable workflow", "on:\n  workflow_call:"],
];

for (const [expected, fixture] of adversarialFixtures) {
  const workflow = fixture.startsWith("permissions:")
    ? fixture
    : `${safePreamble}${fixture}`;
  assert.ok(
    workflowViolations(workflow).includes(expected),
    `expected ${expected} for:\n${fixture}`,
  );
}
