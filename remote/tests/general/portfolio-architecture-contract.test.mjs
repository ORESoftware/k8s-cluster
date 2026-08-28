import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const contractPath = resolve(repoRoot, "docs/architecture/portfolio-contract.json");
const catalogPath = resolve(repoRoot, "docs/architecture/portfolio-organizations.json");
const contract = JSON.parse(await readFile(contractPath, "utf8"));
const catalog = JSON.parse(await readFile(catalogPath, "utf8"));

const expectedOrganizations = [
  "3FA-app",
  "ORESoftware",
  "OmniBlitz",
  "StreemPilot",
  "agent-pontifex",
  "akrion-sim",
  "anticaptrad",
  "apostille-me",
  "athlet-o",
  "benefactor-cc",
  "canonical-cloud",
  "channelsiege",
  "claritas-viz",
  "cliptown",
  "daedalus-fab",
  "declarative-migrations",
  "discrete-event-systems",
  "drone-mngr",
  "embedded-alerts",
  "evento-globolo",
  "fanwaave",
  "fiducia-cloud",
  "fifa-math",
  "file-tunnel",
  "gha-indie-worker",
  "hacker-house-medellin",
  "hypeblitz",
  "hypesiege",
  "memebank",
  "messaging-intel",
  "meta-agents-demo",
  "networking-components",
  "opto-sync",
  "quaestor-ledger",
  "rust-ssr-demos",
  "sagitta-stack",
  "scintilla-run",
  "shared-auth",
  "sonus-auris",
  "streamkore",
  "unreal-unity-poc",
  "usa-acc",
  "voxletra",
  "zed-pkg",
  "zed-pkg-test"
].sort();

test("catalog covers every connected GitHub installation exactly once", () => {
  const names = catalog.organizations.map((organization) => organization.name);
  assert.equal(new Set(names).size, names.length, "organization names must be unique");
  assert.equal(catalog.organization_count, names.length);
  assert.equal(catalog.organization_count, 45);
  assert.deepEqual([...names].sort(), expectedOrganizations);
});

test("the original inline classifications remain represented in the live catalog", () => {
  const liveByName = new Map(
    catalog.organizations.map((organization) => [organization.name, organization])
  );

  for (const organization of contract.organizations) {
    const live = liveByName.get(organization.name);
    assert.ok(live, `inline organization ${organization.name} is missing from the live catalog`);
    assert.equal(live.class, organization.class, `class changed for ${organization.name}`);
    assert.equal(
      live.lifecycle,
      organization.lifecycle,
      `lifecycle changed for ${organization.name}`
    );
  }
});

test("organization classes cannot silently become production dependencies", () => {
  const validClasses = new Set(["platform", "product", "research", "test-fixture", "reserved"]);
  const validLifecycles = new Set(["active", "incubating", "research", "test", "reserved"]);

  for (const organization of catalog.organizations) {
    assert.ok(validClasses.has(organization.class), `invalid class for ${organization.name}`);
    assert.ok(
      validLifecycles.has(organization.lifecycle),
      `invalid lifecycle for ${organization.name}`
    );

    if (["reserved", "test-fixture"].includes(organization.class)) {
      assert.equal(
        organization.runtime_dependency_allowed,
        false,
        `${organization.name} must not be a runtime dependency`
      );
    }

    if (["reserved", "test-fixture", "research"].includes(organization.class)) {
      assert.equal(
        organization.production_dependency_allowed,
        false,
        `${organization.name} must not be a production dependency`
      );
    }

    if (organization.repository_visibility?.startsWith("no repositories visible")) {
      assert.equal(
        organization.production_dependency_allowed,
        false,
        `${organization.name} has no visible release authority`
      );
    }
  }
});

test("new agent and CI platforms have explicit non-overlapping boundaries", () => {
  const byName = new Map(
    catalog.organizations.map((organization) => [organization.name, organization])
  );
  const pontifex = byName.get("agent-pontifex");
  const gha = byName.get("gha-indie-worker");

  assert.equal(pontifex.class, "platform");
  assert.ok(pontifex.platform_boundary.owns.includes("model routing and budget enforcement"));
  assert.ok(
    pontifex.platform_boundary.calls.some((entry) => entry.includes("gha-indie-worker"))
  );
  assert.ok(
    pontifex.platform_boundary.does_not_own.includes("GitHub Actions workflow semantics")
  );

  assert.equal(gha.class, "platform");
  assert.ok(gha.platform_boundary.owns.includes("GitHub Actions-compatible worker execution"));
  assert.ok(gha.platform_boundary.calls.some((entry) => entry.includes("k8s-cluster")));
  assert.ok(gha.platform_boundary.does_not_own.includes("general coding-agent model routing"));
  assert.ok(gha.platform_boundary.does_not_own.includes("arbitrary caller-selected shell execution"));
});

