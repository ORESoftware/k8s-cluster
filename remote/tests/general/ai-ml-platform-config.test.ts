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

function csvValuesFromYamlEnv(source: string, name: string): Set<string> {
  const match = source.match(new RegExp(`name:\\s*${name}[\\s\\S]*?value:\\s*([^\\n]+)`));
  assert.ok(match, `expected ${name} env var`);
  return new Set(
    match[1]
      .trim()
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean),
  );
}

test('python ai/ml pipeline turns telemetry into MDP-ready feature events', async () => {
  const source = await readRepoFile('remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py');
  const readme = await readRepoFile('remote/deployments/ai-ml-pipeline/readme.md');
  const dockerfile = await readRepoFile('remote/deployments/ai-ml-pipeline/Dockerfile');
  const imageBuildScript = await readRepoFile('remote/tools/build-ai-ml-platform-images.sh');

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
  assert.match(dockerfile, /python:3\.12-slim@sha256:[a-f0-9]{64}/);
  assert.match(dockerfile, /COPY remote\/libs\/nats\/subject-defs\/generated\/python/);
  assert.match(dockerfile, /PYTHONDONTWRITEBYTECODE=1/);
  assert.match(dockerfile, /dd_ai_ml_pipeline\.py/);
  assert.match(imageBuildScript, /CONTAINER_NAMESPACE="\$\{CONTAINER_NAMESPACE:-k8s\.io\}"/);
  assert.match(imageBuildScript, /docker\.io\/library\/dd-ai-ml-pipeline:dev/);
  assert.match(imageBuildScript, /docker\.io\/library\/dd-spark-pipeline-server:dev/);
  assert.doesNotMatch(imageBuildScript, /\b(git checkout|git reset|git stash|rm|sed)\b/);
});

