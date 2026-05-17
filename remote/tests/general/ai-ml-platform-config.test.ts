import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/ai-ml-pipeline/src/dd_ai_ml_pipeline.py'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('python ai/ml pipeline turns telemetry into MDP-ready feature events', async () => {
  const source = await readRepoFile('remote/ai-ml-pipeline/src/dd_ai_ml_pipeline.py');
  const readme = await readRepoFile('remote/ai-ml-pipeline/readme.md');
  const dockerfile = await readRepoFile('remote/ai-ml-pipeline/Dockerfile');

  assert.match(source, /SERVICE_NAME = "dd-ai-ml-pipeline"/);
  assert.match(source, /OnlineTelemetryModel/);
  assert.match(source, /EWMA|ewma/);
  assert.match(source, /zScore/);
  assert.match(source, /transitionModel/);
  assert.match(source, /dd\.remote\.telemetry\.raw/);
  assert.match(source, /dd\.remote\.ml\.features/);
  assert.match(source, /dd\.remote\.telemetry\.mdp/);
  assert.match(source, /dd\.remote\.events/);
  assert.match(source, /POST \/ingest/);
  assert.match(source, /POST \/analyze/);
  assert.match(source, /POST \/mdp\/features/);
  assert.match(source, /SERVER_AUTH_SECRET is required unless ML_ALLOW_UNAUTHENTICATED=true/);
  assert.match(source, /compare_digest/);
  assert.match(source, /MAX_TELEMETRY_WINDOW_MS/);
  assert.match(source, /MAX_TRACKED_SERIES|ML_MAX_TRACKED_SERIES/);
  assert.match(source, /X-Content-Type-Options/);
  assert.match(source, /nats publish rejected oversize payload/);
  assert.match(source, /dd_ai_ml_pipeline_features_total/);
  assert.match(source, /dd_ai_ml_pipeline_auth_failures_total/);
  assert.match(readme, /Python3 online feature pipeline/);
  assert.match(readme, /dd-mdp-optimizer/);
  assert.match(readme, /MLflow-registered models/);
  assert.match(dockerfile, /python:3\.12-slim/);
  assert.match(dockerfile, /dd_ai_ml_pipeline\.py/);
});

test('ai/ml platform bundle deploys the python pipeline and open-source stack catalog', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-ai-ml-platform.application.yaml');
  const kustomization = await readRepoFile('remote/argocd/ai-ml-platform/kustomization.yaml');
  const namespace = await readRepoFile('remote/argocd/ai-ml-platform/namespace.yaml');
  const catalog = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-tool-catalog.configmap.yaml',
  );
  const requirements = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-python-workflow-requirements.configmap.yaml',
  );
  const externalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-agent-secrets.externalsecret.yaml',
  );
  const serviceAccount = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-serviceaccount.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-pipeline.networkpolicy.yaml',
  );
  const deployment = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-pipeline.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-pipeline.service.yaml',
  );

  assert.match(app, /name:\s*dd-ai-ml-platform/);
  assert.match(app, /path:\s*remote\/argocd\/ai-ml-platform/);
  assert.match(app, /namespace:\s*ai-ml/);
  assert.match(namespace, /name:\s*ai-ml/);
  assert.match(kustomization, /dd-ai-ml-tool-catalog\.configmap\.yaml/);
  assert.match(kustomization, /dd-ai-ml-python-workflow-requirements\.configmap\.yaml/);
  assert.match(kustomization, /dd-ai-ml-agent-secrets\.externalsecret\.yaml/);
  assert.match(kustomization, /dd-ai-ml-serviceaccount\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.deployment\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.service\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.networkpolicy\.yaml/);

  for (const tool of [
    'Dagster',
    'Airflow',
    'MLflow',
    'dbt',
    'Kafka',
    'Spark',
    'Metaflow',
    'LlamaIndex',
    'Qdrant',
    'Airbyte',
  ]) {
    assert.match(catalog, new RegExp(`"name": "${tool}"`));
  }
  assert.match(catalog, /dd-mdp-optimizer/);
  assert.match(requirements, /dbt-core/);
  assert.match(requirements, /metaflow/);
  assert.match(requirements, /llama-index/);

  assert.match(deployment, /name:\s*dd-ai-ml-pipeline/);
  assert.match(deployment, /namespace:\s*ai-ml/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /serviceAccountName:\s*dd-ai-ml-pipeline/);
  assert.match(deployment, /python:3\.12-slim/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/ai-ml-pipeline/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8099'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /ML_ALLOW_UNAUTHENTICATED[\s\S]*value:\s*'false'/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /ML_RAW_TELEMETRY_SUBJECT[\s\S]*dd\.remote\.telemetry\.raw/);
  assert.match(deployment, /ML_FEATURE_SUBJECT[\s\S]*dd\.remote\.ml\.features/);
  assert.match(deployment, /ML_MDP_TELEMETRY_SUBJECT[\s\S]*dd\.remote\.telemetry\.mdp/);
  assert.match(deployment, /ML_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-ai-ml-pipeline/);
  assert.match(service, /namespace:\s*ai-ml/);
  assert.match(service, /port:\s*8099/);
  assert.match(externalSecret, /kind:\s*ExternalSecret/);
  assert.match(externalSecret, /namespace:\s*ai-ml/);
  assert.match(externalSecret, /name:\s*dd-agent-secrets/);
  assert.match(serviceAccount, /automountServiceAccountToken:\s*false/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*default/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(networkPolicy, /port:\s*4222/);
});

