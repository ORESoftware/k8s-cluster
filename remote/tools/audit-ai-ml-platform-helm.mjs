#!/usr/bin/env node
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '../..');
const helmBin = process.env.HELM_BIN ?? 'helm';
const yqBin = process.env.YQ_BIN ?? 'yq';
const chartCacheRoot = resolve(
  repoRoot,
  process.env.AI_ML_PLATFORM_CHART_CACHE ?? 'tmp/ai-ml-platform-helm-audit',
);
const pullCharts = process.argv.includes('--pull') || process.env.AI_ML_PLATFORM_PULL_CHARTS === '1';

const charts = [
  {
    label: 'Airflow',
    appPath: 'remote/argocd/apps/dd-airflow.application.yaml',
    repoURL: 'https://airflow.apache.org',
    chart: 'airflow',
    targetRevision: '1.21.0',
    archiveURL: 'https://archive.apache.org/dist/airflow/helm-chart/1.21.0/airflow-1.21.0.tgz',
  },
  {
    label: 'Airbyte',
    appPath: 'remote/argocd/apps/dd-airbyte.application.yaml',
    repoURL: 'https://airbytehq.github.io/helm-charts',
    chart: 'airbyte',
    targetRevision: '1.9.2',
    allowed: {
      tokenTrue: ['dd-airbyte-cron', 'dd-airbyte-worker', 'dd-airbyte-workload-launcher'],
      secrets: [
        'dd-airbyte-airbyte-secrets[stringData:DATABASE_USER|stringData:DATABASE_PASSWORD|stringData:AWS_ACCESS_KEY_ID|stringData:AWS_SECRET_ACCESS_KEY;hook=pre-install,pre-upgrade]',
      ],
    },
  },
  {
    label: 'Strimzi',
    appPath: 'remote/argocd/apps/dd-kafka-strimzi.application.yaml',
    repoURL: 'https://strimzi.io/charts/',
    chart: 'strimzi-kafka-operator',
    archivePrefix: 'strimzi-kafka-operator-helm-3-chart',
    targetRevision: '1.0.0',
  },
  {
    label: 'Spark',
    appPath: 'remote/argocd/apps/dd-spark-operator.application.yaml',
    repoURL: 'https://apache.github.io/spark-kubernetes-operator',
    chart: 'spark-kubernetes-operator',
    targetRevision: '1.6.0',
    allowed: {
      unboundedEmptyDirs: ['spark-kubernetes-operator/logs-volume'],
    },
  },
  {
    label: 'Dagster',
    appPath: 'remote/argocd/apps/dd-dagster.application.yaml',
    repoURL: 'https://dagster-io.github.io/helm',
    chart: 'dagster',
    targetRevision: '1.13.3',
    allowed: {
      tokenTrue: ['dd-dagster-daemon', 'dd-dagster-dagster-webserver'],
    },
  },
  {
    label: 'MLflow',
    appPath: 'remote/argocd/apps/dd-mlflow.application.yaml',
    repoURL: 'https://community-charts.github.io/helm-charts',
    chart: 'mlflow',
    targetRevision: '1.8.1',
    allowed: {
      writableRoots: ['dd-mlflow/ini-file-initializer=unset'],
      unboundedEmptyDirs: ['dd-mlflow/ini-file'],
      secrets: [
        'dd-mlflow-env-secret[no-data]',
        'dd-mlflow-flask-server-secret-key[data:MLFLOW_FLASK_SERVER_SECRET_KEY;hook=pre-install,pre-upgrade]',
      ],
    },
  },
  {
    label: 'Qdrant',
    appPath: 'remote/argocd/apps/dd-qdrant.application.yaml',
    repoURL: 'https://qdrant.github.io/qdrant-helm',
    chart: 'qdrant',
    targetRevision: '1.17.1',
    allowed: {
      unboundedEmptyDirs: ['dd-qdrant/qdrant-init'],
    },
  },
];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    maxBuffer: 80 * 1024 * 1024,
    ...options,
  });
  if (result.status !== 0) {
    const stderr = result.stderr ? `\n${result.stderr.trim()}` : '';
    throw new Error(`${command} ${args.join(' ')} failed with status ${result.status}${stderr}`);
  }
  return result.stdout;
}

