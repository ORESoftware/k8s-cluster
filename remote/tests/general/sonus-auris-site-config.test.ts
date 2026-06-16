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
const pinnedSiteRevision = "2f94949c6a36533cec3cfe36022f7f9da50648de";

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

function extractLocationBlock(config: string, marker: string): string {
  const start = config.indexOf(marker);
  assert.notEqual(start, -1, `missing gateway location marker ${marker}`);
  const rest = config.slice(start);
  const next = rest.indexOf("\n\n      location ", 1);
  return next === -1 ? rest : rest.slice(0, next);
}

test("sonus auris site is pinned as a public submodule", async () => {
  const gitmodules = await readRepoFile(".gitmodules");
  const packageJson = JSON.parse(
    await readRepoFile("remote/submodules/sonus-auris-site.web/package.json"),
  ) as { scripts?: Record<string, string> };
  const astroConfig = await readRepoFile("remote/submodules/sonus-auris-site.web/astro.config.mjs");

  assert.match(gitmodules, /path = remote\/submodules\/sonus-auris-site\.web/);
  assert.match(gitmodules, /url = https:\/\/github\.com\/sonus-auris\/sonus-auris-site\.web/);
  assert.match(gitmodules, /branch = main/);
  assert.equal(packageJson.scripts?.build, "astro build");
  assert.match(astroConfig, /base:\s*'\/sonus-auris-site\.web'/);
});

test("sonus auris manifests build a cloud-neutral static site service", async () => {
  const kustomization = await readRepoFile("remote/argocd/dd-next-runtime/kustomization.yaml");
  const configMap = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.configmap.yaml",
  );
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.deployment.yaml",
  );
  const service = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
  );

  assert.match(kustomization, /dd-sonus-auris-site\.configmap\.yaml/);
  assert.match(kustomization, /dd-sonus-auris-site\.deployment\.yaml/);
  assert.match(kustomization, /dd-sonus-auris-site\.service\.yaml/);
  assert.match(configMap, /name:\s*dd-sonus-auris-site-nginx/);
  assert.match(configMap, /try_files \$uri \$uri\/ \/index\.html;/);

  assert.match(deployment, /name:\s*dd-sonus-auris-site/);
  assert.match(deployment, /replicas:\s*2/);
  assert.match(deployment, new RegExp(`dd\\.dev/sonus-auris-site-revision:\\s*'${pinnedSiteRevision}'`));
  assert.match(deployment, /image:\s*docker\.io\/library\/node:22-bookworm/);
  assert.match(deployment, /git clone --depth 1 --no-checkout/);
  assert.match(deployment, new RegExp(`value:\\s*${pinnedSiteRevision}`));
  assert.match(deployment, /replaceAll\("\/sonus-auris-site\.web\/", "\/sonus-auris\/"\)/);
  assert.match(deployment, /npm run build -- --base \/sonus-auris/);
  assert.match(deployment, /image:\s*docker\.io\/nginxinc\/nginx-unprivileged:1\.27-alpine/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.doesNotMatch(deployment, /hostPath:/);

  assert.match(service, /name:\s*dd-sonus-auris-site/);
  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /port:\s*8080/);
  assert.match(service, /targetPort:\s*http/);
});

test("gateway exposes sonus auris at /sonus-auris without auth coupling", async () => {
  const gateway = await readRepoFile("remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml");
  const readme = await readRepoFile("remote/argocd/dd-next-runtime/readme.md");
  const exactBlock = extractLocationBlock(gateway, "location = /sonus-auris");
  const prefixBlock = extractLocationBlock(gateway, "location /sonus-auris/");

  for (const block of [exactBlock, prefixBlock]) {
    assert.match(block, /if \(\$request_method !~ \^\(GET\|HEAD\)\$\)/);
    assert.match(block, /proxy_set_header X-Forwarded-Prefix \/sonus-auris;/);
    assert.match(block, /proxy_set_header Auth "";/);
    assert.match(block, /proxy_set_header Cookie "";/);
    assert.match(block, /proxy_pass http:\/\/dd-sonus-auris-site\.default\.svc\.cluster\.local:8080\/;/);
    assert.doesNotMatch(block, /\$dd_gateway_auth_ok/);
  }

  assert.match(readme, /`\/sonus-auris`, `\/sonus-auris\/` -> `dd-sonus-auris-site:8080`/);
});
