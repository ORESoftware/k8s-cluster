import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/selenium-server/pom.xml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

const SRC = 'remote/deployments/selenium-server/src/main/java/com/oresoftware/dd/selenium';

const STEP_ACTIONS = [
  'goto',
  'click',
  'fill',
  'select',
  'press',
  'waitForSelector',
  'waitForUrl',
  'waitForTimeout',
  'extractText',
  'extractAttribute',
  'screenshot',
  'evaluate',
] as const;

test('selenium-server is a Java Vert.x API driving Selenium over RemoteWebDriver', async () => {
  const pom = await readRepoFile('remote/deployments/selenium-server/pom.xml');
  const app = await readRepoFile(`${SRC}/App.java`);
  const mainVerticle = await readRepoFile(`${SRC}/MainVerticle.java`);
  const config = await readRepoFile(`${SRC}/Config.java`);
  const auth = await readRepoFile(`${SRC}/Auth.java`);
  const runner = await readRepoFile(`${SRC}/run/ScenarioRunner.java`);
  const runHandler = await readRepoFile(`${SRC}/handlers/RunHandler.java`);
  const dockerfile = await readRepoFile('remote/deployments/selenium-server/Dockerfile');
  const readme = await readRepoFile('remote/deployments/selenium-server/readme.md');

  // Selenium-java is the binding; Vert.x is the HTTP layer; shaded jar is the artifact.
  assert.match(pom, /<artifactId>selenium-java<\/artifactId>/);
  assert.match(pom, /<artifactId>vertx-web<\/artifactId>/);
  assert.match(pom, /<artifactId>vertx-micrometer-metrics<\/artifactId>/);
  assert.match(pom, /maven-shade-plugin/);
  assert.match(pom, /com\.oresoftware\.dd\.selenium\.App/);
  assert.match(pom, /<finalName>dd-selenium-server<\/finalName>/);

  // The blocking WebDriver work is offloaded to a bounded worker executor, not the event loop.
  assert.match(app, /createSharedWorkerExecutor/);
  assert.match(app, /MicrometerMetricsOptions/);

  // Routes are registered at root and mirrored under /selenium/* for the gateway prefix.
  assert.match(mainVerticle, /router\.post\("\/run"\)/);
  assert.match(mainVerticle, /router\.post\("\/selenium\/run"\)/);
  assert.match(mainVerticle, /"\/selenium\/healthz"/);
  assert.match(mainVerticle, /"\/selenium\/metrics"/);
  assert.match(mainVerticle, /"\/selenium\/status"/);
  assert.match(mainVerticle, /"\/selenium\/tools"/);
  assert.match(mainVerticle, /"service", "dd-selenium-server"/);

  // The driver talks to the in-pod Grid over RemoteWebDriver with a hardened Chrome profile.
  assert.match(runner, /import org\.openqa\.selenium\.remote\.RemoteWebDriver;/);
  assert.match(runner, /new RemoteWebDriver\(/);
  assert.match(runner, /ChromeOptions/);
  assert.match(runner, /--no-sandbox/);
  assert.match(runner, /--disable-dev-shm-usage/);
  assert.match(runner, /WebDriverWait/);
  assert.match(runner, /config\.remoteUrl/);
  // Every documented step action must exist in the runner switch.
  for (const action of STEP_ACTIONS) {
    assert.match(runner, new RegExp(`case "${action}":`), `missing step action: ${action}`);
  }
  // evaluate stays opt-in.
  assert.match(runner, /SELENIUM_ALLOW_EVALUATE=true/);

  // Config / security model.
  assert.match(config, /readInt\("HTTP_PORT", 8105\)/);
  assert.match(config, /"SELENIUM_REMOTE_URL", "http:\/\/localhost:4444"/);
  assert.match(config, /SELENIUM_MAX_CONCURRENT/);
  assert.match(config, /SELENIUM_MAX_STEPS/);
  assert.match(config, /SELENIUM_MAX_TIMEOUT_MS/);
  assert.match(config, /readBool\("SELENIUM_ALLOW_EVALUATE", false\)/);
  assert.match(config, /SERVER_AUTH_SECRET/);

  assert.match(auth, /x-server-auth/);
  assert.match(auth, /MessageDigest\.isEqual/);
  assert.match(auth, /Bearer/);

  // Concurrency cap returns 429 instead of silently queueing.
  assert.match(runHandler, /tryAcquire\(\)/);
  assert.match(runHandler, /setStatusCode\(429\)/);

  assert.match(dockerfile, /eclipse-temurin:17-jre/);
  assert.match(dockerfile, /mvn -B -e -DskipTests package/);
  assert.match(dockerfile, /EXPOSE 8105/);

  assert.match(readme, /dd-selenium-server/);
  assert.match(readme, /Selenium/);
  assert.match(readme, /RemoteWebDriver/);
  assert.match(readme, /standalone-chromium/);
  assert.match(readme, /POST `?\/run`?|`POST \/run`/);
  assert.match(readme, /SERVER_AUTH_SECRET/);
  assert.match(readme, /SELENIUM_ALLOW_EVALUATE/);
  // Documented as the Selenium-only sibling of the multi-driver service.
  assert.match(readme, /dd-browser-test-server/);
});

test('selenium-server deploys as a Grid + Java API pod through Argo and the gateway', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-selenium-server.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-selenium-server.service.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const promtail = await readRepoFile('remote/argocd/observability/promtail.configmap.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-selenium-server/);

  // Two containers: the official Selenium Grid + the self-building maven API.
  assert.match(deployment, /name:\s*selenium\b[\s\S]*image:\s*selenium\/standalone-chromium/);
  assert.match(deployment, /name:\s*selenium-api[\s\S]*image:\s*docker\.io\/library\/maven:3\.9\.9-eclipse-temurin-17/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/selenium-server/);
  assert.match(deployment, /mvn -B -e -DskipTests package/);

  // Grid is pod-internal; API is the public port.
  assert.match(deployment, /containerPort:\s*4444/);
  assert.match(deployment, /containerPort:\s*8105/);
  assert.match(deployment, /name:\s*HTTP_PORT[\s\S]*value:\s*'8105'/);
  assert.match(deployment, /name:\s*SELENIUM_REMOTE_URL[\s\S]*value:\s*http:\/\/localhost:4444/);
  assert.match(deployment, /name:\s*SELENIUM_MAX_CONCURRENT[\s\S]*value:\s*'2'/);
  // Arbitrary script execution must default to off in production manifests.
  assert.match(deployment, /name:\s*SELENIUM_ALLOW_EVALUATE[\s\S]*value:\s*'false'/);
  assert.match(
    deployment,
    /name:\s*SERVER_AUTH_SECRET[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/,
  );

  // Hardening + a shared-memory mount for Chromium.
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /mountPath:\s*\/dev\/shm/);
  assert.match(deployment, /medium:\s*Memory/);

  // The Service exposes only the API; the Grid port must not be published.
  assert.match(service, /name:\s*dd-selenium-server/);
  assert.match(service, /port:\s*8105/);
  assert.match(service, /targetPort:\s*http/);
  assert.doesNotMatch(service, /port:\s*4444/);

  assert.match(kustomization, /dd-selenium-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-selenium-server\.service\.yaml/);

  // Gateway routes share the auth shape used by /scrape and /browser-test.
  assert.match(
    gateway,
    /location = \/selenium[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-selenium-server\.default\.svc\.cluster\.local:8105/,
  );
  assert.match(
    gateway,
    /location \/selenium\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-selenium-server\.default\.svc\.cluster\.local:8105/,
  );

  // Logs flow through promtail's prod selector and the service is documented.
  assert.match(promtail, /dd-selenium-server/);
  assert.match(runtimeReadme, /`dd-selenium-server`/);
  assert.match(runtimeReadme, /RemoteWebDriver/);
});
