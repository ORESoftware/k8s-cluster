import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

async function read(path) {
  return readFile(new URL(`../../../${path}`, import.meta.url), 'utf8');
}

const cloudNames = ['aws', 'hetzner'];
const apiRef = '3d2d4c98024e5bf718dc79c2648bd10d7759a31c';
const webRef = '75f5ac4e35e121485af75588ce453f350e0cabbc';

for (const cloud of cloudNames) {
  test(`${cloud} bootstraps its pinned Zed overlays with the shared platform`, async () => {
    const kustomization = await read(`remote/argocd/clusters/${cloud}/kustomization.yaml`);
    const parent = await read(
      `remote/argocd/clusters/${cloud}/zed.bootstrap.application.yaml`,
    );
    const apps = await read(`remote/argocd/apps/zed.${cloud}.applications.yaml`);
    const clusterApps = await read(`remote/argocd/clusters/${cloud}/applications.yaml`);
    const expectedInclude =
      `{projects/zed.tenant.yaml,projects/zed.appproject.yaml,projects/zed.bootstrap-postgres.yaml,apps/zed.${cloud}.applications.yaml}`;

    assert.match(kustomization, /- zed\.bootstrap\.application\.yaml/);
    assert.match(parent, new RegExp(`name:\\s*dd-zed-bootstrap-${cloud}`));
    assert.match(parent, /repoURL:\s*https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git/);
    assert.match(parent, /targetRevision:\s*main/);
    assert.match(parent, /path:\s*remote\/argocd/);
    assert.match(parent, /directory:[\s\S]*recurse:\s*true/);
    assert.ok(parent.includes(`include: '${expectedInclude}'`));
    assert.doesNotMatch(parent, /apps\/zed\.applications\.yaml/);
    assert.match(parent, /automated:[\s\S]*prune:\s*true[\s\S]*selfHeal:\s*true/);
    assert.match(parent, /- ServerSideApply=true/);
    assert.doesNotMatch(parent, /CreateNamespace=true/);

    // Each cluster must render its own overlay. The generic k8s root leaves
    // PUBLIC_* URLs at registry.zpkg.tech and cannot select the right ingress.
    assert.equal((apps.match(new RegExp(`path:\\s*k8s/overlays/${cloud}`, 'g')) ?? []).length, 2);
    assert.doesNotMatch(apps, /path:\s*k8s\s*$/m);
    assert.equal((apps.match(new RegExp(`targetRevision:\\s*${apiRef}`, 'g')) ?? []).length, 1);
    assert.equal((apps.match(new RegExp(`targetRevision:\\s*${webRef}`, 'g')) ?? []).length, 1);
    assert.doesNotMatch(apps, /targetRevision:\s*main/);
    assert.match(
      apps,
      new RegExp(
        `zed-api-server=ghcr\\.io/zed-pkg/zed-api-server:${apiRef}`,
      ),
    );
    assert.match(
      apps,
      new RegExp(
        `zed-web-server=ghcr\\.io/zed-pkg/zed-web-server:${webRef}`,
      ),
    );
    assert.equal((apps.match(/argocd\.argoproj\.io\/sync-wave:\s*"0"/g) ?? []).length, 2);
    assert.match(apps, /name:\s*dd-zed-api-server/);
    assert.match(apps, /repoURL:\s*https:\/\/github\.com\/zed-pkg\/zed-api-server\.rs\.git/);
    assert.match(apps, /name:\s*dd-zed-web-server/);
    assert.match(apps, /repoURL:\s*https:\/\/github\.com\/zed-pkg\/zed-web-server\.rs\.git/);

    // Zed and the telemetry plane must be installed by the same cloud overlay.
    assert.match(clusterApps, /name:\s*dd-observability/);
    assert.match(clusterApps, /path:\s*remote\/argocd\/observability/);
  });
}

