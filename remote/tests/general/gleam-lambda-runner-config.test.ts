import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/gleam-lambda-runner/gleam.toml'))) {
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
  const gleamToml = await readRepoFile('remote/deployments/gleam-lambda-runner/gleam.toml');
  const httpServer = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/http_server.gleam',
  );
  const main = await readRepoFile('remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner.gleam');
  const natsModule = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/nats.gleam',
  );
  const childProcess = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/child_process.gleam',
  );
  const lambdaNats = await readRepoFile('remote/deployments/gleam-lambda-runner/src/lambda_nats.erl');
  const runtimeEnv = await readRepoFile('remote/deployments/gleam-lambda-runner/src/lambda_runtime_env.erl');
  const pgContract = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/pg_contract.gleam',
  );
  const pgDefsToml = await readRepoFile('remote/libs/pg-defs/generated/gleam/gleam.toml');
  const erlPort = await readRepoFile('remote/deployments/gleam-lambda-runner/src/lambda_child_runner.erl');
  const jsRunner = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/child-runtimes/js-function-runner.mjs',
  );
  const pythonRunner = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/child-runtimes/python-function-runner.py',
  );
  const rubyRunner = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/child-runtimes/ruby-function-runner.rb',
  );
  const bashRunner = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/child-runtimes/bash-function-runner.mjs',
  );
  const polyglotRunner = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/child-runtimes/polyglot-function-runner.mjs',
  );
  const restApi = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  const webHome = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const webHomeReadme = await readRepoFile('remote/deployments/web-home-rs/readme.md');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const restApiDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );
  // Single source of truth for shared table DDL; per-table dupes were retired.
  const tableSql = await readRepoFile('remote/libs/pg-defs/schema/schema.sql');
  const externalSecrets = await readRepoFile('remote/argocd/secrets/common/external-secrets.yaml');
  const manifest = await readRepoFile('remote/deployments/gleam-lambda-runner/manifest.toml');
  const dockerfile = await readRepoFile('remote/deployments/gleam-lambda-runner/Dockerfile');
  const nodeRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/nodejs.Dockerfile',
  );
  const bashRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/bash.Dockerfile',
  );
  const golangRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/golang.Dockerfile',
  );
  const dartRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/dart.Dockerfile',
  );
  const erlangRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/erlang.Dockerfile',
  );
  const elixirRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/elixir.Dockerfile',
  );
  const javaRuntimeDockerfile = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/runtime-images/java.Dockerfile',
  );

  assert.match(gleamToml, /name = "gleam_lambda_runner"/);
  assert.match(gleamToml, /dd_pg_defs = \{ path = "\.\.\/\.\.\/libs\/pg-defs\/generated\/gleam" \}/);
  assert.match(manifest, /name = "dd_pg_defs"[\s\S]*source = "local"[\s\S]*path = "\.\.\/\.\.\/libs\/pg-defs\/generated\/gleam"/);
  assert.doesNotMatch(pgDefsToml, /\bpog\b/);
  assert.doesNotMatch(manifest, /\bpgo\b|\bpog\b/);
  assert.match(dockerfile, /COPY remote\/libs\/pg-defs\/generated\/gleam \.\/remote\/libs\/pg-defs\/generated\/gleam/);
  assert.match(dockerfile, /COPY remote\/deployments\/gleam-lambda-runner\/src \.\/remote\/deployments\/gleam-lambda-runner\/src/);
  assert.match(dockerfile, /python3/);
  assert.match(dockerfile, /ruby/);
  assert.match(dockerfile, /bash/);
  assert.match(dockerfile, /alpine\/edge\/main/);
  assert.match(dockerfile, /COPY remote\/deployments\/gleam-lambda-runner\/runtime-images \.\/remote\/deployments\/gleam-lambda-runner\/runtime-images/);
  assert.doesNotMatch(dockerfile, /COPY remote\/deployments\/gleam-lambda-runner \.\/remote\/deployments\/gleam-lambda-runner/);
  assert.match(dockerfile, /WORKDIR \/app\/remote\/deployments\/gleam-lambda-runner/);
  assert.match(nodeRuntimeDockerfile, /nodejs-current/);
  assert.match(bashRuntimeDockerfile, /nodejs-current/);
  assert.match(nodeRuntimeDockerfile, /ENV NODE_NO_WARNINGS=1/);
  assert.match(bashRuntimeDockerfile, /ENV NODE_NO_WARNINGS=1/);
  assert.match(nodeRuntimeDockerfile, /ENTRYPOINT \["node", "--permission", "\/opt\/dd-lambda\/runner\.mjs"\]/);
  assert.match(bashRuntimeDockerfile, /ENTRYPOINT \["node", "--permission", "--allow-child-process", "\/opt\/dd-lambda\/runner\.mjs"\]/);
  for (const runtimeDockerfile of [
    golangRuntimeDockerfile,
    dartRuntimeDockerfile,
    erlangRuntimeDockerfile,
    elixirRuntimeDockerfile,
    javaRuntimeDockerfile,
  ]) {
    assert.match(runtimeDockerfile, /polyglot-function-runner\.mjs/);
    assert.match(runtimeDockerfile, /LAMBDA_TARGET_RUNTIME=/);
    assert.match(runtimeDockerfile, /USER 10001:10001/);
  }
  assert.doesNotMatch(nodeRuntimeDockerfile, /"--allow-net"/);
  assert.doesNotMatch(bashRuntimeDockerfile, /"--allow-net"/);
  assert.doesNotMatch(nodeRuntimeDockerfile, /node:22-alpine/);
  assert.doesNotMatch(bashRuntimeDockerfile, /node:22-alpine/);
  assert.match(pgContract, /import pg_defs/);
  assert.match(pgContract, /pg_defs\.lambda_functions_select_sql/);
  assert.match(gleamToml, /mist = ">= 6\.0\.0 and < 7\.0\.0"/);
  assert.match(
    httpServer,
    /\["invoke", function_id\] ->\s*require_authenticated_post\(req, fn\(\) \{ invoke\(req, function_id\) \}\)/,
  );
  assert.match(
    httpServer,
    /\["check"\] -> require_authenticated_post\(req, fn\(\) \{ check\(req\) \}\)/,
  );
  assert.match(httpServer, /child_process\.check_definition/);
  assert.match(httpServer, /server_auth_secret/);
  assert.match(httpServer, /@external\(erlang, "lambda_runtime_env", "getenv"\)/);
  assert.match(httpServer, /pub fn bind_host\(\)/);
  assert.match(httpServer, /pub fn bind_port\(\)/);
  assert.match(httpServer, /LAMBDA_SERVER_AUTH_SECRET/);
  assert.match(httpServer, /SERVER_AUTH_SECRET/);
  assert.match(httpServer, /REMOTE_DEV_SERVER_SECRET/);
  assert.match(httpServer, /auth_not_configured/);
  assert.match(httpServer, /SERVER_AUTH_SECRET is not configured/);
  assert.match(httpServer, /x-server-auth/);
  assert.match(httpServer, /x-lambda-runner-auth/);
  assert.match(httpServer, /x-agent-auth/);
  assert.match(httpServer, /authConfigured/);
  assert.match(runtimeEnv, /-module\(lambda_runtime_env\)/);
  assert.match(runtimeEnv, /os:getenv\(Name\)/);
  assert.match(httpServer, /NODE_NO_WARNINGS=1/);
  assert.match(httpServer, /node --permission --allow-net child-runtimes\/js-function-runner\.mjs/);
  assert.match(
    httpServer,
    /\["destroy", reuse_key\] ->\s*require_authenticated_post\(req, fn\(\) \{ destroy\(reuse_key\) \}\)/,
  );
  assert.match(httpServer, /child_timeout_ms = 30_000/);
  assert.doesNotMatch(
    httpServer,
    /x-dd-lambda-command|x-dd-lambda-reuse-key|x-dd-lambda-timeout-ms/,
  );
  assert.match(httpServer, /max_body_bytes = 5_242_880/);
  assert.match(httpServer, /string\.replace\("\\t", "\\\\t"\)/);
  assert.match(main, /import gleam_lambda_runner\/nats/);
  assert.match(main, /nats\.start\(\)/);
  assert.match(main, /http_server\.bind_host\(\)/);
  assert.match(main, /http_server\.bind_port\(\)/);
  assert.match(natsModule, /@external\(erlang, "lambda_nats", "start"\)/);
  assert.match(natsModule, /pub fn publish\(subject: String, payload: String\)/);
  assert.match(lambdaNats, /-define\(SERVER, lambda_nats_singleton\)/);
  assert.match(lambdaNats, /gen_tcp:connect/);
  // Subject + queue-group + functions defaults now resolve through
  // dd_nats_subject_consts (generated from
  // remote/libs/nats/subject-defs/schema/lambdas.schema.json) so a schema
  // rename surfaces at build time instead of silently drifting.
  assert.match(lambdaNats, /NATS_LAMBDA_INVOKE_SUBJECT/);
  assert.match(
    lambdaNats,
    /env_binary\("NATS_LAMBDA_INVOKE_SUBJECT", dd_nats_subject_consts:lambdas_invoke_wildcard\(\)\)/,
  );
  assert.match(lambdaNats, /NATS_LAMBDA_RESULT_SUBJECT/);
  assert.match(
    lambdaNats,
    /env_binary\("NATS_LAMBDA_RESULT_SUBJECT", dd_nats_subject_consts:lambdas_results_subject\(\)\)/,
  );
  assert.match(lambdaNats, /NATS_LAMBDA_FUNCTIONS_SUBJECT/);
  assert.match(
    lambdaNats,
    /env_binary\("NATS_LAMBDA_FUNCTIONS_SUBJECT", dd_nats_subject_consts:lambdas_functions_subject\(\)\)/,
  );
  assert.match(
    lambdaNats,
    /env_binary\("NATS_LAMBDA_QUEUE_GROUP", dd_nats_subject_consts:lambda_runner_queue_group\(\)\)/,
  );
  assert.match(lambdaNats, /NATS_USERNAME/);
  assert.match(lambdaNats, /NATS_PASSWORD/);
  assert.match(lambdaNats, /NATS_TOKEN/);
  assert.match(lambdaNats, /CONTAINER_POOL_NATS_URL/);
  assert.match(lambdaNats, /auth_token/);
  assert.match(lambdaNats, /parse_nats_url_auth/);
  assert.match(lambdaNats, /SUB /);
  assert.match(lambdaNats, /PONG\\r\\n/);
  assert.match(lambdaNats, /lambda_child_runner:invoke/);
  assert.match(lambdaNats, /send_pub\(Socket, Subject, Payload\)/);
  assert.match(lambdaNats, /"PUB "/);
  assert.match(lambdaNats, /NATS_LAMBDA_MAX_PAYLOAD_BYTES/);
  assert.match(lambdaNats, /dropping oversized message/);
  assert.match(lambdaNats, /<<"\\\\t">>/);
  assert.match(childProcess, /@external\(erlang, "lambda_child_runner", "invoke"\)/);
  assert.match(childProcess, /@external\(erlang, "lambda_child_runner", "check_definition"\)/);
  assert.match(erlPort, /ShellCommand = "exec " \+\+ binary_to_list\(Command\)/);
  assert.match(erlPort, /open_port\(\{spawn_executable, "\/bin\/sh"\}/);
  assert.match(erlPort, /stderr_to_stdout/);
  assert.match(erlPort, /check_definition\(Command0, DefinitionJson0, TimeoutMs0\)/);
  assert.match(erlPort, /check_worker_key/);
  assert.doesNotMatch(erlPort, /compile check is not implemented/);
  assert.match(erlPort, /worker_loop/);
  assert.match(erlPort, /lambda_child_runner_manager/);
  assert.match(erlPort, /manager_bootstrap/);
  assert.match(erlPort, /lambda_definition_sql/);
  assert.match(erlPort, /'gleam_lambda_runner@pg_contract':lambda_functions_select_sql\(\)/);
  assert.match(erlPort, /id = '", Identifier, "'/);
  assert.match(erlPort, /command_for_definition/);
  assert.match(erlPort, /supported_runtime\(Runtime\)/);
  assert.match(erlPort, /unsupported lambda runtime/);
  assert.match(erlPort, /host_command\(<<"python3">>\)/);
  assert.match(erlPort, /host_command\(<<"ruby">>\)/);
  assert.match(erlPort, /host_command\(<<"bash">>\)/);
  assert.match(erlPort, /<<"golang">>/);
  assert.match(erlPort, /<<"dart">>/);
  assert.match(erlPort, /<<"erlang">>/);
  assert.match(erlPort, /<<"elixir">>/);
  assert.match(erlPort, /<<"java">>/);
  assert.match(erlPort, /host_command_from_env/);
  assert.match(erlPort, /LAMBDA_NODEJS_HOST_COMMAND/);
  assert.match(erlPort, /NODE_NO_WARNINGS=1/);
  assert.match(erlPort, /CONTAINER_POOL_NATS_URL/);
  assert.match(erlPort, /CONTAINER_POOL_NATS_SUBJECT_PREFIX/);
  assert.match(erlPort, /LAMBDA_PYTHON3_HOST_COMMAND/);
  assert.match(erlPort, /LAMBDA_RUBY_HOST_COMMAND/);
  assert.match(erlPort, /LAMBDA_BASH_HOST_COMMAND/);
  assert.match(erlPort, /host_runtime_allowed/);
  assert.match(erlPort, /LAMBDA_ALLOW_HOST_RUNTIMES/);
  assert.match(erlPort, /lambda runtime requires containerized=true/);
  assert.match(erlPort, /container_command/);
  assert.match(erlPort, /LAMBDA_CONTAINER_RUNNER/);
  assert.match(erlPort, /ctr_container_command/);
  assert.match(erlPort, /safe_reuse_key/);
  assert.match(erlPort, /reuseKey contains unsupported characters/);
  assert.match(erlPort, /<<"\\\\t">>/);
  assert.match(erlPort, /--mount type=tmpfs,dst=\/tmp,options=rw:noexec:nosuid:size=16m/);
  assert.match(erlPort, /--cap-drop CAP_NET_RAW/);
  assert.match(erlPort, /--read-only/);
  assert.match(erlPort, /--user 10001:10001/);
  assert.match(erlPort, /--cap-drop ALL/);
  assert.match(erlPort, /--ulimit nofile=64:64/);
  assert.match(erlPort, /prewarm_workers/);
  assert.match(erlPort, /os:getenv\("LAMBDA_DATABASE_URL"\)/);
  assert.doesNotMatch(erlPort, /AGENT_TASKS_RDS_DATABASE_URL|AGENT_TASKS_DATABASE_URL|RDS_DATABASE_URL/);
  assert.match(erlPort, /identifier_kind/);
  assert.match(erlPort, /erlang:monitor\(process, Pid\)/);
  assert.match(erlPort, /dd_lambda_runner_child_stdio_bytes_total/);
  assert.match(jsRunner, /new Function\(/);
  assert.match(jsRunner, /containerPool:\s*Object\.freeze/);
  assert.match(jsRunner, /dispatchContainerPool/);
  assert.match(jsRunner, /CONTAINER_POOL_NATS_URL/);
  assert.match(jsRunner, /CONTAINER_POOL_NATS_SUBJECT_PREFIX/);
  // js-function-runner now defaults the per-pool request subject through
  // the generated containerPoolLanguageRequestsSubject() formatter so the
  // dot layout stays in sync with
  // remote/libs/nats/subject-defs/schema/container-pool.schema.json.
  assert.match(
    jsRunner,
    /import \{\s*containerPoolLanguageRequestsSubject,?\s*\} from '\.\.\/\.\.\/\.\.\/libs\/nats\/subject-defs\/generated\/javascript\/index\.mjs';/,
  );
  assert.match(jsRunner, /containerPoolLanguageRequestsSubject\(poolSlug\)/);
  assert.match(jsRunner, /connectTcp/);
  assert.match(jsRunner, /PUB \$\{subject\} \$\{inbox\}/);
  assert.match(jsRunner, /LAMBDA_FUNCTION_CACHE_MAX/);
  assert.match(jsRunner, /LAMBDA_RESULT_MAX_BYTES/);
  assert.match(jsRunner, /checkOnly === true/);
  assert.match(jsRunner, /globalThis\.console = safeConsole/);
  assert.match(jsRunner, /Object\.defineProperty\(globalThis, 'process'/);
  assert.match(jsRunner, /resolveDefinition/);
  assert.doesNotMatch(jsRunner, /loadFunctionDefinition/);
  assert.doesNotMatch(jsRunner, /node:child_process|execFileAsync\('psql'|LAMBDA_DATABASE_URL/);
  assert.match(jsRunner, /functionBody is required/);
  assert.match(pythonRunner, /SAFE_BUILTINS/);
  assert.match(pythonRunner, /urllib\.request/);
  assert.match(pythonRunner, /handler\(request, context\)/);
  assert.match(pythonRunner, /checkOnly/);
  assert.match(pythonRunner, /"mode": mode/);
  assert.match(rubyRunner, /BasicObject/);
  assert.match(rubyRunner, /StandardError = ::StandardError/);
  assert.match(rubyRunner, /Net::HTTP/);
  assert.match(rubyRunner, /compile_function/);
  assert.match(rubyRunner, /InstructionSequence\.compile/);
  assert.match(rubyRunner, /checkOnly/);
  assert.match(rubyRunner, /rescue StandardError, SyntaxError/);
  assert.match(bashRunner, /spawn\('\/bin\/bash'/);
  assert.match(bashRunner, /function checkBash/);
  assert.match(bashRunner, /spawn\('\/bin\/bash', \['-n'\]/);
  assert.match(bashRunner, /LAMBDA_REQUEST_JSON/);
  assert.match(polyglotRunner, /function runGolang/);
  assert.match(polyglotRunner, /function runDart/);
  assert.match(polyglotRunner, /function runErlang/);
  assert.match(polyglotRunner, /function runElixir/);
  assert.match(polyglotRunner, /function runJava/);
  assert.match(polyglotRunner, /LAMBDA_TARGET_RUNTIME/);
  assert.match(polyglotRunner, /runtime\/image mismatch/);
  assert.match(restApi, /\/api\/lambdas\/functions/);
  assert.match(restApi, /get\(lambda_functions\)\.post\(create_lambda_function\)/);
  assert.match(restApi, /patch\(update_lambda_function\)/);
  assert.match(restApi, /validate_lambda_runtime/);
  assert.match(restApi, /runtime must be one of nodejs, python3, ruby, bash, golang, dart, erlang, elixir, or java/);
  assert.match(restApi, /validate_lambda_reuse_key/);
  assert.match(restApi, /reuseKey may contain only ASCII letters/);
  assert.match(restApi, /validate_lambda_image_build_root/);
  assert.match(restApi, /lambda image build root must not contain \. or \.\. path components/);
  assert.match(restApi, /\.join\("remote"\)[\s\S]*\.join\("deployments"\)[\s\S]*\.join\("gleam-lambda-runner"\)/);
  assert.match(restApi, /"nodejs"/);
  assert.match(restApi, /"python3"/);
  assert.match(restApi, /"ruby"/);
  assert.match(restApi, /"bash"/);
  assert.match(restApi, /"golang"/);
  assert.match(restApi, /"dart"/);
  assert.match(restApi, /"erlang"/);
  assert.match(restApi, /"elixir"/);
  assert.match(restApi, /"java"/);
  assert.match(restApi, /maybe_package_lambda_image/);
  assert.match(restApi, /LAMBDA_IMAGE_BUILD_ENABLED/);
  assert.match(restApi, /LAMBDA_ALLOW_HOST_RUNTIMES/);
  assert.match(restApi, /nodejs-current/);
  assert.match(restApi, /NODE_NO_WARNINGS=1/);
  assert.doesNotMatch(restApi, /node:22-alpine/);
  assert.match(restApi, /normalize_lambda_runtime_alias/);
  assert.match(restApi, /filter_map\(normalize_lambda_runtime_alias\)/);
  assert.match(restApi, /host execution is disabled for this runtime/);
  assert.match(restApi, /nerdctl/);
  assert.match(restApi, /lambda-\{\}", function\.id/);
  assert.match(restApi, /entryCommand must use the managed lambda child runtime/);
  assert.match(restApi, /functionBody exceeds configured byte limit/);
  assert.match(restApi, /NatsLambdaFunctionMessage/);
  assert.match(restApi, /publish_lambda_function_update_to_nats/);
  assert.doesNotMatch(restApi, /invoke_lambda_function/);
  assert.match(webHome, /id="containerized" type="checkbox"/);
  assert.match(webHome, /const hostAllowedRuntimes = new Set\(\["nodejs"\]\)/);
  assert.match(webHome, /function syncContainerPolicy\(\)/);
  assert.match(webHome, /requiresContainer \? "This runtime requires container execution\."/);
  assert.match(webHome, /id="process-profile"/);
  assert.match(webHome, /id="check"/);
  assert.match(webHome, /<option value="nodejs">nodejs process<\/option>/);
  assert.match(webHome, /<option value="python3">python3 process<\/option>/);
  assert.match(webHome, /<option value="ruby">ruby process<\/option>/);
  assert.match(webHome, /<option value="bash">bash process<\/option>/);
  assert.match(webHome, /<option value="dart">dart process<\/option>/);
  assert.match(webHome, /<option value="erlang">erlang process<\/option>/);
  assert.match(webHome, /<option value="elixir">elixir process<\/option>/);
  assert.match(webHome, /<option value="java">java process<\/option>/);
  assert.match(webHome, /<option value="rust">rust process<\/option>/);
  assert.match(webHome, /<option value="golang">golang process<\/option>/);
  assert.match(webHome, /<option value="gleamlang">gleamlang process<\/option>/);
  assert.match(webHome, /registerLambdaServiceWorker/);
  assert.match(webHome, /dd-lambda-function-draft:v2/);
  assert.match(webHome, /localStorage\.setItem/);
  assert.match(webHome, /dd-lambda-draft-save/);
  assert.match(webHome, /func Handler\(request map\[string\]any, context map\[string\]any\) \(any, error\)/);
  assert.match(webHome, /dynamic handler\(Map<String, dynamic> request, Map<String, dynamic> context\)/);
  assert.match(webHome, /-spec handle\(binary\(\), binary\(\)\) -> binary\(\)\./);
  assert.match(webHome, /@spec handle\(binary\(\), binary\(\)\) :: binary\(\)/);
  assert.match(webHome, /public static String handle\(String requestJson, String contextJson\) throws Exception/);
  assert.match(webHome, /id="container-runner"/);
  assert.match(webHome, /containerd \/ ctr/);
  assert.match(webHome, /containerd \/ nerdctl/);
  assert.match(webHome, /<option value="docker">docker<\/option>/);
  assert.match(webHome, /id="base-image"/);
  const processProfilesBlock = webHome.match(/const processProfiles = \{([\s\S]*?)\n\};\nconst hostAllowedRuntimes/);
  assert.ok(processProfilesBlock);
  const bashProfile = processProfilesBlock[1].match(/bash:\s*\{([\s\S]*?)\n\s*\},\n\s*golang:/);
  const golangProfile = processProfilesBlock[1].match(/golang:\s*\{([\s\S]*?)\n\s*\},\n\s*dart:/);
  assert.ok(bashProfile);
  assert.ok(golangProfile);
  assert.match(bashProfile[1], /runtime: "bash"/);
  assert.match(golangProfile[1], /runtime: "golang"/);
  assert.doesNotMatch(golangProfile[1], /runtime: "nodejs"/);
  assert.match(webHome, /docker\.io\/library\/dd-container-pool-rust-runtime:dev/);
  assert.match(webHome, /docker\.io\/library\/dd-container-pool-golang-runtime:dev/);
  assert.match(webHome, /docker\.io\/library\/dd-container-pool-gleamlang-runtime:dev/);
  assert.match(webHome, /metaData\.lambdaDeployment/);
  assert.match(webHome, /context\.containerPool\.dispatch/);
  assert.match(webHome, /const queryParams = new URLSearchParams\(location\.search\)/);
  assert.match(webHome, /function applyQueryAutofill\(\)/);
  assert.match(webHome, /function validateDraft\(\)/);
  assert.match(webHome, /function backendSyntaxCheck\(payload\)/);
  assert.match(webHome, /setSaveState\("backend check passed"/);
  assert.doesNotMatch(webHome, /clientSyntaxCheck/);
  assert.doesNotMatch(webHome, /local check passed/);
  assert.doesNotMatch(webHome, /normalizeRuntime\(payload\.runtime\) !== "nodejs"/);
  assert.match(webHome, /shouldReplaceGeneratedBody/);
  assert.match(webHome, /state\.editorDirty/);
  assert.match(webHome, /"processProfile", "profile", "process"/);
  assert.match(webHome, /"containerRunner", "runner", "baseImage", "image"/);
  assert.match(webHome, /"functionBody",\s*"body",\s*"code",\s*"source"/);
  assert.match(webHome, /state\.queryAutofillActive/);
  assert.match(webHomeReadme, /query params to prefill a new draft/);
  assert.match(webHomeReadme, /`processProfile` \(`nodejs`, `python3`, `ruby`,\s*`bash`, `golang`, `dart`, `erlang`, `elixir`, `java`, `rust`, or `gleamlang`\)/);
  assert.match(webHomeReadme, /`POST \/lambdas\/check` runner path to compile or syntax-check/);
  assert.match(webHome, /<option value="python3">python3<\/option>/);
  assert.match(webHome, /<option value="ruby">ruby<\/option>/);
  assert.match(webHome, /<option value="bash">bash<\/option>/);
  assert.match(webHome, /<option value="golang">golang<\/option>/);
  assert.match(webHome, /<option value="dart">dart<\/option>/);
  assert.match(webHome, /<option value="erlang">erlang<\/option>/);
  assert.match(webHome, /<option value="elixir">elixir<\/option>/);
  assert.match(webHome, /<option value="java">java<\/option>/);
  assert.match(restApiDeployment, /LAMBDA_IMAGE_BUILD_ENABLED/);
  assert.match(restApiDeployment, /securityContext:\s*\n\s*privileged:\s*true/);
  assert.match(restApiDeployment, /mountPath:\s*\/run\/containerd\/containerd.sock/);
  assert.match(restApiDeployment, /mountPath:\s*\/var\/lib\/containerd/);
  assert.match(restApiDeployment, /mountPropagation:\s*Bidirectional/);
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
    /location\s+=\s+\/lambdas\/check[\s\S]*proxy_pass http:\/\/dd-gleam-lambda-runner\.default\.svc\.cluster\.local:8083\/check/,
  );
  assert.match(
    gateway,
    /location\s+\/api\/lambdas\/[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  assert.match(externalSecrets, /name:\s*dd-gleam-lambda-runner-secrets/);
  assert.match(externalSecrets, /key:\s*dd\/remote-dev\/lambda-runner-secrets/);
  // schema.sql is the canonical contract for every shared table; regexes
  // match its `default X not null` ordering (per-table dupes that used
  // `not null default X` were retired in favor of this single file).
  assert.match(tableSql, /Do not apply it directly to a shared database/);
  assert.match(tableSql, /create table if not exists lambda_functions/);
  assert.match(tableSql, /entry_command text default '[^']+' not null/);
  assert.match(tableSql, /lambda_functions_body_size_chk/);
  assert.match(tableSql, /lambda_functions_entry_command_chk/);
  assert.match(tableSql, /containerized boolean default false not null/);
  assert.match(tableSql, /container_build_status/);
  assert.match(tableSql, /runtime in \('nodejs', 'javascript', 'typescript', 'python3', 'python', 'ruby', 'bash', 'shell', 'golang', 'go', 'dart', 'erlang', 'erl', 'elixir', 'ex', 'java', 'jvm'\)/);
});

test('gleam lambda runner ships ec2 service manifests', async () => {
  const ec2Deployment = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml',
  );
  const ec2Service = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.service.yaml',
  );
  const ec2Rbac = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner-rbac.yaml',
  );
  const ec2NetworkPolicy = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.networkpolicy.yaml',
  );
  const ec2Kustomization = await readRepoFile('remote/deployments/gleam-lambda-runner/k8s/ec2/kustomization.yaml');

  assert.match(ec2Deployment, /name:\s*dd-gleam-lambda-runner/);
  assert.match(ec2Deployment, /serviceAccountName:\s*dd-gleam-lambda-runner/);
  assert.doesNotMatch(ec2Deployment, /lambda-nats-bridge|name:\s*nats-bridge/);
  assert.match(ec2Deployment, /securityContext:\s*\n\s*privileged:\s*true/);
  assert.doesNotMatch(ec2Deployment, /hostPID:\s*true/);
  assert.match(ec2Deployment, /alpine\/edge\/main/);
  assert.match(ec2Deployment, /nodejs-current/);
  assert.match(ec2Deployment, /python3/);
  assert.match(ec2Deployment, /ruby/);
  assert.match(ec2Deployment, /bash/);
  assert.match(ec2Deployment, /gcompat/);
  assert.match(ec2Deployment, /libc6-compat/);
  assert.match(ec2Deployment, /rebar3/);
  assert.match(ec2Deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/gleam-lambda-runner/);
  assert.match(ec2Deployment, /gleam deps download \|\| \{ sleep 10; gleam deps download; \}/);
  assert.match(ec2Deployment, /gleam build/);
  assert.match(ec2Deployment, /hpack\.app/);
  assert.match(ec2Deployment, /erlc -o build\/dev\/erlang\/hpack\/ebin/);
  assert.match(ec2Deployment, /containerPort:\s*8083/);
  assert.match(ec2Deployment, /requests:[\s\S]*memory:\s*512Mi/);
  assert.match(ec2Deployment, /limits:[\s\S]*memory:\s*4Gi/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Deployment, /dd-gleam-lambda-runner-secrets/);
  assert.match(ec2Deployment, /name:\s*LAMBDA_DATABASE_URL[\s\S]*key:\s*LAMBDA_DATABASE_URL/);
  assert.match(ec2Deployment, /name:\s*SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/);
  assert.match(ec2Deployment, /name:\s*NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(ec2Deployment, /NATS_LAMBDA_INVOKE_SUBJECT[\s\S]*dd\.remote\.lambdas\.invoke\.\*/);
  assert.match(ec2Deployment, /NATS_LAMBDA_QUEUE_GROUP[\s\S]*dd-gleam-lambda-runner/);
  assert.match(ec2Deployment, /NATS_LAMBDA_RESULT_SUBJECT[\s\S]*dd\.remote\.lambdas\.results/);
  assert.match(ec2Deployment, /NATS_LAMBDA_FUNCTIONS_SUBJECT[\s\S]*dd\.remote\.lambdas\.functions/);
  assert.match(ec2Deployment, /NATS_LAMBDA_MAX_PAYLOAD_BYTES[\s\S]*5242880/);
  assert.match(ec2Deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(ec2Deployment, /LAMBDA_RESULT_MAX_BYTES[\s\S]*1048576/);
  assert.match(ec2Deployment, /LAMBDA_ALLOW_HOST_RUNTIMES[\s\S]*nodejs/);
  assert.match(ec2Deployment, /LAMBDA_PREWARM_RUNTIMES[\s\S]*nodejs/);
  assert.match(ec2Deployment, /LAMBDA_PREWARM_CONTAINER_RUNTIMES[\s\S]*value:\s*''/);
  assert.match(ec2Deployment, /mountPath:\s*\/run\/containerd/);
  assert.match(ec2Deployment, /mountPath:\s*\/var\/lib\/containerd/);
  assert.match(ec2Deployment, /mountPropagation:\s*Bidirectional/);
  assert.match(ec2Deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(ec2Deployment, /LAMBDA_CONTAINER_RUNNER[\s\S]*value:\s*ctr/);
  assert.match(ec2Deployment, /LAMBDA_CONTAINER_NETWORK[\s\S]*value:\s*host/);
  assert.match(ec2Deployment, /mountPath:\s*\/usr\/local\/bin\/ctr/);
  assert.match(ec2Deployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*failureThreshold:\s*60/);
  assert.doesNotMatch(ec2Deployment, /mountPath:\s*\/opt\/cni\/bin/);
  assert.doesNotMatch(ec2Deployment, /mountPath:\s*\/var\/run\/cilium/);
  assert.doesNotMatch(ec2Deployment, /apk add[\s\S]*\|\| true/);
  assert.doesNotMatch(ec2Deployment, /dd-remote-rest-api-secrets/);
  assert.match(ec2Rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-gleam-lambda-runner/);
  assert.match(ec2Rbac, /automountServiceAccountToken:\s*false/);
  assert.match(ec2Kustomization, /dd-gleam-lambda-runner-rbac\.yaml/);
  assert.match(ec2Kustomization, /dd-gleam-lambda-runner\.networkpolicy\.yaml/);
  assert.match(ec2NetworkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-gleam-lambda-runner/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(ec2NetworkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(ec2NetworkPolicy, /port:\s*4222/);
  assert.match(ec2NetworkPolicy, /port:\s*5432/);
  assert.match(ec2NetworkPolicy, /port:\s*8083/);
  assert.match(ec2Service, /port:\s*8083/);
});
