import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

const workflowsDir = new URL("../.github/workflows/", import.meta.url);
const release = await readFile(
  new URL("release.yml", workflowsDir),
  "utf8",
);
const deployDocs = await readFile(
  new URL("../docs/deploy.md", import.meta.url),
  "utf8",
);
const boundaryDocs = await readFile(
  new URL("../docs/repo-boundaries.md", import.meta.url),
  "utf8",
);
const allowedReadOnlyReusableWorkflows = new Set([
  "canonical-cloud/canonical.cloud/.github/workflows/agents-hierarchy.yml@202c89a988a9adaa43f5113d9d0d1d009bf60e3b",
]);

const applicationPublisherSignals = [
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

  if (permissionLines.length !== 1) {
    return false;
  }

  const [, start] = permissionLines[0];
  const entries = [];
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === "" || /^\s*#/.test(line)) {
      continue;
    }
    if (!/^\s/.test(line)) {
      break;
    }
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

function applicationPublisherViolations(workflow) {
  const executable = executableWorkflowText(workflow);
  const violations = applicationPublisherSignals
    .filter(([, pattern]) => pattern.test(executable))
    .map(([description]) => description);
  violations.push(...outboundReusableWorkflowViolations(executable));
  return violations;
}

function applicationWorkflowViolations(workflow) {
  const violations = applicationPublisherViolations(workflow);
  if (!hasReadOnlyTopLevelPermissions(workflow)) {
    violations.push("top-level permissions are not exactly contents: read");
  }
  return violations;
}

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
  assert.match(
    release,
    /WEB_IMAGE: ghcr\.io\/canonical-cloud\/canonical-web-server/,
  );
  assert.match(
    release,
    /REVOKER_IMAGE: ghcr\.io\/canonical-cloud\/canonical-session-revoker/,
  );
  assert.match(
    release,
    /\$\{\{ env\.WEB_IMAGE \}\}:\$\{\{ env\.RELEASE_SHA \}\}/,
  );
  assert.match(
    release,
    /\$\{\{ env\.REVOKER_IMAGE \}\}:\$\{\{ env\.RELEASE_SHA \}\}/,
  );
  assert.match(release, /\$\{\{ env\.RELEASE_SHA \}\}/);
  assert.match(release, /provenance: mode=max/g);
  assert.match(release, /sbom: true/g);
  assert.match(release, /attest-build-provenance@[0-9a-f]{40}/g);
  assert.doesNotMatch(release, /:main|:latest/);
});

test("pinned app workflows cannot publish or declare a competing release", async () => {
  const appRoot = new URL("../apps/canonical-web-server.rs/", import.meta.url);
  const appWorkflows = new URL(".github/workflows/", appRoot);
  for (const name of (await readdir(appWorkflows)).filter((entry) =>
    /\.ya?ml$/.test(entry),
  )) {
    const workflow = await readFile(new URL(name, appWorkflows), "utf8");
    assert.deepEqual(
      applicationWorkflowViolations(workflow),
      [],
      `${name} crosses the application/release boundary`,
    );
  }
  await assert.rejects(
    readFile(new URL("release/current-containers.env", appRoot), "utf8"),
    /ENOENT/,
  );
});

