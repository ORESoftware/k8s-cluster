#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  '..',
);

const workloadRoots = [
  path.join(repoRoot, 'remote', 'argocd'),
  path.join(repoRoot, 'remote', 'deployments'),
];
const exporterConfigPath = path.join(
  repoRoot,
  'remote',
  'argocd',
  'observability',
  'k8s-resource-exporter.configmap.yaml',
);
const exporterDeploymentPath = path.join(
  repoRoot,
  'remote',
  'argocd',
  'observability',
  'k8s-resource-exporter.deployment.yaml',
);
const grafanaDashboardsPath = path.join(
  repoRoot,
  'remote',
  'argocd',
  'observability',
  'grafana.dashboards.configmap.yaml',
);
const webHomeMainPath = path.join(
  repoRoot,
  'remote',
  'deployments',
  'web-home-rs',
  'src',
  'main.rs',
);
const deploymentsDir = path.join(repoRoot, 'remote', 'deployments');

const dependencyManifestNames = new Set([
  'Cargo.toml',
  'package.json',
  'pom.xml',
  'build.gradle',
  'build.gradle.kts',
  'pubspec.yaml',
  'go.mod',
  'gleam.toml',
]);
const sourceFileExtensions = new Set([
  '.cjs',
  '.dart',
  '.erl',
  '.ex',
  '.exs',
  '.fs',
  '.fsx',
  '.gleam',
  '.go',
  '.java',
  '.js',
  '.jsx',
  '.kt',
  '.mjs',
  '.py',
  '.rs',
  '.scala',
  '.ts',
  '.tsx',
]);
const forbiddenAutoInstrumentationPackages = [
  '@opentelemetry/auto-instrumentations-node',
  '@opentelemetry/instrumentation-http',
  '@opentelemetry/instrumentation-fastify',
  '@opentelemetry/instrumentation-express',
  '@opentelemetry/instrumentation-fetch',
  '@opentelemetry/instrumentation-undici',
  '@opentelemetry/instrumentation-xml-http-request',
  'dd-trace',
  'elastic-apm-node',
  'newrelic',
  'require-in-the-middle',
  'shimmer',
];
const forbiddenMonkeyPatchPatterns = [
  { label: 'Module._load replacement', pattern: /\bModule\._load\s*=/ },
  { label: 'require.extensions mutation', pattern: /\brequire\.extensions\s*\[/ },
  {
    label: 'process stdout/stderr replacement',
    pattern: /\bprocess\.(stdout|stderr)\s*=/,
  },
  {
    label: 'process stdout/stderr write replacement',
    pattern: /\bprocess\.(stdout|stderr)\.write\s*=/,
  },
  {
    label: 'console method replacement',
    pattern: /\bconsole\.(debug|error|info|log|warn)\s*=/,
  },
  {
    label: 'global fetch replacement',
    pattern: /\b(global|globalThis|window)\.fetch\s*=/,
  },
  {
    label: 'HTTP client replacement',
    pattern: /\b(http|https)\.(get|request)\s*=/,
  },
];
const ignoredDependencyDirs = new Set([
  '.dart_tool',
  '.git',
  '.gradle',
  '.pnpm-store',
  'build',
  'coverage',
  'dist',
  'node_modules',
  'out',
  'target',
]);

function walk(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(dir, entry.name);
    if (entry.isDirectory() && ignoredDependencyDirs.has(entry.name)) return [];
    if (entry.isDirectory()) return walk(entryPath);
    return [entryPath];
  });
}