function yq(expr, filePath) {
  return run(yqBin, ['-r', expr, filePath]).trim();
}

function checkTool(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error(`required tool is unavailable: ${command}`);
  }
}

function walkFiles(root) {
  if (!existsSync(root)) {
    return [];
  }
  const entries = readdirSync(root, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const fullPath = join(root, entry.name);
    if (entry.isDirectory()) {
      return walkFiles(fullPath);
    }
    return [fullPath];
  });
}

function findChartArchive(config) {
  const prefix = config.archivePrefix ?? config.chart;
  const expectedName = `${prefix}-${config.targetRevision}.tgz`;
  return walkFiles(chartCacheRoot).find((filePath) => filePath.endsWith(`/${expectedName}`));
}

function pullChart(config) {
  const destination = chartCacheRoot;
  mkdirSync(destination, { recursive: true });
  try {
    run(helmBin, [
      'pull',
      config.chart,
      '--repo',
      config.repoURL,
      '--version',
      config.targetRevision,
      '--destination',
      destination,
    ]);
  } catch (error) {
    if (!config.archiveURL) {
      throw error;
    }
    run(helmBin, ['pull', config.archiveURL, '--destination', destination]);
  }
}

function renderChart(config) {
  let chartArchive = findChartArchive(config);
  if (!chartArchive && pullCharts) {
    pullChart(config);
    chartArchive = findChartArchive(config);
  }
  if (!chartArchive) {
    throw new Error(
      `${config.label}: chart archive not found under ${chartCacheRoot}; set AI_ML_PLATFORM_CHART_CACHE or run with --pull`,
    );
  }

  const appPath = resolve(repoRoot, config.appPath);
  const values = yq('.spec.source.helm.values', appPath);
  const release = yq('.spec.source.helm.releaseName // .metadata.name', appPath);
  const namespace = yq('.spec.destination.namespace // "default"', appPath);
  const scratch = mkdtempSync(join(tmpdir(), 'ai-ml-platform-helm-audit-'));
  const valuesPath = join(scratch, `${release}-values.yaml`);
  writeFileSync(valuesPath, values);

  const yaml = run(helmBin, [
    'template',
    release,
    chartArchive,
    '--namespace',
    namespace,
    '--skip-tests',
    '-f',
    valuesPath,
  ]);
  const json = run(yqBin, ['eval-all', '-o=json', '[.]', '-'], { input: yaml });
  return JSON.parse(json).filter(Boolean);
}

function podSpecFor(doc) {
  if (!doc?.kind) {
    return null;
  }
  if (['Deployment', 'StatefulSet', 'DaemonSet', 'ReplicaSet', 'Job'].includes(doc.kind)) {
    return doc.spec?.template?.spec ?? null;
  }
  if (doc.kind === 'CronJob') {
    return doc.spec?.jobTemplate?.spec?.template?.spec ?? null;
  }
  if (doc.kind === 'Pod') {
    return doc.spec ?? null;
  }
  return null;
}