test("application release boundary rejects known publication escape hatches", () => {
  const safePreamble = "permissions:\n  contents: read\n";
  const safeValidationWorkflow = `${safePreamble}jobs:\n  validate:\n    uses: canonical-cloud/canonical.cloud/.github/workflows/agents-hierarchy.yml@202c89a988a9adaa43f5113d9d0d1d009bf60e3b`;
  assert.deepEqual(
    applicationWorkflowViolations(safeValidationWorkflow),
    [],
    "immutable read-only hierarchy validation must not be treated as publishing",
  );

  const fixtures = [
    ["write-all permissions", "permissions: write-all"],
    [
      "contents write permission",
      "jobs:\n  release:\n    permissions: { contents: 'write' }",
    ],
    [
      "package write permission",
      'jobs:\n  release:\n    permissions:\n      "packages": "write"',
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
    ["registry attestation upload", "with:\n  push-to-registry: 'true'"],
    ["publishing action input", "with:\n  push: true"],
    ["Docker push command", "run: docker image push ghcr.io/example/app:sha"],
    [
      "Docker push command",
      "run: docker --config /tmp/docker-config image push ghcr.io/example/app:sha",
    ],
    [
      "Docker registry login command",
      "run: docker --config /tmp/config login ghcr.io",
    ],
    [
      "Docker build push flag",
      "run: docker buildx build --push -t ghcr.io/example/app:sha .",
    ],
    [
      "OCI publishing tool",
      "run: oras cp source.example/app target.example/app",
    ],
    ["OCI publishing tool", "run: crane append --base source --new_layer layer"],
    [
      "OCI publishing tool",
      "run: skopeo sync --src docker --dest docker images target",
    ],
    [
      "alternate container push command",
      "run: buildah push app ghcr.io/example/app:sha",
    ],
    [
      "outbound reusable workflow",
      "jobs:\n  release:\n    uses: example/publisher/.github/workflows/release.yml@main",
    ],
    ["inbound reusable workflow", "on:\n  workflow_call:"],
  ];

  for (const [expected, fixture] of fixtures) {
    const workflow = fixture.startsWith("permissions:")
      ? fixture
      : `${safePreamble}${fixture}`;
    assert.ok(
      applicationWorkflowViolations(workflow).includes(expected),
      `expected ${expected} for:\n${fixture}`,
    );
  }
});

test("release is the only publisher and repository provisioning stays separately bounded", async () => {
  const workflowFiles = (await readdir(workflowsDir)).filter((name) =>
    /\.ya?ml$/.test(name),
  );
  const privilegedWorkflows = [];
  for (const name of workflowFiles) {
    const workflow = await readFile(new URL(name, workflowsDir), "utf8");
    if (applicationPublisherViolations(workflow).length > 0) {
      privilegedWorkflows.push(name);
    }
  }
  assert.deepEqual(privilegedWorkflows.sort(), [
    "provision-canonical-e2e.yml",
    "release.yml",
  ]);

  const provisioning = await readFile(
    new URL("provision-canonical-e2e.yml", workflowsDir),
    "utf8",
  );
  assert.match(provisioning, /^on:\n  workflow_dispatch:/m);
  assert.doesNotMatch(
    provisioning,
    /^  (?:pull_request|push|workflow_call|workflow_run):/m,
  );
  assert.match(provisioning, /^permissions:\n  contents: read$/m);
  assert.match(provisioning, /if: inputs\.mode == 'apply'/);
  assert.match(provisioning, /environment: canonical-repository-provisioning/);
  assert.match(provisioning, /CANONICAL_REPOSITORY_PROVISIONING_APPROVED/);
  assert.match(
    provisioning,
    /canonical-e2e-repository-provisioning-approved/,
  );
  assert.match(
    provisioning,
    /Require the protected-environment approval marker[\s\S]*Create Canonical source provisioning token/,
  );
  assert.match(provisioning, /permission-administration: write/);
  assert.match(provisioning, /permission-contents: write/);
  assert.match(provisioning, /permission-workflows: write/);
  assert.match(provisioning, /--confirm "\$CONFIRMATION"/);
  assert.doesNotMatch(
    provisioning,
    /(?:packages|attestations|id-token): write|docker\/login-action|docker\/build-push-action|push-to-registry|\bdocker\b[^\r\n]*\b(?:image\s+)?push\b|\b(?:oras|crane|skopeo)\b|\b(?:podman|buildah|nerdctl)\b[^\r\n]*\bpush\b/i,
  );

  assert.match(deployDocs, /sole deployable release authority/);
  assert.match(boundaryDocs, /Only this superproject's pinned-stack CI/);
});

test("release has no cluster credential or direct deployment path", () => {
  assert.match(release, /packages: write/);
  assert.match(release, /attestations: write/);
  assert.match(release, /id-token: write/);
  assert.doesNotMatch(
    release,
    /kubectl|kubeconfig|KUBECONFIG|MIGRATION_DATABASE_URL/,
  );
  assert.match(release, /Argo CD is the only deployment writer/);
  assert.match(
    release,
    /remote\/argocd\/canonical-cloud\/promote-release\.mjs/,
  );
});