test('ai/ml platform bundle deploys the python pipeline and open-source stack catalog', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-ai-ml-platform.application.yaml');
  const kustomization = await readRepoFile('remote/argocd/ai-ml-platform/kustomization.yaml');
  const namespace = await readRepoFile('remote/argocd/ai-ml-platform/namespace.yaml');
  const airbyteNamespace = await readRepoFile('remote/argocd/ai-ml-platform/airbyte.namespace.yaml');
  const kafkaNamespace = await readRepoFile('remote/argocd/ai-ml-platform/kafka.namespace.yaml');
  const sparkNamespace = await readRepoFile('remote/argocd/ai-ml-platform/spark.namespace.yaml');
  const resourceControls = await readRepoFile(
    'remote/argocd/ai-ml-platform/resource-controls.yaml',
  );
  const availabilityControls = await readRepoFile(
    'remote/argocd/ai-ml-platform/availability-controls.yaml',
  );
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
  const platformExternalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-platform-secrets.externalsecret.yaml',
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
    assert.match(ns, /pod-security\.kubernetes\.io\/enforce:\s*baseline/);
    assert.match(ns, /pod-security\.kubernetes\.io\/audit:\s*restricted/);
    assert.match(ns, /pod-security\.kubernetes\.io\/warn:\s*restricted/);
  }
  assert.match(kustomization, /airbyte\.namespace\.yaml/);
  assert.match(kustomization, /kafka\.namespace\.yaml/);
  assert.match(kustomization, /spark\.namespace\.yaml/);
  assert.match(kustomization, /resource-controls\.yaml/);
  assert.match(kustomization, /availability-controls\.yaml/);
  assert.match(kustomization, /dd-dagster-postgresql\.statefulset\.yaml/);
  assert.match(kustomization, /dd-mlflow-postgresql\.statefulset\.yaml/);
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
  assert.match(
    deployment,
    /strategy:[\s\S]*type:\s*RollingUpdate[\s\S]*maxSurge:\s*1[\s\S]*maxUnavailable:\s*0/,
  );
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /serviceAccountName:\s*dd-ai-ml-pipeline/);
  assert.match(deployment, /image:\s*docker\.io\/library\/dd-ai-ml-pipeline:dev/);
  assert.doesNotMatch(deployment, /docker\.io\/library\/python:3\.12-slim/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /mountPath:\s*\/opt\/dd-next-1/);
  assert.doesNotMatch(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/ai-ml-pipeline/);
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
  assert.match(deployment, /securityContext:[\s\S]*runAsUser:\s*10001/);
  assert.match(deployment, /securityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /name:\s*tmp[\s\S]*emptyDir:[\s\S]*sizeLimit:\s*512Mi/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-ai-ml-pipeline/);
  assert.match(service, /namespace:\s*ai-ml/);
  assert.match(service, /port:\s*8099/);
  for (const ns of ['ai-ml', 'airbyte', 'kafka', 'spark']) {
    assert.match(resourceControls, new RegExp(`kind:\\s*ResourceQuota[\\s\\S]*namespace:\\s*${ns}`));
    assert.match(resourceControls, new RegExp(`kind:\\s*LimitRange[\\s\\S]*namespace:\\s*${ns}`));
  }
  assert.match(resourceControls, /requests\.cpu/);
  assert.match(resourceControls, /limits\.memory/);
  assert.match(resourceControls, /persistentvolumeclaims/);
  assert.match(resourceControls, /defaultRequest:[\s\S]*cpu:\s*100m/);
  assert.match(resourceControls, /max:[\s\S]*memory:\s*32Gi/);
  for (const [name, namespace, label] of [
    ['dd-ai-ml-pipeline', 'ai-ml', 'app:\\s*dd-ai-ml-pipeline'],
    ['dd-airbyte-postgresql', 'airbyte', 'app\\.kubernetes\\.io/name:\\s*dd-airbyte-postgresql'],
    ['dd-dagster-postgresql', 'ai-ml', 'app\\.kubernetes\\.io/name:\\s*dd-dagster-postgresql'],
    ['dd-mlflow-postgresql', 'ai-ml', 'app\\.kubernetes\\.io/name:\\s*dd-mlflow-postgresql'],
  ] as const) {
    assert.match(
      availabilityControls,
      new RegExp(
        `kind:\\s*PodDisruptionBudget[\\s\\S]*name:\\s*${name}[\\s\\S]*namespace:\\s*${namespace}[\\s\\S]*minAvailable:\\s*1[\\s\\S]*${label}`,
      ),
    );
  }
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
  assert.match(aiMlBoundary, /kind:\s*NetworkPolicy/);
  assert.match(aiMlBoundary, /policyTypes:[\s\S]*- Ingress/);
  assert.doesNotMatch(aiMlBoundary, /policyTypes:[\s\S]*- Egress/);
  assert.match(
    aiMlBoundary,
    /matchExpressions:[\s\S]*key:\s*app[\s\S]*operator:\s*NotIn[\s\S]*dd-ai-ml-pipeline/,
  );
  assert.match(
    aiMlBoundary,
    /key:\s*app\.kubernetes\.io\/name[\s\S]*operator:\s*NotIn[\s\S]*dd-dagster-postgresql[\s\S]*dd-mlflow-postgresql/,
  );
  assert.match(aiMlBoundary, /namespaceSelector:[\s\S]*kubernetes\.io\/metadata\.name:\s*observability/);
  for (const policy of [kafkaBoundary, sparkBoundary]) {
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
  const sparkPipelineKustomization = await readRepoFile(
    'remote/deployments/spark-pipeline-server/k8s/ec2/kustomization.yaml',
  );
  const sparkPipelinePdb = await readRepoFile(
    'remote/deployments/spark-pipeline-server/k8s/ec2/dd-spark-pipeline-server.pdb.yaml',
  );
  const sparkPipelineNetworkPolicy = await readRepoFile(
    'remote/deployments/spark-pipeline-server/k8s/ec2/dd-spark-pipeline-server.networkpolicy.yaml',
  );
  const sparkPipelineMain = await readRepoFile(
    'remote/deployments/spark-pipeline-server/src/main/java/com/oresoftware/dd/sparkpipeline/MainVerticle.java',
  );
  const sparkPipelineApiDocsHandler = await readRepoFile(
    'remote/deployments/spark-pipeline-server/src/main/java/com/oresoftware/dd/sparkpipeline/handlers/ApiDocsHandler.java',
  );
  const sparkPipelinePom = await readRepoFile('remote/deployments/spark-pipeline-server/pom.xml');
  const sparkPipelineDockerfile = await readRepoFile(
    'remote/deployments/spark-pipeline-server/Dockerfile',
  );
  const sparkPipelineApiDocs = JSON.parse(
    await readRepoFile('remote/deployments/spark-pipeline-server/generated/api-docs.json'),
  );
  const generatedApiDocsIndex = JSON.parse(
    await readRepoFile('remote/deployments/generated-api-docs-index.json'),
  );
  const bastion = await readRepoFile('remote/deployments/bastion-rs/src/main.rs');
  const dagster = await readRepoFile('remote/argocd/apps/dd-dagster.application.yaml');
  const airflow = await readRepoFile('remote/argocd/apps/dd-airflow.application.yaml');
  const mlflow = await readRepoFile('remote/argocd/apps/dd-mlflow.application.yaml');
  const kafka = await readRepoFile('remote/argocd/apps/dd-kafka-strimzi.application.yaml');
  const spark = await readRepoFile('remote/argocd/apps/dd-spark-operator.application.yaml');
  const qdrant = await readRepoFile('remote/argocd/apps/dd-qdrant.application.yaml');
  const airbyte = await readRepoFile('remote/argocd/apps/dd-airbyte.application.yaml');
  const platformExternalSecret = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-ai-ml-platform-secrets.externalsecret.yaml',
  );
  const dagsterPostgres = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-dagster-postgresql.statefulset.yaml',
  );
  const mlflowPostgres = await readRepoFile(
    'remote/argocd/ai-ml-platform/dd-mlflow-postgresql.statefulset.yaml',
  );

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
    assert.match(optionalApp, /syncPolicy:[\s\S]*automated:[\s\S]*prune:\s*true[\s\S]*selfHeal:\s*true/);
  }
  assert.doesNotMatch(platform, /CreateNamespace=true/);
  assert.match(sparkPipeline, /destination:[\s\S]*namespace:\s*ai-ml/);
  assert.match(sparkPipelineKustomization, /dd-spark-pipeline-server\.pdb\.yaml/);
  assert.match(sparkPipelineDeployment, /namespace:\s*ai-ml/);
  assert.match(sparkPipelineDeployment, /image:\s*docker\.io\/library\/dd-spark-pipeline-server:dev/);
  assert.doesNotMatch(sparkPipelineDeployment, /docker\.io\/library\/maven/);
  assert.doesNotMatch(sparkPipelineDeployment, /hostPath:/);
  assert.doesNotMatch(sparkPipelineDeployment, /mountPath:\s*\/opt\/dd-next-1/);
  assert.doesNotMatch(sparkPipelineDeployment, /MAVEN_CONFIG/);
  assert.match(
    sparkPipelineDeployment,
    /strategy:[\s\S]*type:\s*RollingUpdate[\s\S]*maxSurge:\s*1[\s\S]*maxUnavailable:\s*0/,
  );
  assert.match(sparkPipelineDeployment, /automountServiceAccountToken:\s*false/);
  assert.match(sparkPipelineDeployment, /securityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(sparkPipelineDeployment, /securityContext:[\s\S]*runAsUser:\s*10001/);
  assert.match(
    sparkPipelineDeployment,
    /securityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/,
  );
  assert.match(sparkPipelineDeployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(sparkPipelineDeployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(sparkPipelineDeployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(sparkPipelineDeployment, /name:\s*tmp[\s\S]*emptyDir:[\s\S]*sizeLimit:\s*512Mi/);
  assert.match(sparkPipelinePdb, /kind:\s*PodDisruptionBudget/);
  assert.match(sparkPipelinePdb, /name:\s*dd-spark-pipeline-server/);
  assert.match(sparkPipelinePdb, /namespace:\s*ai-ml/);
  assert.match(sparkPipelinePdb, /minAvailable:\s*1/);
  assert.match(sparkPipelinePdb, /app:\s*dd-spark-pipeline-server/);
  assert.match(sparkPipelineNetworkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(sparkPipelineNetworkPolicy, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(sparkPipelineNetworkPolicy, /port:\s*8085/);
  assert.match(sparkPipelineNetworkPolicy, /kubernetes\.io\/metadata\.name:\s*kube-system/);
  assert.match(sparkPipelineNetworkPolicy, /protocol:\s*UDP[\s\S]*port:\s*53/);
  assert.doesNotMatch(sparkPipelineNetworkPolicy, /port:\s*443/);
  assert.doesNotMatch(sparkPipelineNetworkPolicy, /cidr:\s*0\.0\.0\.0\/0/);
  assert.match(sparkPipelineNetworkPolicy, /protocol:\s*TCP[\s\S]*port:\s*5432/);
  assert.match(
    sparkPipelineNetworkPolicy,
    /Optional RDS\/Postgres access[\s\S]*cidr:\s*10\.0\.0\.0\/8[\s\S]*cidr:\s*172\.16\.0\.0\/12[\s\S]*cidr:\s*192\.168\.0\.0\/16[\s\S]*ports:[\s\S]*protocol:\s*TCP[\s\S]*port:\s*5432/,
  );
  assert.match(sparkPipelineMain, /router\.get\("\/docs\/api"\)\.handler\(ApiDocsHandler\.html\(\)\)/);
  assert.match(sparkPipelineMain, /router\.get\("\/api\/docs"\)\.handler\(ApiDocsHandler\.html\(\)\)/);
  assert.match(sparkPipelineMain, /router\.get\("\/api\/docs\.json"\)\.handler\(ApiDocsHandler\.json\(\)\)/);
  assert.match(sparkPipelineApiDocsHandler, /getResourceAsStream\(path\)/);
  assert.match(sparkPipelinePom, /<directory>\$\{project\.basedir\}\/generated<\/directory>/);
  assert.match(sparkPipelineDockerfile, /docker\.io\/library\/dd-spark-pipeline-server:dev/);
  assert.match(sparkPipelineDockerfile, /COPY remote\/deployments\/spark-pipeline-server\/generated \.\/generated/);
  assert.equal(sparkPipelineApiDocs.service, 'spark-pipeline-server');
  assert.equal(sparkPipelineApiDocs.language, 'java');
  const sparkPipelineRoutes = new Map(
    sparkPipelineApiDocs.routes.map((route: { path: string }) => [route.path, route]),
  );
  for (const path of ['/docs/api', '/api/docs', '/api/docs.json']) {
    const route = sparkPipelineRoutes.get(path) as { methods: string[]; auth: string } | undefined;
    assert.ok(route, `generated Spark API docs missing ${path}`);
    assert.ok(route.methods.includes('GET'), `${path} must be a GET route`);
    assert.equal(route.auth, 'public');
  }
  const jobRoute = sparkPipelineRoutes.get('/v1/jobs') as { methods: string[]; auth: string } | undefined;
  assert.ok(jobRoute?.methods.includes('GET'));
  assert.ok(jobRoute?.methods.includes('POST'));
  assert.equal(jobRoute?.auth, 'service-defined');
  assert.ok(
    generatedApiDocsIndex.services.some((service: { service: string; language: string }) => {
      return service.service === 'spark-pipeline-server' && service.language === 'java';
    }),
    'generated API docs index missing spark-pipeline-server',
  );
  assert.match(bastion, /slug:\s*"spark-pipeline"[\s\S]*namespace:\s*"ai-ml"/);
  assert.match(
    bastion,
    /slug:\s*"spark-pipeline"[\s\S]*service:\s*"dd-spark-pipeline-server\.ai-ml\.svc\.cluster\.local:8085"/,
  );
  assert.match(project, /kind:\s*AppProject/);
  assert.match(project, /name:\s*dd-ai-ml-platform/);
  assert.match(project, /argocd\.argoproj\.io\/sync-wave:\s*"-20"/);
  assert.match(platform, /argocd\.argoproj\.io\/sync-wave:\s*"-10"/);
  assert.match(sparkPipeline, /argocd\.argoproj\.io\/sync-wave:\s*"5"/);
  for (const optionalApp of [dagster, airflow, mlflow, kafka, spark, qdrant, airbyte]) {
    assert.match(optionalApp, /argocd\.argoproj\.io\/sync-wave:\s*"10"/);
  }
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
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*LimitRange/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*ResourceQuota/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*ExternalSecret/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*NetworkPolicy/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*PodDisruptionBudget/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*Job/);
  assert.doesNotMatch(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*Pod(?:\s|$)/);
  assert.match(project, /namespaceResourceWhitelist:[\s\S]*kind:\s*RoleBinding/);
  assert.match(dagster, /repoURL:\s*https:\/\/dagster-io\.github\.io\/helm/);
  assert.match(dagster, /chart:\s*dagster/);
  assert.match(dagster, /targetRevision:\s*1\.13\.3/);
  assert.match(airflow, /repoURL:\s*https:\/\/airflow\.apache\.org/);
  assert.match(airflow, /chart:\s*airflow/);
  assert.match(airflow, /targetRevision:\s*1\.21\.0/);
  assert.match(airflow, /securityContexts:[\s\S]*pod:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(airflow, /securityContexts:[\s\S]*containers:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(airflow, /volumes:[\s\S]*name:\s*airflow-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(airflow, /volumeMounts:[\s\S]*name:\s*airflow-tmp[\s\S]*mountPath:\s*\/tmp/);
  assert.match(airflow, /data:[\s\S]*metadataSecretName:\s*dd-airflow-secrets/);
  assert.match(airflow, /data:[\s\S]*brokerUrlSecretName:\s*dd-airflow-secrets/);
  assert.match(airflow, /enableBuiltInSecretEnvVars:[\s\S]*AIRFLOW__CELERY__BROKER_URL:\s*false/);
  assert.match(airflow, /apiServer:[\s\S]*extraVolumeMounts:[\s\S]*mountPath:\s*\/opt\/airflow\/logs/);
  assert.match(airflow, /apiServer:[\s\S]*extraVolumes:[\s\S]*name:\s*api-server-logs[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(airflow, /dagProcessor:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(airflow, /dagProcessor:[\s\S]*logGroomerSidecar:[\s\S]*resources:[\s\S]*requests:[\s\S]*cpu:\s*25m/);
  assert.match(airflow, /statsd:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(airflow, /statsd:[\s\S]*securityContexts:[\s\S]*container:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(airflow, /statsd:[\s\S]*securityContexts:[\s\S]*container:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(airflow, /scheduler:[\s\S]*logGroomerSidecar:[\s\S]*resources:[\s\S]*requests:[\s\S]*memory:\s*64Mi/);
  assert.match(airflow, /triggerer:[\s\S]*logGroomerSidecar:[\s\S]*resources:[\s\S]*limits:[\s\S]*memory:\s*128Mi/);
  assert.match(airflow, /migrateDatabaseJob:[\s\S]*extraVolumeMounts:[\s\S]*mountPath:\s*\/opt\/airflow\/logs/);
  assert.match(airflow, /migrateDatabaseJob:[\s\S]*extraVolumes:[\s\S]*name:\s*airflow-migration-logs[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(airflow, /migrateDatabaseJob:[\s\S]*resources:[\s\S]*requests:[\s\S]*cpu:\s*50m/);
  assert.match(airflow, /logs:[\s\S]*emptyDirConfig:[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(airflow, /postgresql:[\s\S]*auth:[\s\S]*username:\s*airflow/);
  assert.match(airflow, /postgresql:[\s\S]*auth:[\s\S]*database:\s*airflow/);
  assert.match(airflow, /postgresql:[\s\S]*existingSecret:\s*dd-airflow-secrets/);
  assert.match(airflow, /postgresql:[\s\S]*serviceAccount:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(airflow, /postgresql:[\s\S]*primary:[\s\S]*resources:[\s\S]*requests:[\s\S]*cpu:\s*100m/);
  assert.match(airflow, /postgresql:[\s\S]*primary:[\s\S]*persistence:[\s\S]*size:\s*10Gi/);
  assert.match(airflow, /postgresql:[\s\S]*containerSecurityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(airflow, /postgresql:[\s\S]*containerSecurityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(airflow, /postgresql:[\s\S]*extraVolumeMounts:[\s\S]*mountPath:\s*\/tmp/);
  assert.match(airflow, /postgresql:[\s\S]*extraVolumes:[\s\S]*name:\s*postgresql-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(airflow, /postgresql:[\s\S]*shmVolume:[\s\S]*sizeLimit:\s*256Mi/);
  assert.match(airflow, /postgresql:[\s\S]*networkPolicy:[\s\S]*primaryAccessOnlyFrom:[\s\S]*enabled:\s*true/);
  assert.match(platformExternalSecret, /name:\s*dd-airflow-secrets[\s\S]*template:[\s\S]*mergePolicy:\s*Merge/);
  assert.match(platformExternalSecret, /postgresql\+psycopg2:\/\/airflow:\{\{ \.password \| urlquery \}\}@dd-airflow-postgresql\.ai-ml\.svc\.cluster\.local:5432\/airflow\?sslmode=disable/);
  assert.match(mlflow, /repoURL:\s*https:\/\/community-charts\.github\.io\/helm-charts/);
  assert.match(mlflow, /chart:\s*mlflow/);
  assert.match(mlflow, /targetRevision:\s*1\.8\.1/);
  assert.match(mlflow, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(mlflow, /securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(mlflow, /securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(mlflow, /serviceAccount:[\s\S]*automount:\s*false/);
  assert.match(mlflow, /extraEnvVars:[\s\S]*HOME:\s*\/tmp\/mlflow-home/);
  assert.match(mlflow, /extraVolumes:[\s\S]*name:\s*mlflow-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(mlflow, /extraVolumeMounts:[\s\S]*name:\s*mlflow-tmp[\s\S]*mountPath:\s*\/tmp/);
  assert.match(mlflow, /extraPodLabels:[\s\S]*dd-mlflow-postgresql-client:\s*'true'/);
  assert.match(mlflow, /backendStore:[\s\S]*postgres:[\s\S]*host:\s*dd-mlflow-postgresql/);
  assert.match(mlflow, /backendStore:[\s\S]*existingDatabaseSecret:[\s\S]*name:\s*dd-mlflow-postgresql/);
  assert.match(mlflow, /auth:[\s\S]*postgres:[\s\S]*host:\s*dd-mlflow-postgresql/);
  assert.match(mlflow, /postgresql:\s*\n\s+enabled:\s*false/);
  assert.match(mlflowPostgres, /image:\s*docker\.io\/library\/postgres:17\.6-alpine@sha256:[a-f0-9]{64}/);
  assert.match(mlflowPostgres, /automountServiceAccountToken:\s*false/);
  assert.match(mlflowPostgres, /readOnlyRootFilesystem:\s*true/);
  assert.match(mlflowPostgres, /name:\s*postgres-run[\s\S]*sizeLimit:\s*64Mi/);
  assert.match(mlflowPostgres, /name:\s*postgres-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(mlflowPostgres, /name:\s*dd-mlflow-postgresql-ingress/);
  assert.match(mlflowPostgres, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(mlflowPostgres, /dd-mlflow-postgresql-client:\s*'true'/);
  assert.match(mlflowPostgres, /egress:\s*\[\]/);
  assert.match(dagster, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(dagster, /generatePostgresqlPasswordSecret:\s*false/);
  assert.match(dagster, /dagsterDaemon:[\s\S]*labels:[\s\S]*dd-dagster-postgresql-client:\s*'true'/);
  assert.match(dagster, /dagsterDaemon:[\s\S]*securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(dagster, /dagsterDaemon:[\s\S]*volumeMounts:[\s\S]*mountPath:\s*\/tmp/);
  assert.match(dagster, /dagsterDaemon:[\s\S]*volumes:[\s\S]*name:\s*dagster-daemon-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(dagster, /dagsterDaemon:[\s\S]*initContainerResources:[\s\S]*requests:[\s\S]*cpu:\s*50m/);
  assert.match(dagster, /dagsterWebserver:[\s\S]*labels:[\s\S]*dd-dagster-postgresql-client:\s*'true'/);
  assert.match(dagster, /dagsterWebserver:[\s\S]*securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(dagster, /dagsterWebserver:[\s\S]*volumeMounts:[\s\S]*mountPath:\s*\/tmp/);
  assert.match(dagster, /dagsterWebserver:[\s\S]*volumes:[\s\S]*name:\s*dagster-webserver-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(dagster, /dagsterWebserver:[\s\S]*initContainerResources:[\s\S]*requests:[\s\S]*memory:\s*128Mi/);
  assert.match(dagster, /runLauncher:[\s\S]*k8sRunLauncher:[\s\S]*resources:[\s\S]*limits:[\s\S]*memory:\s*1Gi/);
  assert.match(dagster, /runLauncher:[\s\S]*k8sRunLauncher:[\s\S]*securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(dagster, /runLauncher:[\s\S]*podSpecConfig:[\s\S]*automountServiceAccountToken:\s*false/);
  assert.match(dagster, /postgresql:[\s\S]*enabled:\s*false/);
  assert.match(dagster, /postgresql:[\s\S]*postgresqlHost:\s*dd-dagster-postgresql/);
  assert.match(dagster, /postgresql:[\s\S]*postgresqlUsername:\s*dagster/);
  assert.match(dagsterPostgres, /image:\s*docker\.io\/library\/postgres:17\.6-alpine@sha256:[a-f0-9]{64}/);
  assert.match(dagsterPostgres, /automountServiceAccountToken:\s*false/);
  assert.match(dagsterPostgres, /readOnlyRootFilesystem:\s*true/);
  assert.match(dagsterPostgres, /name:\s*postgres-run[\s\S]*sizeLimit:\s*64Mi/);
  assert.match(dagsterPostgres, /name:\s*postgres-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(dagsterPostgres, /name:\s*dd-dagster-postgresql-ingress/);
  assert.match(dagsterPostgres, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(dagsterPostgres, /dd-dagster-postgresql-client:\s*'true'/);
  assert.match(dagsterPostgres, /egress:\s*\[\]/);
  assert.match(kafka, /repoURL:\s*https:\/\/strimzi\.io\/charts\//);
  assert.match(kafka, /chart:\s*strimzi-kafka-operator/);
  assert.match(kafka, /targetRevision:\s*1\.0\.0/);
  assert.match(kafka, /watchAnyNamespace:\s*false/);
  assert.match(kafka, /watchNamespaces:[\s\S]*-\s*kafka/);
  assert.match(kafka, /tmpDirSizeLimit:\s*16Mi/);
  assert.match(kafka, /resources:[\s\S]*requests:[\s\S]*cpu:\s*200m/);
  assert.match(kafka, /podSecurityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(kafka, /securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(kafka, /securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(kafka, /securityContext:[\s\S]*capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(spark, /repoURL:\s*https:\/\/apache\.github\.io\/spark-kubernetes-operator/);
  assert.match(spark, /chart:\s*spark-kubernetes-operator/);
  assert.match(spark, /targetRevision:\s*1\.6\.0/);
  assert.doesNotMatch(spark, /controller:\s*\n\s+replicas/);
  assert.match(spark, /operatorDeployment:[\s\S]*replicas:\s*1/);
  assert.match(spark, /operatorDeployment:[\s\S]*strategy:[\s\S]*type:\s*Recreate/);
  assert.match(spark, /operatorPod:[\s\S]*securityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(spark, /operatorContainer:[\s\S]*jvmArgs:[\s\S]*-Djava\.io\.tmpdir=\/opt\/spark-operator\/logs/);
  assert.match(spark, /operatorContainer:[\s\S]*resources:[\s\S]*requests:[\s\S]*ephemeral-storage:\s*1Gi/);
  assert.match(spark, /operatorContainer:[\s\S]*securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(spark, /operatorContainer:[\s\S]*securityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.doesNotMatch(spark, /operatorContainer:[\s\S]*volumeMounts:[\s\S]*mountPath:\s*\/tmp/);
  assert.doesNotMatch(spark, /volumes:[\s\S]*name:\s*tmp[\s\S]*emptyDir:\s*\{\}/);
  assert.match(spark, /namespaces:\s*\n\s+create:\s*false/);
  assert.match(spark, /overrideWatchedNamespaces:\s*true/);
  assert.match(qdrant, /repoURL:\s*https:\/\/qdrant\.github\.io\/qdrant-helm/);
  assert.match(qdrant, /chart:\s*qdrant/);
  assert.match(qdrant, /targetRevision:\s*1\.17\.1/);
  assert.match(qdrant, /image:[\s\S]*useUnprivilegedImage:\s*true/);
  assert.match(qdrant, /updateVolumeFsOwnership:\s*false/);
  assert.match(qdrant, /snapshotPersistence:[\s\S]*enabled:\s*true[\s\S]*size:\s*5Gi/);
  assert.match(qdrant, /additionalVolumes:[\s\S]*secretName:\s*dd-qdrant-api-keys/);
  assert.match(qdrant, /additionalVolumeMounts:[\s\S]*mountPath:\s*\/qdrant\/config\/local\.yaml/);
  assert.doesNotMatch(qdrant, /valueFrom:[\s\S]*secretKeyRef/);
  assert.match(qdrant, /podSecurityContext:[\s\S]*seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(qdrant, /podSecurityContext:[\s\S]*fsGroup:\s*3000/);
  assert.match(qdrant, /containerSecurityContext:[\s\S]*runAsNonRoot:\s*true/);
  assert.match(qdrant, /containerSecurityContext:[\s\S]*allowPrivilegeEscalation:\s*false/);
  assert.match(qdrant, /containerSecurityContext:[\s\S]*readOnlyRootFilesystem:\s*true/);
  assert.match(qdrant, /containerSecurityContext:[\s\S]*capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(platformExternalSecret, /name:\s*dd-qdrant-api-keys[\s\S]*template:[\s\S]*local\.yaml:/);
  assert.match(platformExternalSecret, /api_key:\s*'\{\{ index \. "api-key" \}\}'/);
  assert.match(platformExternalSecret, /read_only_api_key:\s*'\{\{ index \. "read-only-api-key" \}\}'/);
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
  assert.match(
    airbyte,
    /jobs:[\s\S]*kube:[\s\S]*labels:[\s\S]*dd-airbyte-postgresql-client:\s*'true'/,
  );
  assert.match(airbyte, /storage:\s*\n\s+type:\s*s3/);
  assert.match(airbyte, /storageSecretName:\s*dd-airbyte-storage/);
  assert.match(airbyte, /authenticationType:\s*credentials/);
  assert.match(airbyte, /jobs:[\s\S]*resources:[\s\S]*requests:[\s\S]*cpu:\s*100m/);
  for (const component of [
    'airbyte-bootloader',
    'webapp',
    'server',
    'worker',
    'workload-launcher',
    'workload-api-server',
    'temporal',
    'cron',
    'connector-builder-server',
  ]) {
    assert.match(airbyte, new RegExp(`${component}:[\\s\\S]*resources:[\\s\\S]*requests:[\\s\\S]*memory:`));
    assert.match(airbyte, new RegExp(`${component}:[\\s\\S]*resources:[\\s\\S]*limits:[\\s\\S]*cpu:`));
    assert.match(
      airbyte,
      new RegExp(`${component}:[\\s\\S]*containerSecurityContext:[\\s\\S]*readOnlyRootFilesystem:\\s*true`),
    );
  }
  for (const component of [
    'airbyte-bootloader',
    'server',
    'worker',
    'workload-launcher',
    'workload-api-server',
    'temporal',
    'cron',
  ]) {
    assert.match(
      airbyte,
      new RegExp(`${component}:[\\s\\S]*podLabels:[\\s\\S]*dd-airbyte-postgresql-client:\\s*'true'`),
    );
  }
  for (const component of [
    'airbyte-bootloader',
    'server',
    'worker',
    'workload-launcher',
    'workload-api-server',
    'cron',
    'connector-builder-server',
  ]) {
    assert.match(
      airbyte,
      new RegExp(`${component}:[\\s\\S]*extraVolumeMounts:[\\s\\S]*mountPath:\\s*\\/tmp`),
    );
    assert.match(
      airbyte,
      new RegExp(`${component}:[\\s\\S]*extraVolumes:[\\s\\S]*name:\\s*tmpdir[\\s\\S]*sizeLimit:\\s*1Gi`),
    );
  }
  assert.match(airbyte, /webapp:[\s\S]*extraVolumeMounts:[\s\S]*mountPath:\s*\/var\/run\//);
  assert.match(airbyte, /webapp:[\s\S]*extraVolumes:[\s\S]*name:\s*nginx-cache[\s\S]*sizeLimit:\s*64Mi/);
  assert.match(airbyte, /temporal:[\s\S]*extraInitContainers:[\s\S]*name:\s*temporal-config-loader/);
  assert.match(airbyte, /temporal:[\s\S]*extraVolumeMounts:[\s\S]*name:\s*temporal-config[\s\S]*mountPath:\s*\/etc\/temporal\/config/);
  assert.doesNotMatch(airbyte, /minio123|keycloak123|postgresqlPassword:\s*airbyte/);
  assert.match(kustomization, /dd-airbyte-postgresql\.statefulset\.yaml/);
  assert.match(kustomization, /dd-airbyte\.networkpolicy\.yaml/);
  assert.match(externalSecret, /name:\s*dd-airbyte-database/);
  assert.match(externalSecret, /property:\s*AIRBYTE_DATABASE_USER/);
  assert.match(externalSecret, /property:\s*AIRBYTE_DATABASE_PASSWORD/);
  assert.match(externalSecret, /name:\s*dd-airbyte-storage/);
  assert.match(externalSecret, /property:\s*AIRBYTE_S3_ACCESS_KEY_ID/);
  assert.match(externalSecret, /property:\s*AIRBYTE_S3_SECRET_ACCESS_KEY/);
  assert.match(postgres, /image:\s*docker\.io\/library\/postgres:17\.6-alpine@sha256:[a-f0-9]{64}/);
  assert.match(postgres, /automountServiceAccountToken:\s*false/);
  assert.match(postgres, /readOnlyRootFilesystem:\s*true/);
  assert.match(postgres, /name:\s*postgres-run[\s\S]*sizeLimit:\s*64Mi/);
  assert.match(postgres, /name:\s*postgres-tmp[\s\S]*sizeLimit:\s*1Gi/);
  assert.match(postgres, /name:\s*POSTGRES_PASSWORD[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-airbyte-database/);
  assert.match(postgres, /kind:\s*NetworkPolicy[\s\S]*name:\s*dd-airbyte-postgresql-ingress/);
  assert.match(postgres, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(postgres, /dd-airbyte-postgresql-client:\s*'true'/);
  assert.match(postgres, /egress:\s*\[\]/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(
    networkPolicy,
    /matchExpressions:[\s\S]*key:\s*app\.kubernetes\.io\/name[\s\S]*operator:\s*NotIn[\s\S]*dd-airbyte-postgresql/,
  );
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
  const exporterDeployment = await readRepoFile(
    'remote/argocd/observability/k8s-resource-exporter.deployment.yaml',
  );
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');
  const watchedApps = csvValuesFromYamlEnv(exporterDeployment, 'WATCH_APPS');

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
  for (const app of [
    'dd-ai-ml-pipeline',
    'dd-airbyte-postgresql',
    'dd-dagster-postgresql',
    'dd-mlflow-postgresql',
    'dd-spark-pipeline-server',
  ]) {
    assert.ok(watchedApps.has(app), `WATCH_APPS missing ${app}`);
  }
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