test("repository roles encode one-way interfaces, clients, services, workers, and MCP", () => {
  const requiredRoles = [
    "interfaces",
    "clients",
    "api-server",
    "web-server",
    "worker",
    "control-plane",
    "sync",
    "mcp-server",
    "infra",
    "monorepo",
    "e2e"
  ];

  assert.deepEqual(Object.keys(contract.repository_roles).sort(), requiredRoles.sort());
  assert.deepEqual(contract.repository_roles.interfaces.may_depend_on, []);
  assert.ok(contract.repository_roles.clients.may_depend_on.includes("interfaces"));
  assert.ok(contract.repository_roles["web-server"].may_depend_on.includes("clients"));
  assert.ok(
    contract.repository_roles["mcp-server"].must_not.includes("read product databases directly")
  );
  assert.equal(Object.hasOwn(contract.repository_roles, "backend"), false);
});

test("data and async integration fail closed", () => {
  assert.equal(contract.call_rules.cross_product_database_access, "forbidden");
  assert.equal(contract.call_rules.mcp_direct_database_access, "forbidden");
  assert.equal(contract.call_rules.production_moving_branch_dependencies, "forbidden");
  assert.match(contract.call_rules.internal_async, /NATS JetStream/);
  assert.match(contract.call_rules.internal_async, /outbox\/inbox/);
  assert.equal(contract.call_rules.direct_supabase.default, "forbidden");

  const envelope = new Set(contract.call_rules.event_envelope_required_fields);
  for (const field of [
    "event_id",
    "event_type",
    "event_version",
    "correlation_id",
    "causation_id",
    "traceparent"
  ]) {
    assert.ok(envelope.has(field), `event envelope must include ${field}`);
  }
});

test("platform ownership has one authority per shared concern", () => {
  const boundaries = contract.platform_boundaries;
  for (const required of [
    "ORESoftware/k8s-cluster",
    "shared-auth",
    "fiducia-cloud",
    "scintilla-run",
    "opto-sync",
    "file-tunnel",
    "networking-components",
    "zed-pkg",
    "declarative-migrations",
    "ORESoftware/mcp-rust-libs"
  ]) {
    assert.ok(Object.hasOwn(boundaries, required), `missing platform boundary ${required}`);
  }

  assert.ok(boundaries["shared-auth"].owns.includes("human identity"));
  assert.ok(boundaries["shared-auth"].does_not_own.includes("product authorization"));
  assert.ok(boundaries["fiducia-cloud"].owns.includes("fencing"));
  assert.match(boundaries["fiducia-cloud"].deployment, /dedicated Fiducia multi-cloud clusters/);
  assert.ok(boundaries["scintilla-run"].does_not_own.includes("durable workflow authority"));
  assert.ok(boundaries["opto-sync"].does_not_own.includes("product schemas"));
  assert.ok(boundaries["ORESoftware/k8s-cluster"].owns.includes("durable worker platform"));
});

test("observability propagates context without leaking sensitive or high-cardinality data", () => {
  const requiredAttributes = new Set(contract.observability.required_resource_attributes);
  for (const attribute of [
    "service.name",
    "service.namespace",
    "service.version",
    "deployment.environment",
    "k8s.cluster.name",
    "product.name",
    "component.role",
    "git.commit.sha"
  ]) {
    assert.ok(requiredAttributes.has(attribute), `missing resource attribute ${attribute}`);
  }

  assert.match(contract.observability.trace_propagation, /W3C trace-context/);
  assert.ok(
    contract.observability.high_cardinality_metric_or_log_labels_forbidden.includes("user_id")
  );
  assert.ok(contract.observability.sensitive_payloads_forbidden.includes("raw messages"));
  assert.ok(contract.observability.sensitive_payloads_forbidden.includes("raw audio"));
  assert.match(contract.observability.audit_separation, /append-only audit store/);
});

test("deployment is immutable, GitOps-owned, and guarded for destructive changes", () => {
  assert.match(contract.deployment.artifact_promotion, /build once/);
  assert.match(contract.deployment.artifact_promotion, /immutable digest/);
  assert.ok(contract.deployment.gitops.cluster_repo_owns.includes("registration"));
  assert.ok(contract.deployment.gitops.app_repo_owns.includes("namespace-scoped workloads"));
  assert.match(contract.deployment.database_migrations, /never auto-approved/);
  assert.match(contract.deployment.special_placement.fiducia_raft, /dedicated multi-cloud/);
  assert.match(contract.deployment.special_placement.drone_and_embedded, /device-local safety/);
});