test('open-source ai/ml platform tools have Argo CD entries', async () => {
  const dagster = await readRepoFile('remote/argocd/apps/dd-dagster.application.yaml');
  const airflow = await readRepoFile('remote/argocd/apps/dd-airflow.application.yaml');
  const mlflow = await readRepoFile('remote/argocd/apps/dd-mlflow.application.yaml');
  const kafka = await readRepoFile('remote/argocd/apps/dd-kafka-strimzi.application.yaml');
  const spark = await readRepoFile('remote/argocd/apps/dd-spark-operator.application.yaml');
  const qdrant = await readRepoFile('remote/argocd/apps/dd-qdrant.application.yaml');
  const airbyte = await readRepoFile('remote/argocd/apps/dd-airbyte.application.yaml');

  assert.match(dagster, /repoURL:\s*https:\/\/dagster-io\.github\.io\/helm/);
  assert.match(dagster, /chart:\s*dagster/);
  assert.match(dagster, /targetRevision:\s*1\.13\.3/);
  assert.match(airflow, /repoURL:\s*https:\/\/airflow\.apache\.org/);
  assert.match(airflow, /chart:\s*airflow/);
  assert.match(airflow, /targetRevision:\s*1\.21\.0/);
  assert.match(mlflow, /repoURL:\s*https:\/\/community-charts\.github\.io\/helm-charts/);
  assert.match(mlflow, /chart:\s*mlflow/);
  assert.match(mlflow, /targetRevision:\s*1\.8\.1/);
  assert.match(kafka, /repoURL:\s*https:\/\/strimzi\.io\/charts\//);
  assert.match(kafka, /chart:\s*strimzi-kafka-operator/);
  assert.match(kafka, /targetRevision:\s*1\.0\.0/);
  assert.match(spark, /repoURL:\s*https:\/\/apache\.github\.io\/spark-kubernetes-operator/);
  assert.match(spark, /chart:\s*spark-kubernetes-operator/);
  assert.match(spark, /targetRevision:\s*1\.6\.0/);
  assert.match(qdrant, /repoURL:\s*https:\/\/qdrant\.github\.io\/qdrant-helm/);
  assert.match(qdrant, /chart:\s*qdrant/);
  assert.match(qdrant, /targetRevision:\s*1\.17\.1/);
  assert.match(airbyte, /repoURL:\s*https:\/\/airbytehq\.github\.io\/helm-charts/);
  assert.match(airbyte, /chart:\s*airbyte/);
  assert.match(airbyte, /targetRevision:\s*1\.9\.2/);
});

test('gateway, observability, and homepage expose the ai/ml pipeline', async () => {
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');

  assert.match(gateway, /location = \/ml[\s\S]*return 302 \/ml\//);
  assert.match(gateway, /location \/ml\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"/);
  assert.match(gateway, /location \/ml\/[\s\S]*dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099\//);
  assert.match(
    prometheus,
    /job_name:\s*dd-ai-ml-pipeline[\s\S]*dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-ai-ml-pipeline[\s\S]*dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099/,
  );
  assert.match(home, /dd-ai-ml-pipeline/);
  assert.match(home, /Python3 online feature pipeline/);
  assert.match(home, /href="\/ml\/"/);
  assert.match(home, /dd\.remote\.ml\.features/);
  assert.match(runtimeReadme, /AI\/ML feature pipeline/);
  assert.match(
    runtimeReadme,
    /Dagster, Airflow, MLflow, dbt, Kafka through Strimzi, Spark,\s+Metaflow, LlamaIndex, Qdrant, and Airbyte/,
  );
  assert.match(remoteReadme, /argocd\/ai-ml-platform/);
});
