#!/usr/bin/env node

import fs from 'node:fs';

const files = {
  config: 'remote/argocd/observability/k8s-resource-exporter.configmap.yaml',
  deployment: 'remote/argocd/observability/k8s-resource-exporter.deployment.yaml',
  checker: 'remote/tools/check-observability-coverage.mjs',
};

function replaceExactlyOnce(text, before, after, label) {
  if (text.includes(after)) return text;
  const count = text.split(before).length - 1;
  if (count !== 1) {
    throw new Error(`${label}: expected exactly one source match, found ${count}`);
  }
  return text.replace(before, after);
}

function writeChanged(path, next) {
  const current = fs.readFileSync(path, 'utf8');
  if (current === next) return false;
  fs.writeFileSync(path, next);
  return true;
}

let config = fs.readFileSync(files.config, 'utf8');
config = replaceExactlyOnce(
  config,
  'dd-billing-server,dd-browser-job-runner,dd-browser-test-server,dd-build-server,',
  'dd-billing-server,dd-browser-job-runner,dd-browser-mcp-rs,dd-browser-test-server,dd-build-server,',
  'config DEFAULT_WATCH_APPS browser MCP',
);
config = replaceExactlyOnce(
  config,
  'dd-mdp-optimizer,dd-mlflow-postgresql,dd-music-rs,dd-nats,',
  'dd-mdp-optimizer,dd-mlflow-postgresql,dd-music-rs,dd-nats,dd-nats-bridge,',
  'config DEFAULT_WATCH_APPS NATS bridge',
);
config = replaceExactlyOnce(
  config,
  'dd-shared-auth,dd-shared-auth-nats-bridge,dd-zed-api-server,dd-zed-web-server,',
  'dd-shared-auth,dd-shared-auth-nats-bridge,dd-zed-api-server,dd-zed-web-server,zed-postgres,',
  'config DEFAULT_WATCH_APPS Zed Postgres',
);

let deployment = fs.readFileSync(files.deployment, 'utf8');
deployment = replaceExactlyOnce(
  deployment,
  'value: ai-ml,airbyte,anon-proxy,athleto,big-data,canonical-cloud,daedalus,default,fiducia,headlamp,kafka,messaging,observability,presence,scintilla,shared-auth,spark,threefa,vpn\n',
  'value: ai-ml,airbyte,anon-proxy,athleto,big-data,canonical-cloud,daedalus,default,fiducia,headlamp,kafka,messaging,observability,presence,scintilla,shared-auth,spark,threefa,vpn,zed\n',
  'deployment WATCH_NAMESPACES Zed',
);
deployment = replaceExactlyOnce(
  deployment,
  'dd-billing-server,dd-browser-job-runner,dd-browser-test-server,dd-build-server,',
  'dd-billing-server,dd-browser-job-runner,dd-browser-mcp-rs,dd-browser-test-server,dd-build-server,',
  'deployment WATCH_APPS browser MCP',
);
deployment = replaceExactlyOnce(
  deployment,
  'dd-mdp-optimizer,dd-mlflow-postgresql,dd-music-rs,dd-nats,',
  'dd-mdp-optimizer,dd-mlflow-postgresql,dd-music-rs,dd-nats,dd-nats-bridge,',
  'deployment WATCH_APPS NATS bridge',
);
deployment = replaceExactlyOnce(
  deployment,
  'dd-shared-auth,dd-shared-auth-nats-bridge,dd-zed-api-server,dd-zed-web-server,',
  'dd-shared-auth,dd-shared-auth-nats-bridge,dd-zed-api-server,dd-zed-web-server,zed-postgres,',
  'deployment WATCH_APPS Zed Postgres',
);

let checker = fs.readFileSync(files.checker, 'utf8');
checker = replaceExactlyOnce(
  checker,
  `const webHomeMainPath = path.join(\n  repoRoot,\n  'remote',\n  'deployments',\n  'web-home-rs',\n  'src',\n  'main.rs',\n);`,
  `const webHomeMainPath = path.join(\n  repoRoot,\n  'remote',\n  'deployments',\n  'web-home-rs',\n  'src',\n  'main.rs',\n);\nconst webHomeGrafanaPath = path.join(\n  repoRoot,\n  'remote',\n  'deployments',\n  'web-home-rs',\n  'src',\n  'grafana.rs',\n);`,
  'checker web-home Grafana source path',
);
checker = replaceExactlyOnce(
  checker,
  `const webHomeMain = fs.readFileSync(webHomeMainPath, 'utf8');`,
  `const webHomeMain = fs.readFileSync(webHomeMainPath, 'utf8');\nconst webHomeGrafana = fs.readFileSync(webHomeGrafanaPath, 'utf8');\nconst webHomeRoutingSources = \`${'${webHomeMain}'}\\n${'${webHomeGrafana}'}\`;`,
  'checker web-home routing source load',
);
checker = replaceExactlyOnce(
  checker,
  `  if (!pattern.test(webHomeMain)) {\n    failures.push(\`Missing ${'${label}'} in ${'${path.relative(repoRoot, webHomeMainPath)}'}.\`);\n  }`,
  `  if (!pattern.test(webHomeRoutingSources)) {\n    failures.push(\n      \`Missing ${'${label}'} in web-home routing sources (${'${path.relative(repoRoot, webHomeMainPath)}'}, ${'${path.relative(repoRoot, webHomeGrafanaPath)}'}).\`,\n    );\n  }`,
  'checker web-home route and target scan',
);

const changed = [
  writeChanged(files.config, config) && files.config,
  writeChanged(files.deployment, deployment) && files.deployment,
  writeChanged(files.checker, checker) && files.checker,
].filter(Boolean);

console.log(changed.length === 0 ? 'DEN-374 migration already applied.' : `Updated ${changed.join(', ')}`);