function parseMetadata(doc) {
  const metadata = { labels: {} };
  let inMetadata = false;
  let inLabels = false;

  for (const line of doc.split(/\r?\n/)) {
    if (/^metadata:\s*$/.test(line)) {
      inMetadata = true;
      inLabels = false;
      continue;
    }
    if (inMetadata && /^\S/.test(line)) break;
    if (!inMetadata) continue;

    const nameMatch = line.match(/^\s{2}name:\s*['"]?([^'"\n#]+?)['"]?\s*(?:#.*)?$/);
    if (nameMatch) metadata.name = nameMatch[1].trim();

    const namespaceMatch = line.match(
      /^\s{2}namespace:\s*['"]?([^'"\n#]+?)['"]?\s*(?:#.*)?$/,
    );
    if (namespaceMatch) metadata.namespace = namespaceMatch[1].trim();

    if (/^\s{2}labels:\s*$/.test(line)) {
      inLabels = true;
      continue;
    }
    if (inLabels) {
      if (!/^\s{4}/.test(line)) {
        inLabels = false;
        continue;
      }
      const labelMatch = line.match(/^\s{4}([^:]+):\s*['"]?([^'"\n#]+?)['"]?\s*(?:#.*)?$/);
      if (labelMatch) metadata.labels[labelMatch[1].trim()] = labelMatch[2].trim();
    }
  }

  return metadata;
}

function parseWorkloadManifestDocs(file) {
  const text = fs.readFileSync(file, 'utf8');
  const workloads = [];

  for (const doc of text.split(/^---\s*$/m)) {
    const kind = doc.match(/^kind:\s*(Deployment|StatefulSet|DaemonSet)\s*$/m)?.[1];
    if (!kind) continue;

    const metadata = parseMetadata(doc);
    if (!metadata.name) continue;
    const namespace = metadata.namespace ?? 'default';
    const app = metadata.labels.app ?? metadata.labels['app.kubernetes.io/name'] ?? metadata.name;
    workloads.push({
      file,
      kind,
      name: metadata.name,
      namespace,
      app,
    });
  }

  return workloads;
}

function parseWatchedApps(configText) {
  const blockMatch = configText.match(/DEFAULT_WATCH_APPS = \(([\s\S]*?)\n\s*\)/);
  if (!blockMatch) {
    throw new Error('Could not find DEFAULT_WATCH_APPS in k8s resource exporter config.');
  }

  return new Set(
    [...blockMatch[1].matchAll(/"([^"]*)"/g)]
      .flatMap((match) => match[1].split(','))
      .map((value) => value.trim())
      .filter(Boolean),
  );
}

function parseCsvEnvValue(manifestText, name) {
  const match = manifestText.match(new RegExp(`name:\\s*${name}[\\s\\S]*?value:\\s*([^\\n]+)`));
  if (!match) {
    throw new Error(`Could not find ${name} env var in k8s resource exporter deployment.`);
  }
  return new Set(
    match[1]
      .trim()
      .split(',')
      .map((value) => value.trim())
      .filter(Boolean),
  );
}

function configMapLiteral(configMapText, key) {
  const marker = `  ${key}: |\n`;
  const start = configMapText.indexOf(marker);
  if (start === -1) {
    throw new Error(`Could not find ConfigMap literal ${key}.`);
  }

  const lines = [];
  const afterMarker = configMapText.slice(start + marker.length);
  for (const line of afterMarker.split('\n')) {
    if (/^  \S/.test(line)) break;
    lines.push(line.startsWith('    ') ? line.slice(4) : line);
  }
  return lines.join('\n');
}

function dependencyNamesFromManifest(file, text) {
  const basename = path.basename(file);
  if (basename === 'package.json') {
    const parsed = JSON.parse(text);
    return new Set(
      [
        'dependencies',
        'devDependencies',
        'optionalDependencies',
        'peerDependencies',
        'bundledDependencies',
        'bundleDependencies',
      ].flatMap((key) => Object.keys(parsed[key] ?? {})),
    );
  }

  return null;
}

function sourceFilesUnder(dir) {
  return walk(dir).filter(
    (file) => sourceFileExtensions.has(path.extname(file)) && !isTestSourceFile(file),
  );
}

function isTestSourceFile(file) {
  const relativeParts = path.relative(deploymentsDir, file).split(path.sep);
  const basename = path.basename(file);
  return (
    relativeParts.includes('test') ||
    relativeParts.includes('tests') ||
    /(?:^|\.)test\.[^.]+$/.test(basename) ||
    /(?:^|\.)spec\.[^.]+$/.test(basename) ||
    /_test\.[^.]+$/.test(basename)
  );
}

const workloadFiles = workloadRoots.flatMap((root) =>
  walk(root).filter((file) => /\.ya?ml$/.test(file)),
);
const workloadsByKey = new Map();
for (const workload of workloadFiles.flatMap(parseWorkloadManifestDocs)) {
  workloadsByKey.set(`${workload.kind}/${workload.namespace}/${workload.name}`, workload);
}
const workloads = [...workloadsByKey.values()].sort((a, b) =>
  `${a.kind}/${a.namespace}/${a.name}`.localeCompare(`${b.kind}/${b.namespace}/${b.name}`),
);

const exporterConfig = fs.readFileSync(exporterConfigPath, 'utf8');
const exporterDeployment = fs.readFileSync(exporterDeploymentPath, 'utf8');
const defaultWatchedApps = parseWatchedApps(exporterConfig);
const deployedWatchedApps = parseCsvEnvValue(exporterDeployment, 'WATCH_APPS');
const deployedWatchedNamespaces = parseCsvEnvValue(exporterDeployment, 'WATCH_NAMESPACES');
const missingConfigWatch = workloads.filter((workload) => !defaultWatchedApps.has(workload.app));
const missingDeploymentWatch = workloads.filter((workload) => !deployedWatchedApps.has(workload.app));
const missingNamespaceWatch = [
  ...new Set(workloads.map((workload) => workload.namespace)),
].filter((namespace) => !deployedWatchedNamespaces.has(namespace));
const dependencyManifests = walk(deploymentsDir).filter((file) =>
  dependencyManifestNames.has(path.basename(file)),
);

const grafanaDashboards = fs.readFileSync(grafanaDashboardsPath, 'utf8');
const webHomeMain = fs.readFileSync(webHomeMainPath, 'utf8');

const failures = [];

if (missingConfigWatch.length > 0) {
  failures.push(
    'Workloads missing from DEFAULT_WATCH_APPS in exporter.py:\n' +
      missingConfigWatch
        .map(
          (workload) =>
            `  - ${workload.namespace}/${workload.kind}/${workload.name} app=${workload.app} (${path.relative(repoRoot, workload.file)})`,
        )
        .join('\n'),
  );
}

if (missingDeploymentWatch.length > 0) {
  failures.push(
    'Workloads missing from WATCH_APPS in k8s-resource-exporter deployment:\n' +
      missingDeploymentWatch
        .map(
          (workload) =>
            `  - ${workload.namespace}/${workload.kind}/${workload.name} app=${workload.app} (${path.relative(repoRoot, workload.file)})`,
        )
        .join('\n'),
  );
}

if (missingNamespaceWatch.length > 0) {
  failures.push(
    'Namespaces missing from WATCH_NAMESPACES in k8s-resource-exporter deployment:\n' +
      missingNamespaceWatch.map((namespace) => `  - ${namespace}`).join('\n'),
  );
}

const forbiddenDependencyHits = [];
for (const manifest of dependencyManifests) {
  const text = fs.readFileSync(manifest, 'utf8');
  const dependencyNames = dependencyNamesFromManifest(manifest, text);
  if (dependencyNames) {
    for (const dependency of forbiddenAutoInstrumentationPackages) {
      if (dependencyNames.has(dependency)) {
        forbiddenDependencyHits.push(
          `  - ${path.relative(repoRoot, manifest)} depends on ${dependency}`,
        );
      }
    }
    continue;
  }

  for (const dependency of forbiddenAutoInstrumentationPackages) {
    if (text.includes(dependency)) {
      forbiddenDependencyHits.push(
        `  - ${path.relative(repoRoot, manifest)} contains ${dependency}`,
      );
    }
  }
}

if (forbiddenDependencyHits.length > 0) {
  failures.push(
    'Forbidden auto-instrumentation or monkey-patching packages found:\n' +
      forbiddenDependencyHits.join('\n'),
  );
}

const forbiddenSourceHits = [];
for (const sourceFile of sourceFilesUnder(deploymentsDir)) {
  const text = fs.readFileSync(sourceFile, 'utf8');
  for (const dependency of forbiddenAutoInstrumentationPackages) {
    if (text.includes(dependency)) {
      forbiddenSourceHits.push(
        `  - ${path.relative(repoRoot, sourceFile)} references ${dependency}`,
      );
    }
  }
  for (const { label, pattern } of forbiddenMonkeyPatchPatterns) {
    if (pattern.test(text)) {
      forbiddenSourceHits.push(`  - ${path.relative(repoRoot, sourceFile)} matches ${label}`);
    }
  }
}

if (forbiddenSourceHits.length > 0) {
  failures.push(
    'Forbidden auto-instrumentation or monkey-patching source patterns found:\n' +
      forbiddenSourceHits.join('\n'),
  );
}

let deploymentDrilldown;
try {
  deploymentDrilldown = JSON.parse(configMapLiteral(grafanaDashboards, 'deployment-drilldown.json'));
} catch (error) {
  failures.push(
    `Could not parse deployment drilldown dashboard in ${path.relative(repoRoot, grafanaDashboardsPath)}: ${error.message}`,
  );
}

if (deploymentDrilldown) {
  const dashboardText = JSON.stringify(deploymentDrilldown);
  const variables = deploymentDrilldown.templating?.list ?? [];
  if (deploymentDrilldown.uid !== 'dd-deployment-drilldown') {
    failures.push('Deployment drilldown dashboard uid is not dd-deployment-drilldown.');
  }
  if (!variables.some((variable) => variable.name === 'deployment')) {
    failures.push('Deployment drilldown dashboard is missing the deployment variable.');
  }
  for (const [label, pattern] of [
    [
      'deployment dashboard workload query',
      /dd_k8s_workload_desired_replicas\{workload=\\?"\$deployment\\?"\}/,
    ],
    ['deployment dashboard Loki query', /\{deployment=\\?"\$deployment\\?"\}/],
    ['deployment dashboard resource limits', /dd_k8s_pod_container_(cpu|memory)_limit/],
  ]) {
    if (!pattern.test(dashboardText)) {
      failures.push(`Missing ${label} in deployment drilldown dashboard.`);
    }
  }
}

let observabilityControlPlane;
try {
  observabilityControlPlane = JSON.parse(
    configMapLiteral(grafanaDashboards, 'observability-control-plane.json'),
  );
} catch (error) {
  failures.push(
    `Could not parse observability control-plane dashboard in ${path.relative(repoRoot, grafanaDashboardsPath)}: ${error.message}`,
  );
}

if (observabilityControlPlane) {
  const dashboardText = JSON.stringify(observabilityControlPlane);
  if (observabilityControlPlane.uid !== 'dd-observability-control-plane') {
    failures.push('Observability control-plane dashboard uid is not dd-observability-control-plane.');
  }
  for (const [label, pattern] of [
    ['observability target ratio', /dd:observability:target_up_ratio/],
    ['collector self metrics job', /otel-collector-self/],
    ['Loki and Promtail log flow', /promtail_read_lines_total/],
    ['collector refused telemetry flow', /otelcol_receiver_refused_/],
    ['observability namespace logs', /\{namespace=\\?"observability\\?"\}/],
  ]) {
    if (!pattern.test(dashboardText)) {
      failures.push(`Missing ${label} in observability control-plane dashboard.`);
    }
  }
}

for (const [label, pattern] of [
  ['web-home Grafana redirect route', /\.route\(\s*"\/grafana\/depl\/\{deployment\}"/],
  ['web-home Grafana dashboard target', /dd-deployment-drilldown/],
  ['web-home observability Grafana route', /\.route\(\s*"\/grafana\/observability"/],
  ['web-home observability Grafana target', /dd-observability-control-plane/],
]) {
  if (!pattern.test(webHomeMain)) {
    failures.push(`Missing ${label} in ${path.relative(repoRoot, webHomeMainPath)}.`);
  }
}

if (failures.length > 0) {
  console.error(failures.join('\n\n'));
  process.exit(1);
}

console.log(
  `observability coverage ok: ${workloads.length} checked-in workloads are watched, deployment Grafana routing is provisioned, ${dependencyManifests.length} dependency manifests avoid forbidden auto-instrumentation packages, and source files avoid common monkey-patching patterns.`,
);