function auditDocs(docs) {
  const findings = {
    missingResources: [],
    tokenTrue: [],
    dangerous: [],
    writableRoots: [],
    unboundedEmptyDirs: [],
    duplicateMounts: [],
    secrets: [],
  };
  const workloads = docs.map((doc) => [doc, podSpecFor(doc)]).filter(([, spec]) => spec);

  for (const [doc, spec] of workloads) {
    const workload = doc.metadata?.name ?? '<unnamed>';
    if (spec.automountServiceAccountToken === true) {
      findings.tokenTrue.push(workload);
    }
    for (const hostFlag of ['hostNetwork', 'hostPID', 'hostIPC']) {
      if (spec[hostFlag] === true) {
        findings.dangerous.push(`${workload}/${hostFlag}=true`);
      }
    }
    for (const volume of spec.volumes ?? []) {
      if (volume.emptyDir && !Object.hasOwn(volume.emptyDir, 'sizeLimit')) {
        findings.unboundedEmptyDirs.push(`${workload}/${volume.name}`);
      }
    }
    const containers = [
      ...((spec.initContainers ?? []).map((container) => [container, 'init'])),
      ...((spec.containers ?? []).map((container) => [container, 'main'])),
    ];
    for (const [container, phase] of containers) {
      const id = `${workload}/${container.name}`;
      const resources = container.resources ?? {};
      if (!resources.requests?.cpu || !resources.requests?.memory || !resources.limits?.cpu || !resources.limits?.memory) {
        findings.missingResources.push(`${id}(${phase})`);
      }

      const security = container.securityContext ?? {};
      if (security.privileged === true) {
        findings.dangerous.push(`${id}/privileged=true`);
      }
      if (security.allowPrivilegeEscalation === true) {
        findings.dangerous.push(`${id}/allowPrivilegeEscalation=true`);
      }
      if (Array.isArray(security.capabilities?.add) && security.capabilities.add.length > 0) {
        findings.dangerous.push(`${id}/capabilities.add=${security.capabilities.add.join('+')}`);
      }
      if (security.runAsUser === 0) {
        findings.dangerous.push(`${id}/runAsUser=0`);
      }
      if (security.readOnlyRootFilesystem !== true) {
        findings.writableRoots.push(`${id}=${security.readOnlyRootFilesystem === false ? 'false' : 'unset'}`);
      }

      const mountPaths = new Map();
      for (const mount of container.volumeMounts ?? []) {
        if (mountPaths.has(mount.mountPath)) {
          findings.duplicateMounts.push(`${id}/${mount.mountPath}:${mountPaths.get(mount.mountPath)}+${mount.name}`);
        }
        mountPaths.set(mount.mountPath, mount.name);
      }
    }
  }

  findings.secrets = docs
    .filter((doc) => doc.kind === 'Secret')
    .map((doc) => {
      const dataKeys = Object.keys(doc.data ?? {});
      const stringDataKeys = Object.keys(doc.stringData ?? {});
      const hook = doc.metadata?.annotations?.['helm.sh/hook'];
      const keys = [
        ...dataKeys.map((key) => `data:${key}`),
        ...stringDataKeys.map((key) => `stringData:${key}`),
      ].join('|') || 'no-data';
      return `${doc.metadata?.name ?? '<unnamed>'}[${keys}${hook ? `;hook=${hook}` : ''}]`;
    });

  return { findings, workloads: workloads.length };
}

function difference(actual, allowed) {
  const allowedSet = new Set(allowed ?? []);
  return actual.filter((item) => !allowedSet.has(item));
}

function formatList(items) {
  return items.length ? items.join(',') : '-';
}

function assertNoUnexpected(label, findings, allowed = {}) {
  const unexpected = Object.fromEntries(
    Object.entries(findings).map(([key, values]) => [key, difference(values, allowed[key])]),
  );
  const failed = Object.entries(unexpected).filter(([, values]) => values.length > 0);
  if (failed.length === 0) {
    return;
  }
  const details = failed.map(([key, values]) => `  ${key}: ${values.join(', ')}`).join('\n');
  throw new Error(`${label}: unexpected render-audit findings\n${details}`);
}

function main() {
  checkTool(helmBin, ['version', '--short']);
  checkTool(yqBin, ['--version']);

  let failures = 0;
  for (const config of charts) {
    try {
      const docs = renderChart(config);
      const { findings, workloads } = auditDocs(docs);
      assertNoUnexpected(config.label, findings, config.allowed);
      console.log(
        [
          `${config.label}: docs=${docs.length}`,
          `workloads=${workloads}`,
          `missing_resources=${formatList(findings.missingResources)}`,
          `token_true=${formatList(findings.tokenTrue)}`,
          `dangerous=${formatList(findings.dangerous)}`,
          `writable_roots=${formatList(findings.writableRoots)}`,
          `unbounded_emptydir=${formatList(findings.unboundedEmptyDirs)}`,
          `duplicate_mounts=${formatList(findings.duplicateMounts)}`,
          `secrets=${formatList(findings.secrets)}`,
        ].join(' '),
      );
    } catch (error) {
      failures += 1;
      console.error(error instanceof Error ? error.message : String(error));
    }
  }

  if (failures > 0) {
    process.exitCode = 1;
  }
}

main();
