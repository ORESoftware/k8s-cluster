import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/fsharp-ws-server/DdFsharpWsServer.fsproj'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('fsharp websocket service is bounded, deployed, and guarded', async () => {
  const project = await readRepoFile('remote/fsharp-ws-server/DdFsharpWsServer.fsproj');
  const routes = await readRepoFile('remote/fsharp-ws-server/WsRoutes.fs');
  const dockerfile = await readRepoFile('remote/fsharp-ws-server/Dockerfile');
  const readme = await readRepoFile('remote/fsharp-ws-server/readme.md');
  const app = await readRepoFile('remote/argocd/apps/dd-fsharp-ws-server.application.yaml');
  const kustomization = await readRepoFile('remote/fsharp-ws-server/k8s/ec2/kustomization.yaml');
  const deployment = await readRepoFile(
    'remote/fsharp-ws-server/k8s/ec2/dd-fsharp-ws-server.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/fsharp-ws-server/k8s/ec2/dd-fsharp-ws-server.service.yaml',
  );

  assert.match(project, /<AssemblyName>dd-fsharp-ws-server<\/AssemblyName>/);
  assert.match(project, /<PackageReference Include="System\.Reactive" Version="6\.0\.1" \/>/);
  assert.match(dockerfile, /mcr\.microsoft\.com\/dotnet\/sdk:10\.0/);
  assert.match(dockerfile, /USER 10001:10001/);

  assert.match(routes, /MAX_WS_TEXT_FRAME_BYTES/);
  assert.match(routes, /MAX_BENCHMARK_ITERATIONS/);
  assert.match(routes, /JsonSerializer\.Serialize/);
  assert.match(routes, /MessageTooBig/);
  assert.match(routes, /InvalidMessageType/);
  assert.match(routes, /receiveTextFrame/);
  assert.match(routes, /parseBoundedPositiveIntEnv/);
  assert.match(routes, /handleRxBurst/);
  assert.match(readme, /\/ws\/rx-burst/);
  assert.match(readme, /MAX_WS_TEXT_FRAME_BYTES/);
  assert.match(readme, /MAX_BENCHMARK_ITERATIONS/);

  assert.match(app, /name:\s*dd-fsharp-ws-server/);
  assert.match(app, /repoURL:\s*git@github\.com:ORESoftware\/k8s-cluster\.git/);
  assert.match(app, /targetRevision:\s*dev/);
  assert.match(app, /path:\s*remote\/fsharp-ws-server\/k8s\/ec2/);
  assert.match(kustomization, /dd-fsharp-ws-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-fsharp-ws-server\.service\.yaml/);

  assert.match(deployment, /name:\s*dd-fsharp-ws-server/);
  assert.match(deployment, /mcr\.microsoft\.com\/dotnet\/sdk:10\.0/);
  assert.match(deployment, /\/bin\/bash[\s\S]*-lc/);
  assert.match(deployment, /dotnet publish DdFsharpWsServer\.fsproj/);
  assert.match(deployment, /HTTP_PORT[\s\S]*value:\s*'8087'/);
  assert.match(deployment, /BENCHMARK_ITERATIONS[\s\S]*value:\s*'200'/);
  assert.match(deployment, /MAX_BENCHMARK_ITERATIONS[\s\S]*value:\s*'1000'/);
  assert.match(deployment, /MAX_WS_TEXT_FRAME_BYTES[\s\S]*value:\s*'65536'/);
  assert.match(deployment, /DOTNET_CLI_TELEMETRY_OPTOUT[\s\S]*value:\s*'1'/);
  assert.match(deployment, /DOTNET_NOLOGO[\s\S]*value:\s*'1'/);
  assert.match(deployment, /NUGET_PACKAGES[\s\S]*value:\s*\/tmp\/nuget-packages/);
  assert.match(deployment, /DOTNET_CLI_HOME[\s\S]*value:\s*\/tmp/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /runAsUser:\s*10001/);
  assert.match(deployment, /runAsGroup:\s*10001/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /mountPath:\s*\/tmp[\s\S]*name:\s*tmp/);
  assert.match(deployment, /name:\s*tmp[\s\S]*emptyDir:[\s\S]*sizeLimit:\s*2Gi/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);

  assert.match(service, /name:\s*dd-fsharp-ws-server/);
  assert.match(service, /port:\s*8087/);
  assert.match(service, /targetPort:\s*8087/);
  assert.match(service, /type:\s*ClusterIP/);
});
