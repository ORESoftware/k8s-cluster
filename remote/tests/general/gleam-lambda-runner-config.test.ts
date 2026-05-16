import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/gleam-lambda-runner/gleam.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('gleam lambda runner keeps child-process and database contracts explicit', async () => {
  const gleamToml = await readRepoFile('remote/gleam-lambda-runner/gleam.toml');
  const httpServer = await readRepoFile(
    'remote/gleam-lambda-runner/src/gleam_lambda_runner/http_server.gleam',
  );
  const childProcess = await readRepoFile(
    'remote/gleam-lambda-runner/src/gleam_lambda_runner/child_process.gleam',
  );
  const pgContract = await readRepoFile(
    'remote/gleam-lambda-runner/src/gleam_lambda_runner/pg_contract.gleam',
  );
  const erlPort = await readRepoFile('remote/gleam-lambda-runner/src/lambda_child_runner.erl');
  const jsRunner = await readRepoFile(
    'remote/gleam-lambda-runner/child-runtimes/js-function-runner.mjs',
  );
  const pythonRunner = await readRepoFile(
    'remote/gleam-lambda-runner/child-runtimes/python-function-runner.py',
  );
  const rubyRunner = await readRepoFile(
    'remote/gleam-lambda-runner/child-runtimes/ruby-function-runner.rb',
  );
  const bashRunner = await readRepoFile(
    'remote/gleam-lambda-runner/child-runtimes/bash-function-runner.mjs',
  );
  const restApi = await readRepoFile('remote/rest-api-rs/src/main.rs');
  const webHome = await readRepoFile('remote/web-home-rs/src/main.rs');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const restApiDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );
  const tableSql = await readRepoFile('remote/databases/pg/tables/lambda-functions-table.sql');
  const externalSecrets = await readRepoFile('remote/argocd/secrets/external-secrets.yaml');
  const manifest = await readRepoFile('remote/gleam-lambda-runner/manifest.toml');
  const dockerfile = await readRepoFile('remote/gleam-lambda-runner/Dockerfile');

  assert.match(gleamToml, /name = "gleam_lambda_runner"/);
  assert.match(gleamToml, /dd_pg_defs = \{ path = "\.\.\/libs\/pg-defs\/generated\/gleam" \}/);
  assert.match(manifest, /name = "dd_pg_defs"[\s\S]*source = "local"[\s\S]*path = "\.\.\/libs\/pg-defs\/generated\/gleam"/);
  assert.match(dockerfile, /COPY remote\/libs\/pg-defs\/generated\/gleam \.\/remote\/libs\/pg-defs\/generated\/gleam/);
  assert.match(dockerfile, /COPY remote\/gleam-lambda-runner\/src \.\/remote\/gleam-lambda-runner\/src/);
  assert.match(dockerfile, /python3/);
  assert.match(dockerfile, /ruby/);
  assert.match(dockerfile, /bash/);
  assert.match(dockerfile, /COPY remote\/gleam-lambda-runner\/runtime-images \.\/remote\/gleam-lambda-runner\/runtime-images/);
  assert.doesNotMatch(dockerfile, /COPY remote\/gleam-lambda-runner \.\/remote\/gleam-lambda-runner/);
  assert.match(dockerfile, /WORKDIR \/app\/remote\/gleam-lambda-runner/);
  assert.match(pgContract, /import pg_defs/);
  assert.match(pgContract, /pg_defs\.lambda_functions_select_sql/);
  assert.match(gleamToml, /mist = ">= 6\.0\.0 and < 7\.0\.0"/);
  assert.match(
    httpServer,
    /\["invoke", function_id\] -> require_post\(req, fn\(\) \{ invoke\(req, function_id\) \}\)/,
  );
  assert.match(httpServer, /node --permission --allow-net child-runtimes\/js-function-runner\.mjs/);
  assert.match(
    httpServer,
    /\["destroy", reuse_key\] -> require_post\(req, fn\(\) \{ destroy\(reuse_key\) \}\)/,
  );
  assert.match(httpServer, /child_timeout_ms = 30_000/);
  assert.doesNotMatch(
    httpServer,
    /x-dd-lambda-command|x-dd-lambda-reuse-key|x-dd-lambda-timeout-ms/,
  );
  assert.match(httpServer, /max_body_bytes = 5_242_880/);
  assert.match(childProcess, /@external\(erlang, "lambda_child_runner", "invoke"\)/);
  assert.match(erlPort, /open_port\(\{spawn, binary_to_list\(Command\)\}/);
  assert.match(erlPort, /worker_loop/);
  assert.match(erlPort, /lambda_child_runner_manager/);
  assert.match(erlPort, /manager_bootstrap/);
  assert.match(erlPort, /lambda_definition_sql/);
  assert.match(erlPort, /'gleam_lambda_runner@pg_contract':lambda_functions_select_sql\(\)/);
  assert.match(erlPort, /id = '", Identifier, "'/);
  assert.match(erlPort, /command_for_definition/);
  assert.match(erlPort, /host_command\(<<"python3">>\)/);
  assert.match(erlPort, /host_command\(<<"ruby">>\)/);
  assert.match(erlPort, /host_command\(<<"bash">>\)/);
  assert.match(erlPort, /container_command/);
  assert.match(erlPort, /--read-only/);
  assert.match(erlPort, /--user 10001:10001/);
  assert.match(erlPort, /--cap-drop ALL/);
  assert.match(erlPort, /prewarm_workers/);
  assert.match(erlPort, /os:getenv\("LAMBDA_DATABASE_URL"\)/);
  assert.doesNotMatch(erlPort, /AGENT_TASKS_RDS_DATABASE_URL|AGENT_TASKS_DATABASE_URL|RDS_DATABASE_URL/);
  assert.match(erlPort, /identifier_kind/);
  assert.match(erlPort, /erlang:monitor\(process, Pid\)/);
  assert.match(erlPort, /dd_lambda_runner_child_stdio_bytes_total/);
  assert.match(jsRunner, /new Function\(/);
  assert.match(jsRunner, /LAMBDA_FUNCTION_CACHE_MAX/);
  assert.match(jsRunner, /LAMBDA_RESULT_MAX_BYTES/);
  assert.match(jsRunner, /globalThis\.console = safeConsole/);
  assert.match(jsRunner, /Object\.defineProperty\(globalThis, 'process'/);
  assert.match(jsRunner, /resolveDefinition/);
  assert.doesNotMatch(jsRunner, /loadFunctionDefinition/);
  assert.doesNotMatch(jsRunner, /node:child_process|execFileAsync\('psql'|LAMBDA_DATABASE_URL/);
  assert.match(jsRunner, /functionBody is required/);
  assert.match(pythonRunner, /SAFE_BUILTINS/);
  assert.match(pythonRunner, /urllib\.request/);
  assert.match(pythonRunner, /handler\(request, context\)/);
  assert.match(rubyRunner, /BasicObject/);
  assert.match(rubyRunner, /Net::HTTP/);
  assert.match(bashRunner, /spawn\('\/bin\/bash'/);
  assert.match(bashRunner, /LAMBDA_REQUEST_JSON/);
  assert.match(restApi, /\/api\/lambdas\/functions/);
  assert.match(restApi, /get\(lambda_functions\)\.post\(create_lambda_function\)/);
  assert.match(restApi, /patch\(update_lambda_function\)/);
  assert.match(restApi, /validate_lambda_runtime/);
  assert.match(restApi, /"nodejs"/);
  assert.match(restApi, /"python3"/);
  assert.match(restApi, /"ruby"/);
  assert.match(restApi, /"bash"/);
  assert.match(restApi, /maybe_package_lambda_image/);
  assert.match(restApi, /LAMBDA_IMAGE_BUILD_ENABLED/);
  assert.match(restApi, /nerdctl/);
  assert.match(restApi, /entryCommand must use the managed lambda child runtime/);
  assert.match(restApi, /functionBody exceeds configured byte limit/);
  assert.match(restApi, /NatsLambdaFunctionMessage/);
  assert.match(restApi, /publish_lambda_function_update_to_nats/);
  assert.doesNotMatch(restApi, /invoke_lambda_function/);
  assert.match(webHome, /id="containerized" type="checkbox"/);
  assert.match(webHome, /<option value="python3">python3<\/option>/);
  assert.match(webHome, /<option value="ruby">ruby<\/option>/);
  assert.match(webHome, /<option value="bash">bash<\/option>/);
  assert.match(restApiDeployment, /LAMBDA_IMAGE_BUILD_ENABLED/);
  assert.match(restApiDeployment, /mountPath:\s*\/run\/containerd\/containerd.sock/);
  assert.match(restApiDeployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(
    gateway,
    /location\s+\/lambdas\/invoke\/[\s\S]*dd-gleam-lambda-runner\.default\.svc\.cluster\.local:8083\/invoke\//,
  );
  assert.match(
    gateway,
    /location\s+\/lambdas\/invoke\/[\s\S]*request_method != POST[\s\S]*client_max_body_size 5m/,
  );
  assert.match(
    gateway,
    /location\s+\/api\/lambdas\/[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  assert.match(externalSecrets, /name:\s*dd-gleam-lambda-runner-secrets/);
  assert.match(externalSecrets, /key:\s*dd\/remote-dev\/lambda-runner-secrets/);
  assert.match(tableSql, /Do not apply this file directly/);
  assert.match(tableSql, /create table if not exists lambda_functions/);
  assert.match(tableSql, /entry_command text not null default/);
  assert.match(tableSql, /lambda_functions_body_size_chk/);
  assert.match(tableSql, /lambda_functions_entry_command_chk/);
  assert.match(tableSql, /containerized boolean not null default false/);
  assert.match(tableSql, /container_build_status/);
  assert.match(tableSql, /runtime in \('nodejs', 'javascript', 'typescript', 'python3', 'python', 'ruby', 'bash', 'shell'\)/);
});

test('gleam lambda runner ships ec2 and minikube service manifests', async () => {
  const ec2Deployment = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml',
  );
  const ec2Service = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.service.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/minikube/dd-gleam-lambda-runner.deployment.yaml',
  );
  const minikubeService = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/minikube/dd-gleam-lambda-runner.service.yaml',
  );

  assert.match(ec2Deployment, /name:\s*dd-gleam-lambda-runner/);
  assert.match(ec2Deployment, /nodejs-current/);
  assert.match(ec2Deployment, /python3/);
  assert.match(ec2Deployment, /ruby/);
  assert.match(ec2Deployment, /bash/);
  assert.match(ec2Deployment, /cd \/opt\/dd-next-1\/remote\/gleam-lambda-runner/);
  assert.match(ec2Deployment, /gleam deps download/);
  assert.match(ec2Deployment, /containerPort:\s*8083/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Deployment, /dd-gleam-lambda-runner-secrets/);
  assert.match(ec2Deployment, /name:\s*LAMBDA_DATABASE_URL[\s\S]*key:\s*LAMBDA_DATABASE_URL/);
  assert.match(ec2Deployment, /LAMBDA_RESULT_MAX_BYTES[\s\S]*1048576/);
  assert.match(ec2Deployment, /LAMBDA_PREWARM_RUNTIMES[\s\S]*nodejs,python3,ruby,bash/);
  assert.match(ec2Deployment, /LAMBDA_PREWARM_CONTAINER_RUNTIMES[\s\S]*nodejs,python3,ruby,bash/);
  assert.match(ec2Deployment, /mountPath:\s*\/run\/containerd\/containerd.sock/);
  assert.match(ec2Deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.doesNotMatch(ec2Deployment, /dd-remote-rest-api-secrets/);
  assert.match(ec2Service, /port:\s*8083/);
  assert.match(minikubeDeployment, /image:\s*dd-gleam-lambda-runner:dev/);
  assert.match(minikubeDeployment, /containerPort:\s*8083/);
  assert.match(minikubeDeployment, /dd-gleam-lambda-runner-secrets/);
  assert.match(minikubeDeployment, /name:\s*LAMBDA_DATABASE_URL[\s\S]*key:\s*LAMBDA_DATABASE_URL/);
  assert.match(minikubeDeployment, /LAMBDA_FUNCTION_BODY_MAX_BYTES[\s\S]*262144/);
  assert.match(minikubeDeployment, /LAMBDA_PREWARM_RUNTIMES[\s\S]*nodejs,python3,ruby,bash/);
  assert.match(minikubeService, /targetPort:\s*http/);
});
