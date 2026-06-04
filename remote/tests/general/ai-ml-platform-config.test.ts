import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py'))) {
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
  const source = await readRepoFile('remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py');
  const readme = await readRepoFile('remote/deployments/ai-ml-pipeline/readme.md');
  const dockerfile = await readRepoFile('remote/deployments/ai-ml-pipeline/Dockerfile');

  assert.match(source, /SERVICE_NAME = "dd-ai-ml-pipeline"/);
  assert.match(source, /OnlineTelemetryModel/);
  assert.match(source, /EWMA|ewma/);
  assert.match(source, /zScore/);
  assert.match(source, /transitionModel/);
  // ai-ml-pipeline now defaults every NATS subject + queue group through
  // the generated `dd_nats_subject_defs` Python module (see
  // remote/libs/nats/subject-defs/schema/ai-ml-platform.schema.json and
  // runtime-events.schema.json). The literal magic strings are gone from
  // the Python source; instead we assert that the source pulls in the
  // generated constants and applies them to the right env-var default.
  assert.match(source, /from dd_nats_subject_defs import \([^)]*ML_DEAD_LETTER_SUBJECT[^)]*\)/);
  assert.match(source, /from dd_nats_subject_defs import \([^)]*ML_FEATURES_SUBJECT[^)]*\)/);
  assert.match(source, /from dd_nats_subject_defs import \([^)]*RUNTIME_EVENTS_SUBJECT[^)]*\)/);
  assert.match(source, /from dd_nats_subject_defs import \([^)]*TELEMETRY_MDP_SUBJECT[^)]*\)/);
  assert.match(source, /from dd_nats_subject_defs import \([^)]*TELEMETRY_RAW_QUEUE_GROUP[^)]*\)/);
  assert.match(source, /from dd_nats_subject_defs import \([^)]*TELEMETRY_RAW_SUBJECT[^)]*\)/);
  assert.match(source, /env_value\("ML_RAW_TELEMETRY_SUBJECT", TELEMETRY_RAW_SUBJECT\)/);
  assert.match(source, /env_value\("ML_QUEUE_GROUP", TELEMETRY_RAW_QUEUE_GROUP\)/);
  assert.match(source, /env_value\("ML_FEATURE_SUBJECT", ML_FEATURES_SUBJECT\)/);
  assert.match(source, /env_value\("ML_MDP_TELEMETRY_SUBJECT", TELEMETRY_MDP_SUBJECT\)/);
  assert.match(source, /env_value\("ML_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT\)/);
  assert.match(source, /env_value\("ML_DEAD_LETTER_SUBJECT", ML_DEAD_LETTER_SUBJECT\)/);
  assert.match(source, /POST \/ingest/);
  assert.match(source, /POST \/analyze/);
  assert.match(source, /POST \/mdp\/features/);
  assert.match(source, /GET \/readyz/);
  assert.match(source, /ML_DEAD_LETTER_SUBJECT/);
  assert.match(source, /wait_for_nats_pong/);
  assert.match(source, /publish_dead_letter/);
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
  const airbyteNamespace = await readRepoFile('remote/argocd/ai-ml-platform/airbyte.namespace.yaml');
  const kafkaNamespace = await readRepoFile('remote/argocd/ai-ml-platform/kafka.namespace.yaml');
  const sparkNamespace = await readRepoFile('remote/argocd/ai-ml-platform/spark.namespace.yaml');
  const catalog = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-tool-catalog.configmap.yaml',
  );
  const requirements = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-python-workflow-requirements.configmap.yaml',
  );
  const dataContracts = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-data-contracts.configmap.yaml',
  );
  const externalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-agent-secrets.externalsecret.yaml',
  );
  const restApiExternalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-rest-api-secrets.externalsecret.yaml',
  );
  const serviceAccount = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-serviceaccount.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-pipeline.networkpolicy.yaml',
  );
  const aiMlBoundary = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-namespace-boundary.networkpolicy.yaml',
  );
  const kafkaBoundary = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-kafka-namespace-boundary.networkpolicy.yaml',
  );
  const sparkBoundary = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-spark-namespace-boundary.networkpolicy.yaml',
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
  assert.match(airbyteNamespace, /name:\s*airbyte/);
  assert.match(kafkaNamespace, /name:\s*kafka/);
  assert.match(sparkNamespace, /name:\s*spark/);
  for (const ns of [namespace, airbyteNamespace, kafkaNamespace, sparkNamespace]) {
    assert.match(ns, /pod-security\.kubernetes\.io\/audit:\s*restricted/);
    assert.match(ns, /pod-security\.kubernetes\.io\/warn:\s*restricted/);
  }
  assert.match(kustomization, /airbyte\.namespace\.yaml/);
  assert.match(kustomization, /kafka\.namespace\.yaml/);
  assert.match(kustomization, /spark\.namespace\.yaml/);
  assert.match(kustomization, /dd-ai-ml-tool-catalog\.configmap\.yaml/);
  assert.match(kustomization, /dd-ai-ml-python-workflow-requirements\.configmap\.yaml/);
  assert.match(kustomization, /dd-ai-ml-agent-secrets\.externalsecret\.yaml/);
  assert.match(kustomization, /dd-ai-ml-data-contracts\.configmap\.yaml/);
  assert.match(kustomization, /dd-ai-ml-serviceaccount\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.deployment\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.service\.yaml/);
  assert.match(kustomization, /dd-ai-ml-pipeline\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-ai-ml-namespace-boundary\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-kafka-namespace-boundary\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-spark-namespace-boundary\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-ai-ml-rest-api-secrets\.externalsecret\.yaml/);

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
  assert.match(dataContracts, /telemetry-ingest\.schema\.json/);
  assert.match(dataContracts, /mdp-telemetry\.schema\.json/);
  assert.match(dataContracts, /dd\.remote\.ml\.deadletter/);
  assert.match(dataContracts, /"maximum": 86400000/);

  assert.match(deployment, /name:\s*dd-ai-ml-pipeline/);
  assert.match(deployment, /namespace:\s*ai-ml/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /serviceAccountName:\s*dd-ai-ml-pipeline/);
  assert.match(deployment, /python:3\.12-slim/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/ai-ml-pipeline/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8099'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /ML_ALLOW_UNAUTHENTICATED[\s\S]*value:\s*'false'/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /ML_RAW_TELEMETRY_SUBJECT[\s\S]*dd\.remote\.telemetry\.raw/);
  assert.match(deployment, /ML_FEATURE_SUBJECT[\s\S]*dd\.remote\.ml\.features/);
  assert.match(deployment, /ML_MDP_TELEMETRY_SUBJECT[\s\S]*dd\.remote\.telemetry\.mdp/);
  assert.match(deployment, /ML_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /ML_DEAD_LETTER_SUBJECT[\s\S]*dd\.remote\.ml\.deadletter/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /securityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-ai-ml-pipeline/);
  assert.match(service, /namespace:\s*ai-ml/);
  assert.match(service, /port:\s*8099/);
  assert.match(externalSecret, /kind:\s*ExternalSecret/);
  assert.match(externalSecret, /namespace:\s*ai-ml/);
  assert.match(externalSecret, /name:\s*dd-agent-secrets/);
  assert.doesNotMatch(externalSecret, /dataFrom:/);
  assert.match(externalSecret, /secretKey:\s*SERVER_AUTH_SECRET/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/agent-secrets/);
  assert.match(externalSecret, /property:\s*SERVER_AUTH_SECRET/);
  assert.match(restApiExternalSecret, /kind:\s*ExternalSecret/);
  assert.match(restApiExternalSecret, /namespace:\s*ai-ml/);
  assert.match(restApiExternalSecret, /name:\s*dd-ai-ml-rest-api-secrets/);
  assert.match(restApiExternalSecret, /target:[\s\S]*name:\s*dd-remote-rest-api-secrets/);
  assert.doesNotMatch(restApiExternalSecret, /dataFrom:/);
  assert.match(restApiExternalSecret, /secretKey:\s*RDS_DATABASE_URL/);
  assert.match(restApiExternalSecret, /key:\s*dd\/remote-dev\/rest-api-secrets/);
  assert.match(restApiExternalSecret, /property:\s*RDS_DATABASE_URL/);
  assert.match(serviceAccount, /automountServiceAccountToken:\s*false/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*default/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(networkPolicy, /port:\s*4222/);
  for (const policy of [aiMlBoundary, kafkaBoundary, sparkBoundary]) {
    assert.match(policy, /kind:\s*NetworkPolicy/);
    assert.match(policy, /podSelector:\s*\{\}/);
    assert.match(policy, /policyTypes:[\s\S]*- Ingress/);
    assert.doesNotMatch(policy, /policyTypes:[\s\S]*- Egress/);
    assert.match(policy, /podSelector:\s*\{\}[\s\S]*namespaceSelector:/);
    assert.match(policy, /kubernetes\.io\/metadata\.name:\s*observability/);
  }
  assert.doesNotMatch(aiMlBoundary, /kubernetes\.io\/metadata\.name:\s*airbyte/);
  assert.doesNotMatch(aiMlBoundary, /kubernetes\.io\/metadata\.name:\s*spark/);
});

