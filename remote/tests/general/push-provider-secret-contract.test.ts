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

function envBlock(deployment: string, name: string): string {
  const marker = `- name: ${name}`;
  const start = deployment.indexOf(marker);
  assert.ok(start >= 0, `missing ${name} environment entry`);
  const next = deployment.indexOf("\n            - name:", start + marker.length);
  return deployment.slice(start, next < 0 ? deployment.length : next);
}

test("SendGrid and Twilio credentials are projected only from the provider secret", async () => {
  const runtime = "remote/argocd/dd-next-runtime";
  const [deployment, externalSecret] = await Promise.all([
    readRepoFile(`${runtime}/dd-push-notification-server.deployment.yaml`),
    readRepoFile(`${runtime}/dd-push-notification-server.externalsecret.yaml`),
  ]);

  for (const name of [
    "SENDGRID_API_KEY",
    "TWILIO_ACCOUNT_SID",
    "TWILIO_AUTH_TOKEN",
    "TWILIO_API_KEY_SID",
    "TWILIO_API_KEY_SECRET",
    "TWILIO_MESSAGING_SERVICE_SID",
    "TWILIO_FROM_NUMBER",
  ]) {
    const block = envBlock(deployment, name);
    assert.match(block, /valueFrom:\s*\n\s*secretKeyRef:/);
    assert.match(block, /name:\s*dd-push-notification-server-secrets/);
    assert.match(block, new RegExp(`key:\\s*${name}`));
    assert.doesNotMatch(block, /\n\s*value:\s*\S/);
  }

  assert.match(externalSecret, /target:\s*\n\s*name:\s*dd-push-notification-server-secrets/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/push-notification-server-secrets/);
  assert.match(externalSecret, /exactly mail\.send/);
  assert.match(externalSecret, /Full Access[\s\S]*prohibited/);
  assert.match(externalSecret, /TWILIO_API_KEY_SID[\s\S]*preferred, rotatable/);
  assert.match(externalSecret, /TWILIO_STATUS_CALLBACK_URL[\s\S]*leave absent until/);
});

test("provider policy contains no credential-shaped literals", async () => {
  const runtime = "remote/argocd/dd-next-runtime";
  const [deployment, externalSecret] = await Promise.all([
    readRepoFile(`${runtime}/dd-push-notification-server.deployment.yaml`),
    readRepoFile(`${runtime}/dd-push-notification-server.externalsecret.yaml`),
  ]);
  const source = `${deployment}\n${externalSecret}`;

  assert.doesNotMatch(source, /\bSG\.[A-Za-z0-9_-]{10,}\b/);
  assert.doesNotMatch(source, /\b(?:AC|SK|VA|MG)[0-9a-fA-F]{32}\b/);
  assert.doesNotMatch(source, /TWILIO_(?:AUTH_TOKEN|API_KEY_SECRET)\s*[:=]\s*[A-Za-z0-9._-]{16,}/);
  assert.doesNotMatch(source, /SENDGRID_API_KEY\s*[:=]\s*SG\./);
});
