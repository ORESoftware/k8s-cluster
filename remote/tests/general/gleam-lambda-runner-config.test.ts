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
  const erlPort = await readRepoFile('remote/gleam-lambda-runner/src/lambda_child_runner.erl');
  const jsRunner = await readRepoFile(
    'remote/gleam-lambda-runner/child-runtimes/js-function-runner.mjs',
  );
  const restApi = await readRepoFile('remote/rest-api-rs/src/main.rs');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const tableSql = await readRepoFile('remote/databases/pg/tables/lambda-functions-table.sql');
  const externalSecrets = await readRepoFile('remote/argocd/secrets/external-secrets.yaml');

  assert.match(gleamToml, /name = "gleam_lambda_runner"/);
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
  assert.match(erlPort, /id = '", Identifier, "'::uuid/);
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
  assert.match(restApi, /\/api\/lambdas\/functions/);
  assert.match(restApi, /get\(lambda_functions\)\.post\(create_lambda_function\)/);
  assert.match(restApi, /patch\(update_lambda_function\)/);
  assert.match(restApi, /entryCommand must use the managed lambda child runtime/);
  assert.match(restApi, /functionBody exceeds configured byte limit/);
  assert.match(restApi, /NatsLambdaFunctionMessage/);
  assert.match(restApi, /publish_lambda_function_update_to_nats/);
  assert.doesNotMatch(restApi, /invoke_lambda_function/);
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
  assert.match(ec2Deployment, /cd \/opt\/dd-next-1\/remote\/gleam-lambda-runner/);
  assert.match(ec2Deployment, /gleam deps download/);
  assert.match(ec2Deployment, /containerPort:\s*8083/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Deployment, /dd-gleam-lambda-runner-secrets/);
  assert.match(ec2Deployment, /name:\s*LAMBDA_DATABASE_URL[\s\S]*key:\s*LAMBDA_DATABASE_URL/);
  assert.match(ec2Deployment, /LAMBDA_RESULT_MAX_BYTES[\s\S]*1048576/);
  assert.doesNotMatch(ec2Deployment, /dd-remote-rest-api-secrets/);
  assert.match(ec2Service, /port:\s*8083/);
  assert.match(minikubeDeployment, /image:\s*dd-gleam-lambda-runner:dev/);
  assert.match(minikubeDeployment, /containerPort:\s*8083/);
  assert.match(minikubeDeployment, /dd-gleam-lambda-runner-secrets/);
  assert.match(minikubeDeployment, /name:\s*LAMBDA_DATABASE_URL[\s\S]*key:\s*LAMBDA_DATABASE_URL/);
  assert.match(minikubeDeployment, /LAMBDA_FUNCTION_BODY_MAX_BYTES[\s\S]*262144/);
  assert.match(minikubeService, /targetPort:\s*http/);
});