test('open-source ai/ml platform tools have Argo CD entries', async () => {
  const project = await readRepoFile('remote/argocd/apps/dd-ai-ml-platform.appproject.yaml');
  const platform = await readRepoFile('remote/argocd/apps/dd-ai-ml-platform.application.yaml');
  const sparkPipeline = await readRepoFile(
    'remote/argocd/apps/dd-spark-pipeline-server.application.yaml',
  );
  const sparkPipelineDeployment = await readRepoFile(
    'remote/deployments/spark-pipeline-server/k8s/ec2/dd-spark-pipeline-server.deployment.yaml',
  );
  const dagster = await readRepoFile('remote/argocd/apps/dd-dagster.application.yaml');
  const airflow = await readRepoFile('remote/argocd/apps/dd-airflow.application.yaml');
  const mlflow = await readRepoFile('remote/argocd/apps/dd-mlflow.application.yaml');
  const kafka = await readRepoFile('remote/argocd/apps/dd-kafka-strimzi.application.yaml');
  const spark = await readRepoFile('remote/argocd/apps/dd-spark-operator.application.yaml');
  const qdrant = await readRepoFile('remote/argocd/apps/dd-qdrant.application.yaml');
  const airbyte = await readRepoFile('remote/argocd/apps/dd-airbyte.application.yaml');

  for (const manifest of [
    platform,
    sparkPipeline,
    dagster,
    airflow,
    mlflow,
    kafka,
    spark,
    qdrant,
    airbyte,
  ]) {
    assert.match(manifest, /project:\s*dd-ai-ml-platform/);
  }
  for (const optionalApp of [sparkPipeline, dagster, airflow, mlflow, kafka, spark, qdrant, airbyte]) {
    assert.doesNotMatch(optionalApp, /CreateNamespace=true/);
  }
  assert.match(sparkPipeline, /destination:[\s\S]*namespace:\s*ai-ml/);
  assert.match(sparkPipelineDeployment, /namespace:\s*ai-ml/);
  assert.match(sparkPipelineDeployment, /automountServiceAccountToken:\s*false/);
  assert.match(sparkPipelineDeployment, /securityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(sparkPipelineDeployment, /securityContext:[\s\S]*runAsUser:\s*1000/);
  assert.match(
    sparkPipelineDeployment,
    /securityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/,
  );
  assert.match(sparkPipelineDeployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(sparkPipelineDeployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(sparkPipelineDeployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(sparkPipelineDeployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(sparkPipelineDeployment, /mountPath:\s*\/work[\s\S]*emptyDir:\s*\{\}/);
  assert.match(project, /kind:\s*AppProject/);
  assert.match(project, /name:\s*dd-ai-ml-platform/);
  assert.match(project, /sourceRepos:[\s\S]*https:\/\/airflow\.apache\.org/);
  assert.match(project, /sourceRepos:[\s\S]*https:\/\/strimzi\.io\/charts\//);
  assert.match(project, /destinations:[\s\S]*namespace:\s*ai-ml/);
  assert.match(project, /destinations:[\s\S]*namespace:\s*airbyte/);
  assert.match(project, /destinations:[\s\S]*namespace:\s*kafka/);
  assert.match(project, /destinations:[\s\S]*namespace:\s*spark/);
  assert.doesNotMatch(project, /destinations:[\s\S]*namespace:\s*default/);
  assert.match(project, /clusterResourceWhitelist:[\s\S]*kind:\s*CustomResourceDefinition/);
  assert.match(project, /clusterResourceWhitelist:[\s\S]*kind:\s*ClusterRole/);
  assert.match(project, /clusterResourceWhitelist:[\s\S]*kind:\s*ClusterRoleBinding/);
  assert.doesNotMatch(project, /group:\s*"\*"/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*Deployment/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*StatefulSet/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*ExternalSecret/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*NetworkPolicy/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*PodDisruptionBudget/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*RoleBinding/);
  assert.match(dagster, /repoURL:\s*https:\/\/dagster-io\.github\.io\/helm/);
  assert.match(dagster, /chart:\s*dagster/);
  assert.match(dagster, /targetRevision:\s*1\.13\.3/);
  assert.match(airflow, /repoURL:\s*https:\/\/airflow\.apache\.org/);
  assert.match(airflow, /chart:\s*airflow/);
  assert.match(airflow, /targetRevision:\s*1\.21\.0/);
  assert.match(airflow, /securityContexts:[\s\S]*pod:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(airflow, /dagProcessor:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(airflow, /statsd:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(airflow, /statsd:[\s\S]*securityContexts:[\s\S]*container:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(mlflow, /repoURL:\s*https:\/\/community-charts\.github\.io\/helm-charts/);
  assert.match(mlflow, /chart:\s*mlflow/);
  assert.match(mlflow, /targetRevision:\s*1\.8\.1/);
  assert.match(mlflow, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(mlflow, /securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(dagster, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(kafka, /repoURL:\s*https:\/\/strimzi\.io\/charts\//);
  assert.match(kafka, /chart:\s*strimzi-kafka-operator/);
  assert.match(kafka, /targetRevision:\s*1\.0\.0/);
  assert.match(kafka, /watchAnyNamespace:\s*false/);
  assert.match(kafka, /podSecurityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(kafka, /securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(kafka, /securityContext:[\s\S]*capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(spark, /repoURL:\s*https:\/\/apache\.github\.io\/spark-kubernetes-operator/);
  assert.match(spark, /chart:\s*spark-kubernetes-operator/);
  assert.match(spark, /targetRevision:\s*1\.6\.0/);
  assert.match(spark, /namespaces:\s*\n\s+create:\s*false/);
  assert.match(spark, /overrideWatchedNamespaces:\s*true/);
  assert.match(qdrant, /repoURL:\s*https:\/\/qdrant\.github\.io\/qdrant-helm/);
  assert.match(qdrant, /chart:\s*qdrant/);
  assert.match(qdrant, /targetRevision:\s*1\.17\.1/);
  assert.match(qdrant, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(qdrant, /containerSecurityContext:[\s\S]*capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(airbyte, /repoURL:\s*https:\/\/airbytehq\.github\.io\/helm-charts/);
  assert.match(airbyte, /chart:\s*airbyte/);
  assert.match(airbyte, /targetRevision:\s*1\.9\.2/);
});

test('airbyte chart avoids internal default database and minio paths', async () => {
  const airbyte = await readRepoFile('remote/argocd/apps/dd-airbyte.application.yaml');
  const kustomization = await readRepoFile('remote/argocd/ai-ml-platform/kustomization.yaml');
  const externalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-platform-secrets.externalsecret.yaml',
  );
  const postgres = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-airbyte-postgresql.statefulset.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-airbyte.networkpolicy.yaml',
  );

  assert.match(airbyte, /postgresql:\s*\n\s+enabled:\s*false/);
  assert.match(airbyte, /database:\s*\n\s+type:\s*external/);
  assert.match(airbyte, /secretName:\s*dd-airbyte-database/);
  assert.match(airbyte, /host:\s*dd-airbyte-postgresql\.airbyte\.svc\.cluster\.local/);
  assert.match(airbyte, /storage:\s*\n\s+type:\s*s3/);
  assert.match(airbyte, /storageSecretName:\s*dd-airbyte-storage/);
  assert.match(airbyte, /authenticationType:\s*credentials/);
  assert.doesNotMatch(airbyte, /minio123|keycloak123|postgresqlPassword:\s*airbyte/);
  assert.match(kustomization, /dd-airbyte-postgresql\.statefulset\.yaml/);
  assert.match(kustomization, /dd-airbyte\.networkpolicy\.yaml/);
  assert.match(externalSecret, /name:\s*dd-airbyte-database/);
  assert.match(externalSecret, /property:\s*AIRBYTE_DATABASE_USER/);
  assert.match(externalSecret, /property:\s*AIRBYTE_DATABASE_PASSWORD/);
  assert.match(externalSecret, /name:\s*dd-airbyte-storage/);
  assert.match(externalSecret, /property:\s*AIRBYTE_S3_ACCESS_KEY_ID/);
  assert.match(externalSecret, /property:\s*AIRBYTE_S3_SECRET_ACCESS_KEY/);
  assert.match(postgres, /image:\s*docker\.io\/library\/postgres:17\.6-alpine/);
  assert.match(postgres, /automountServiceAccountToken:\s*false/);
  assert.match(postgres, /readOnlyRootFilesystem:\s*true/);
  assert.match(postgres, /name:\s*POSTGRES_PASSWORD[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-airbyte-database/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /namespace:\s*airbyte/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress/);
  assert.doesNotMatch(networkPolicy, /policyTypes:[\s\S]*- Egress/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*ai-ml/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*default/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
});

test('gateway, observability, and homepage expose the ai/ml pipeline', async () => {
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');

  assert.match(gateway, /location = \/ml[\s\S]*return 302 \/ml\//);
  assert.match(gateway, /location \/ml\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"/);
  assert.match(
    gateway,
    /location \/ml\/[\s\S]*set \$dd_ml_pipeline_upstream dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099[\s\S]*rewrite \^\/ml\/\?\(\.\*\)\$ \/\$1 break[\s\S]*proxy_pass http:\/\/\$dd_ml_pipeline_upstream/,
  );
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
  assert.match(home, /PathEntry \{ label: "\/ml\/", href: Some\("\/ml\/"\) \}/);
  // web-home-rs now sources the displayed NATS subjects from the generated
  // `dd-nats-subject-defs` crate so the operator dashboard stays in
  // lockstep with the source-of-truth schema.
  assert.match(home, /label: ML_FEATURES_SUBJECT/);
  assert.match(home, /label: TELEMETRY_RAW_SUBJECT/);
  assert.match(runtimeReadme, /AI\/ML feature pipeline/);
  assert.match(
    runtimeReadme,
    /Dagster, Airflow, MLflow, dbt, Kafka through Strimzi, Spark,\s+Metaflow, LlamaIndex, Qdrant, and Airbyte/,
  );
  assert.match(remoteReadme, /argocd\/ai-ml-platform/);
});
