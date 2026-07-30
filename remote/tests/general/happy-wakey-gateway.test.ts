import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/dd-next-runtime/kustomization.yaml"))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

test("Happy Wakey gateway is a persistent single-owner runtime", async () => {
  const runtime = "remote/argocd/dd-next-runtime";
  const [kustomization, deployment, pvc, service] = await Promise.all([
    readRepoFile(`${runtime}/kustomization.yaml`),
    readRepoFile(`${runtime}/dd-happy-wakey-gateway.deployment.yaml`),
    readRepoFile(`${runtime}/dd-happy-wakey-gateway.pvc.yaml`),
    readRepoFile(`${runtime}/dd-happy-wakey-gateway.service.yaml`),
  ]);

  for (const resource of [
    "dd-happy-wakey-gateway.deployment.yaml",
    "dd-happy-wakey-gateway.externalsecret.yaml",
    "dd-happy-wakey-gateway.networkpolicy.yaml",
    "dd-happy-wakey-gateway.pvc.yaml",
    "dd-happy-wakey-gateway.service.yaml",
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll(".", "\\.")));
  }
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /strategy:\s*\n\s*type:\s*Recreate/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /claimName:\s*dd-happy-wakey-gateway-state/);
  assert.match(deployment, /SHARED_AUTH_INTROSPECT_SECRET[\s\S]*secretKeyRef:/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.doesNotMatch(deployment, /SENDGRID_API_KEY|TWILIO_AUTH_TOKEN|AUTH_INTROSPECT_SECRET,\s*value:/);
  assert.match(pvc, /storageClassName:\s*dd-block/);
  assert.match(pvc, /accessModes:\s*\n\s*-\s*ReadWriteOnce/);
  assert.match(service, /port:\s*8128/);
});

test("shared-auth and NATS credentials are projected without entering Git", async () => {
  const runtime = "remote/argocd/dd-next-runtime";
  const [secret, policy] = await Promise.all([
    readRepoFile(`${runtime}/dd-happy-wakey-gateway.externalsecret.yaml`),
    readRepoFile(`${runtime}/dd-happy-wakey-gateway.networkpolicy.yaml`),
  ]);

  assert.match(secret, /key:\s*dd\/shared-auth\/provider-credentials/);
  assert.match(secret, /property:\s*AUTH_INTROSPECT_SECRET/);
  assert.doesNotMatch(secret, /secretAccessKey|api[_-]?key|token:\s*[A-Za-z0-9]/i);
  assert.match(policy, /kubernetes\.io\/metadata\.name:\s*shared-auth/);
  assert.match(policy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(policy, /port:\s*8120/);
  assert.match(policy, /port:\s*4222/);
});

test("public gateway exposes authenticated product routes but hides metrics", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );
  const start = gateway.indexOf("location = /happy-wakey/metrics");
  const end = gateway.indexOf("location / {", start);
  assert.ok(start >= 0 && end > start);
  const block = gateway.slice(start, end);

  assert.match(block, /location = \/happy-wakey\/metrics \{\s*return 404;/);
  assert.match(block, /location \/happy-wakey\/ \{/);
  assert.match(block, /\^\(GET\|POST\|PUT\|DELETE\|OPTIONS\)\$/);
  assert.match(block, /proxy_next_upstream off/);
  assert.match(block, /dd-happy-wakey-gateway\.default\.svc\.cluster\.local:8128/);
  assert.doesNotMatch(block, /\$dd_gateway_auth_ok/);
  assert.doesNotMatch(block, /dd-email-sms-contact|dd-nats/);
});

test("gateway and contact service use typed auth and acknowledged fixed-lane delivery", async () => {
  const [cargo, gateway, contact] = await Promise.all([
    readRepoFile("remote/deployments/happy-wakey-gateway-rs/Cargo.toml"),
    readRepoFile("remote/deployments/happy-wakey-gateway-rs/src/lib.rs"),
    readRepoFile("remote/deployments/dd-email-sms-contact-rs/src/main.rs"),
  ]);

  assert.match(cargo, /utoipa-axum\s*=\s*"=0\.2\.0"/);
  assert.match(gateway, /OpenApiRouter/);
  assert.match(gateway, /path = "\/v1\/reminders\/sync"/);
  assert.match(gateway, /security\(\("shared_auth_bearer" = \[\]\)\)/);
  assert.match(gateway, /auth\/introspect/);
  assert.match(gateway, /CONTACT_EMAIL_SEND_SUBJECT: &str = "dd\.remote\.contact\.email\.send"/);
  assert.match(gateway, /\.request\(CONTACT_EMAIL_SEND_SUBJECT,/);
  assert.match(gateway, /result\.idempotency_key\.as_deref\(\)/);
  assert.match(contact, /handle_email_msg\(&s2, &c2, &msg\)/);
  assert.match(contact, /message\.reply\.as_ref\(\)/);
  assert.match(contact, /result\["idempotency_key"\]/);
});
