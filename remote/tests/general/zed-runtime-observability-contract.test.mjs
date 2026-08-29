import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

async function read(path) {
  return readFile(new URL(`../../../${path}`, import.meta.url), 'utf8');
}

for (const cloud of ['aws', 'hetzner']) {
  test(`${cloud} installs the canonical staged Zed bootstrap chain`, async () => {
    const kustomization = await read(`remote/argocd/clusters/${cloud}/kustomization.yaml`);
    const bootstrap = await read(`remote/argocd/clusters/${cloud}/zed.bootstrap.application.yaml`);

    assert.match(kustomization, /- zed\.bootstrap\.application\.yaml/);
    for (const [name, wave] of [
      ['zed-tenant-bootstrap', '-40'],
      ['zed-project-bootstrap', '-30'],
      ['zed-postgres-bootstrap', '-20'],
      ['zed-applications-bootstrap', '-10'],
    ]) {
      assert.match(bootstrap, new RegExp(`name:\\s*${name}[\\s\\S]*?sync-wave:\\s*"${wave}"`));
    }
    assert.equal((bootstrap.match(/targetRevision:\s*main/g) ?? []).length, 4);
    assert.equal((bootstrap.match(/ServerSideApply=true/g) ?? []).length, 4);
    assert.doesNotMatch(bootstrap, /targetRevision:\s*dev/);
  });
}

test('AWS and Hetzner use the same Zed bootstrap definition', async () => {
  const aws = await read('remote/argocd/clusters/aws/zed.bootstrap.application.yaml');
  const hetzner = await read('remote/argocd/clusters/hetzner/zed.bootstrap.application.yaml');
  assert.equal(aws, hetzner);
});

test('the disposable Zed database satisfies the merged Rust registry migration', async () => {
  const postgres = await read('remote/argocd/projects/zed.bootstrap-postgres.yaml');

  assert.match(postgres, /image:\s*pgvector\/pgvector:0\.8\.2-pg17-bookworm/);
  assert.doesNotMatch(postgres, /image:\s*postgres:/);
  assert.doesNotMatch(postgres, /:latest\b/);
  assert.match(postgres, /automountServiceAccountToken:\s*false/);
  assert.match(postgres, /runAsNonRoot:\s*true/);
  assert.match(postgres, /allowPrivilegeEscalation:\s*false/);
  assert.match(postgres, /capabilities:\s*\{\s*drop:\s*\[ALL\]\s*\}/);
  assert.match(postgres, /readinessProbe:[\s\S]*pg_isready/);
  assert.match(postgres, /livenessProbe:[\s\S]*pg_isready/);
  assert.match(postgres, /emptyDir:[\s\S]*medium:\s*Memory[\s\S]*sizeLimit:\s*512Mi/);
  assert.match(postgres, /values:\s*\[dd-zed-api-server, dd-zed-web-server\]/);
});

test('OTEL collector remains the bounded bridge to Tempo Loki and Prometheus', async () => {
  const collector = await read('remote/argocd/observability/otel-collector.configmap.yaml');
  const stack = await read('remote/argocd/observability/kustomization.yaml');

  assert.match(collector, /0\.0\.0\.0:4317/);
  assert.match(collector, /0\.0\.0\.0:4318/);
  assert.match(collector, /memory_limiter:/);
  assert.match(collector, /batch:/);
  assert.match(collector, /dd-tempo\.observability\.svc\.cluster\.local:4317/);
  assert.match(collector, /dd-loki\.observability\.svc\.cluster\.local:3100\/otlp/);
  assert.match(collector, /sending_queue:[\s\S]*enabled:\s*true/);
  assert.match(collector, /retry_on_failure:[\s\S]*enabled:\s*true/);
  assert.match(collector, /spanmetrics:/);
  assert.match(collector, /exporters:[\s\S]*prometheus/);

  for (const resource of [
    'otel-collector.deployment.yaml',
    'prometheus.deployment.yaml',
    'grafana.deployment.yaml',
    'loki.deployment.yaml',
    'tempo.deployment.yaml',
  ]) {
    assert.ok(stack.includes(`- ${resource}`), `${resource} missing from observability kustomization`);
  }
});