test('Zed resources reconcile in dependency order without a mutable shared app bundle', async () => {
  const tenant = await read('remote/argocd/projects/zed.tenant.yaml');
  const project = await read('remote/argocd/projects/zed.appproject.yaml');
  const postgres = await read('remote/argocd/projects/zed.bootstrap-postgres.yaml');

  assert.match(tenant, /argocd\.argoproj\.io\/sync-wave:\s*"-20"/);
  assert.match(project, /argocd\.argoproj\.io\/sync-wave:\s*"-15"/);
  assert.match(postgres, /argocd\.argoproj\.io\/sync-wave:\s*"-10"/);
  assert.match(project, /clusterResourceWhitelist:\s*\[\]/);
  assert.match(project, /namespace:\s*zed/);

  const [awsApps, hetznerApps] = await Promise.all(
    cloudNames.map((cloud) => read(`remote/argocd/apps/zed.${cloud}.applications.yaml`)),
  );
  assert.notEqual(awsApps, hetznerApps);
  assert.match(awsApps, /k8s\/overlays\/aws/);
  assert.doesNotMatch(awsApps, /k8s\/overlays\/hetzner/);
  assert.match(hetznerApps, /k8s\/overlays\/hetzner/);
  assert.doesNotMatch(hetznerApps, /k8s\/overlays\/aws/);
});

test('the disposable Zed database satisfies the real registry migration contract', async () => {
  const postgres = await read('remote/argocd/projects/zed.bootstrap-postgres.yaml');

  assert.match(postgres, /image:\s*pgvector\/pgvector:0\.8\.2-pg17-bookworm/);
  assert.doesNotMatch(postgres, /image:\s*[^\n]*:latest/);
  assert.match(postgres, /automountServiceAccountToken:\s*false/);
  assert.match(postgres, /runAsNonRoot:\s*true/);
  assert.match(postgres, /allowPrivilegeEscalation:\s*false/);
  assert.match(postgres, /capabilities:\s*\{\s*drop:\s*\[ALL\]\s*\}/);
  assert.match(postgres, /readinessProbe:[\s\S]*pg_isready/);
  assert.match(postgres, /livenessProbe:[\s\S]*pg_isready/);
  assert.match(postgres, /emptyDir:[\s\S]*medium:\s*Memory[\s\S]*sizeLimit:\s*512Mi/);
  assert.match(postgres, /values:\s*\[dd-zed-api-server, dd-zed-web-server\]/);
});

test('the shared collector keeps the complete OTEL to Grafana data path bounded', async () => {
  const collector = await read('remote/argocd/observability/otel-collector.configmap.yaml');
  const observability = await read('remote/argocd/observability/kustomization.yaml');

  assert.match(collector, /otlp:[\s\S]*grpc:[\s\S]*0\.0\.0\.0:4317/);
  assert.match(collector, /otlp:[\s\S]*http:[\s\S]*0\.0\.0\.0:4318/);
  assert.match(collector, /memory_limiter:[\s\S]*limit_mib:\s*384[\s\S]*spike_limit_mib:\s*128/);
  assert.match(collector, /processors:[\s\S]*- memory_limiter[\s\S]*- batch/);
  assert.match(collector, /otlp\/tempo:[\s\S]*dd-tempo\.observability\.svc\.cluster\.local:4317/);
  assert.match(collector, /otlphttp\/loki:[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100\/otlp/);
  assert.match(collector, /otlphttp\/loki:[\s\S]*sending_queue:[\s\S]*enabled:\s*true/);
  assert.match(collector, /otlphttp\/loki:[\s\S]*retry_on_failure:[\s\S]*enabled:\s*true/);
  assert.match(collector, /spanmetrics:[\s\S]*namespace:\s*traces_span_metrics/);
  assert.match(collector, /metrics:[\s\S]*exporters:[\s\S]*- prometheus/);
  assert.match(collector, /traces:[\s\S]*exporters:[\s\S]*- otlp\/tempo/);
  assert.match(collector, /logs:[\s\S]*exporters:[\s\S]*- otlphttp\/loki/);

  for (const resource of [
    'otel-collector.deployment.yaml',
    'prometheus.deployment.yaml',
    'grafana.deployment.yaml',
    'loki.deployment.yaml',
    'tempo.deployment.yaml',
    'jaeger.deployment.yaml',
  ]) {
    assert.match(observability, new RegExp(`- ${resource.replaceAll('.', '\\.')}`));
  }
});
