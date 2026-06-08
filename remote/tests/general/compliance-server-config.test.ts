import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/dd-compliance-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust compliance server is modular, deployed, documented, and guarded', async () => {
  const cargo = await readRepoFile('remote/deployments/dd-compliance-rs/Cargo.toml');
  const main = await readRepoFile('remote/deployments/dd-compliance-rs/src/main.rs');
  const routes = await readRepoFile('remote/deployments/dd-compliance-rs/src/routes.rs');
  const audit = await readRepoFile('remote/deployments/dd-compliance-rs/src/audit.rs');
  const diagrams = await readRepoFile('remote/deployments/dd-compliance-rs/src/diagrams.rs');
  const jobs = await readRepoFile('remote/deployments/dd-compliance-rs/src/jobs.rs');
  const reports = await readRepoFile('remote/deployments/dd-compliance-rs/src/reports.rs');
  const standards = await readRepoFile('remote/deployments/dd-compliance-rs/src/standards.rs');
  const readme = await readRepoFile('remote/deployments/dd-compliance-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-compliance-rs.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-compliance-rs.service.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-compliance-rs.networkpolicy.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const resourceExporter = await readRepoFile(
    'remote/argocd/observability/k8s-resource-exporter.configmap.yaml',
  );
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const apiDocsGenerator = await readRepoFile('remote/tools/generate-api-docs.mjs');
  const apiDocs = await readRepoFile('remote/deployments/dd-compliance-rs/generated/api-docs.json');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(cargo, /name = "dd-compliance-rs"/);
  assert.match(cargo, /dd-runtime-config-client\s*=\s*\{\s*path/);
  assert.match(cargo, /base64 = "0\.22"/);
  assert.match(cargo, /reqwest[\s\S]*rustls-tls/);
  assert.match(cargo, /walkdir = "2"/);

  for (const moduleName of [
    'audit',
    'auth',
    'config',
    'diagrams',
    'jobs',
    'metrics',
    'models',
    'observability',
    'reports',
    'routes',
    'standards',
    'util',
  ]) {
    assert.match(main, new RegExp(`mod ${moduleName};`));
  }
  assert.doesNotMatch(main, /struct AuditRequest/);
  assert.match(routes, /\.route\("\/standards", get\(standards\)\)/);
  assert.match(routes, /\.route\("\/controls", get\(controls\)\)/);
  assert.match(routes, /\.route\("\/audits", get\(list_audits\)\.post\(submit_audit\)\)/);
  assert.match(routes, /\.route\("\/audit-sync", post\(audit_sync\)\)/);
  assert.match(routes, /\.route\("\/diagrams\/infrastructure", post\(diagram_infrastructure\)\)/);
  assert.match(routes, /\.route\("\/reports\/system", post\(system_report\)\)/);
  assert.match(routes, /\.route\("\/vulnerability-scan", post\(vulnerability_scan\)\)/);
  assert.match(routes, /include_str!\("\.\.\/generated\/api-docs\.json"\)/);
  assert.match(routes, /jobStoreWritable/);
  assert.match(audit, /COMPLIANCE_ALLOW_EXTERNAL_FETCH=false/);
  assert.match(audit, /COMPLIANCE_ALLOW_REPO_CLONE=false/);
  assert.match(audit, /validate_repo_url/);
  assert.match(audit, /Automated readiness assessment only/);
  assert.match(diagrams, /generate_infrastructure_diagram/);
  assert.match(diagrams, /dd-data-viz-rs/);
  assert.match(diagrams, /Terraform \/ GitOps desired/);
  assert.match(diagrams, /missing in live/);
  assert.match(diagrams, /unexpected live/);
  assert.match(jobs, /JobStore::load/);
  assert.match(jobs, /persist_record/);
  assert.match(jobs, /Semaphore/);
  assert.match(jobs, /job interrupted by service restart/);
  assert.match(jobs, /dd_compliance_jobs_current/);
  assert.match(reports, /generate_system_report/);
  assert.match(reports, /scan_vulnerabilities/);
  assert.match(reports, /markdown_to_pdf/);
  assert.match(reports, /VulnerabilitySeverity::Critical/);
  assert.match(reports, /allowPrivilegeEscalation/);

  for (const standardId of [
    'hipaa',
    'soc-2',
    'fedramp',
    'nist-csf',
    'nist-800-53',
    'gdpr',
    'iso-27001',
    'iso-27017',
    'iso-27018',
    'iso-27701',
    'cis-controls',
    'cyber-essentials',
    'essential-eight',
    'tisax',
    'cmmc',
    'ccpa',
    'cpra',
    'lgpd',
    'pipeda',
    'pdpa-sg',
    'pdpa-th',
    'privacy-act-1988',
    'uk-gdpr',
    'data-protection-act-2018',
    'pci-dss',
    'swift-csp',
    'psd2',
    'sox',
    'glba',
    'basel-iii',
    'hitech',
    'hitrust-csf',
    'fda-21-cfr-part-11',
    'mdr',
    'nis2',
    'fisma',
    'cjis-security-policy',
    'itar',
    'ear',
    'eu-ai-act',
    'iso-42001',
    'nist-ai-rmf',
    'iso-9001',
    'iso-22301',
    'iso-31000',
    'iso-20000',
    'iso-14001',
    'csrd',
    'tcfd',
    'gri',
  ]) {
    assert.match(standards, new RegExp(`id: "${standardId}"`));
  }

  assert.match(readme, /POST \/audits/);
  assert.match(readme, /POST \/audit-sync/);
  assert.match(readme, /POST \/diagrams\/infrastructure/);
  assert.match(readme, /POST \/reports\/system/);
  assert.match(readme, /POST \/vulnerability-scan/);
  assert.match(readme, /durable JSON job records/);
  assert.match(readme, /base64-encoded PDF/);
  assert.match(readme, /dd-data-viz-rs/);
  assert.match(readme, /vulnerability scans/);
  assert.match(readme, /COMPLIANCE_MAX_CONCURRENT_JOBS/);
  assert.match(readme, /SOC 2/);
  assert.match(readme, /EU AI Act/);
  assert.match(readme, /GRI Standards/);

  assert.match(deployment, /name:\s*dd-compliance-rs/);
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /name:\s*prepare-job-data/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/dd-compliance-rs/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8118'/);
  assert.match(deployment, /COMPLIANCE_ALLOW_UNAUTHENTICATED[\s\S]*value:\s*'false'/);
  assert.match(deployment, /COMPLIANCE_ALLOW_EXTERNAL_FETCH[\s\S]*value:\s*'false'/);
  assert.match(deployment, /COMPLIANCE_ALLOW_REPO_CLONE[\s\S]*value:\s*'false'/);
  assert.match(deployment, /COMPLIANCE_MAX_CONCURRENT_JOBS[\s\S]*value:\s*'2'/);
  assert.match(deployment, /COMPLIANCE_DATA_VIZ_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /COMPLIANCE_DATA_VIZ_URL[\s\S]*dd-data-viz-rs\.default\.svc\.cluster\.local:8127/);
  assert.match(deployment, /RUNTIME_CONFIG_SERVICE_NAME[\s\S]*dd-compliance-rs/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*-\s*ALL/);
  assert.match(deployment, /mountPath:\s*\/var\/lib\/dd-compliance-rs/);
  assert.match(deployment, /hostPath:[\s\S]*path:\s*\/var\/lib\/dd-compliance-rs/);
  assert.match(service, /name:\s*dd-compliance-rs/);
  assert.match(service, /port:\s*8118/);
  assert.match(kustomization, /dd-compliance-rs\.deployment\.yaml/);
  assert.match(kustomization, /dd-compliance-rs\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-compliance-rs\.service\.yaml/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /app:\s*dd-compliance-rs/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /app:\s*dd-runtime-config/);
  assert.match(networkPolicy, /app:\s*dd-data-viz-rs/);
  assert.match(networkPolicy, /port:\s*8127/);
  assert.match(networkPolicy, /dd-prometheus/);
  assert.match(networkPolicy, /port:\s*8118/);

  assert.match(gateway, /location = \/compliance[\s\S]*return 302 \/compliance\//);
  assert.match(gateway, /location \/compliance\/[\s\S]*if \(\$dd_gateway_auth_ok = 0\)/);
  assert.match(
    gateway,
    /location \/compliance\/[\s\S]*proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-compliance-rs\.default\.svc\.cluster\.local:8118\//,
  );
  assert.match(prometheus, /job_name:\s*dd-compliance-rs/);
  assert.match(prometheus, /dd-compliance-rs\.default\.svc\.cluster\.local:8118/);
  assert.match(otel, /job_name:\s*dd-compliance-rs/);
  assert.match(otel, /dd-compliance-rs\.default\.svc\.cluster\.local:8118/);
  assert.match(resourceExporter, /dd-compliance-rs/);
  assert.match(home, /dd-compliance-rs:8118/);
  assert.match(home, /\/compliance\/standards/);
  assert.match(home, /\/compliance\/diagrams\/example/);
  assert.match(home, /POST \/compliance\/reports\/system/);
  assert.match(home, /POST \/compliance\/vulnerability-scan/);
  assert.match(home, /Rust compliance readiness server/);
  assert.match(apiDocsGenerator, /\['dd-compliance-rs', 'src\/routes\.rs'\]/);
  assert.match(apiDocs, /"routeCount":\s*(1[0-9]|[2-9][0-9])/);
  assert.match(apiDocs, /"path":\s*"\/audits"/);
  assert.match(apiDocs, /"path":\s*"\/audit-sync"/);
  assert.match(apiDocs, /"path":\s*"\/diagrams\/infrastructure"/);
  assert.match(apiDocs, /"path":\s*"\/reports\/system"/);
  assert.match(apiDocs, /"path":\s*"\/vulnerability-scan"/);
  assert.match(runtimeReadme, /`dd-compliance-rs`/);
  assert.match(runtimeReadme, /durable hostPath-backed job\s+records/);
  assert.match(runtimeReadme, /COMPLIANCE_DATA_VIZ_URL=http:\/\/dd-data-viz-rs\.default\.svc\.cluster\.local:8127/);
  assert.match(runtimeReadme, /base64 PDF output/);
  assert.match(runtimeReadme, /vulnerability scan route/);
  assert.match(runtimeReadme, /\/compliance\/standards/);
});
