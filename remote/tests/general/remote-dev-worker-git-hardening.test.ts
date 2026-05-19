import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/dev-server/src/server.ts'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('remote dev worker keeps branch-safe git setup and ssh command contracts', async () => {
  const server = await readRepoFile('remote/dev-server/src/server.ts');
  const entrypoint = await readRepoFile('remote/dev-server/entrypoint.sh');
  const packageJson = await readRepoFile('remote/dev-server/package.json');
  const telemetry = await readRepoFile('remote/dev-server/src/telemetry.ts');
  const agentTypes = await readRepoFile('remote/dev-server/src/agents/types.ts');
  const agentIndex = await readRepoFile('remote/dev-server/src/agents/index.ts');
  const genericRunner = await readRepoFile('remote/dev-server/src/agents/generic-ai-sdk.ts');
  const geminiRunner = await readRepoFile('remote/dev-server/src/agents/gemini-sdk.ts');
  const opencodeRunner = await readRepoFile('remote/dev-server/src/agents/opencode-ai-sdk.ts');
  const dockerfile = await readRepoFile('remote/dev-server/Dockerfile');
  const localDockerfile = await readRepoFile('remote/dev-server-local/Dockerfile');
  const readme = await readRepoFile('remote/dev-server/readme.md');
  const lockfile = await readRepoFile('remote/dev-server/pnpm-lock.yaml');
  const brokerServer = await readRepoFile('remote/agent-worker-broker-rs/src/main.rs');
  const idleReaper = await readRepoFile('remote/idle-reaper-rs/src/main.rs');
  const webHome = await readRepoFile('remote/web-home-rs/src/main.rs');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );
  const config = await readRepoFile('remote/k8s/01-configmap.yaml');
  const secretsTemplate = await readRepoFile('remote/k8s/02-secrets.template.yaml');
  const threadTemplate = await readRepoFile('remote/k8s/07-thread-deployment.template.yaml');
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');

  assert.match(server, /async function remoteBranchExists\(branch: string\): Promise<boolean>/);
  assert.match(
    server,
    /async function installWorkspaceDependencies\(workspacePath: string\): Promise<\{\s*ok: boolean;\s*error\?: string;\s*\}>\s*\{/,
  );
  assert.match(server, /\['ls-remote', '--heads', 'origin', branch\]/);
  assert.match(server, /\['fetch', '--quiet', 'origin', config\.baseBranch\]/);
  assert.match(server, /\['fetch', '--quiet', 'origin', session\.branch\]/);
  assert.match(server, /let switchSource = `origin\/\$\{config\.baseBranch\}`/);
  assert.match(server, /switchSource = 'FETCH_HEAD'/);
  assert.match(server, /'switch',[\s\S]*'--discard-changes',[\s\S]*'-C',[\s\S]*session\.branch,/);
  assert.match(server, /'switch',[\s\S]*session\.branch,[\s\S]*switchSource/);
  assert.match(server, /\['merge', '--no-edit', `origin\/\$\{config\.baseBranch\}`\]/);
  assert.match(server, /\['push', '--no-verify', '--set-upstream', 'origin', session\.branch\]/);
  assert.match(server, /\['install', '--frozen-lockfile'\]/);
  assert.match(server, /\['install', '--no-frozen-lockfile'\]/);
  assert.match(server, /frozen pnpm install failed/);
  assert.match(server, /fallback pnpm install failed/);
  assert.match(server, /async function waitForBootGitReady\(\): Promise<void>/);
  assert.match(server, /process\.kill\(Number\(gitReadyPid\), 0\)/);
  assert.match(server, /delete process\.env\.GIT_READY_PID/);
  assert.match(server, /await waitForBootGitReady\(\);/);
  assert.match(server, /const installResult = await installWorkspaceDependencies/);
  assert.match(server, /dependencyInstallOk: installResult\.ok/);
  assert.match(server, /dependencyInstallError: installResult\.error/);
  assert.match(server, /await access\(join\(workspacePath, 'package\.json'\)\)/);
  assert.match(server, /import type \{ Dirent \} from 'node:fs'/);
  assert.match(server, /async function publishOutputs\(state: TaskState, taskOutputsDir: string\): Promise<void>/);
  assert.match(server, /let dirents: Dirent\[\];/);
  assert.doesNotMatch(server, /type DirentLike =/);
  assert.doesNotMatch(server, /as unknown as DirentLike\[\]/);
  assert.match(server, /function sanitizeEventValue\(value: unknown\): unknown/);
  assert.match(server, /typeof value === 'string'[\s\S]*sanitizeEventText\(value\)/);
  assert.match(
    server,
    /Array\.isArray\(value\)[\s\S]*value\.map\(\(item\) => sanitizeEventValue\(item\)\)/,
  );
  assert.match(
    server,
    /Object\.fromEntries\([\s\S]*Object\.entries\(value as Record<string, unknown>\)[\s\S]*sanitizeEventValue\(item\)/,
  );
  assert.match(server, /event\.kind === 'claude'/);
  assert.match(server, /raw: sanitizeEventValue\(event\.raw\)/);
  assert.match(server, /redacted-anthropic-key[\s\S]*redacted-openai-key/);
  assert.match(server, /GET  \/ws\s+— WebSocket replay\/live stream for pinned thread tasks/);
  assert.match(server, /function registerWorkerWebSocketUpgrade\(\): void/);
  assert.match(server, /requestUrl\.pathname !== '\/ws'/);
  assert.match(server, /headerMatches\(request\.headers\['x-server-auth'\], config\.serverAuthSecret\)/);
  assert.match(server, /new WorkerWebSocketClient\(socket, requestedThreadId, taskId\)/);
  assert.match(server, /eventBus\.all\$/);
  assert.match(server, /source: 'node-worker-ws'/);
  assert.match(server, /type: 'worker-welcome'/);
  assert.match(server, /type: 'worker-heartbeat'/);
  assert.match(server, /status: 'waiting-for-task'/);
  assert.match(server, /registerWorkerWebSocketUpgrade\(\);[\s\S]*await fastify\.listen/);
  assert.match(server, /const AGENT_FALLBACK_PROVIDER: AgentProvider = 'openai-sdk'/);
  assert.match(server, /const AGENT_SECONDARY_FALLBACK_PROVIDER: AgentProvider = 'claude-sdk'/);
  assert.match(server, /function configAgentProvider\(value: string \| undefined, fallback: AgentProvider\): AgentProvider/);
  assert.match(server, /function configAgentProviderList\(value: string \| undefined, fallback: AgentProvider\[\]\): AgentProvider\[\]/);
  assert.match(server, /const configuredAgentFallbackProvider = configAgentProvider\(/);
  assert.match(server, /const configuredAgentSecondaryFallbackProvider = configAgentProvider\(/);
  assert.match(server, /agentFallbackProvider: configuredAgentFallbackProvider/);
  assert.match(server, /agentSecondaryFallbackProvider: configuredAgentSecondaryFallbackProvider/);
  assert.match(server, /agentProviderRotation: configAgentProviderList\([\s\S]*'generic-ai-sdk'[\s\S]*'gemini-sdk'/);
  assert.match(server, /agentBranchPrefix: process\.env\.AGENT_BRANCH_PREFIX \?\? 'agent\/k8s\/openai-5\.5'/);
  assert.match(server, /titleHint\?\.trim\(\) \|\| promptHint\?\.trim\(\) \|\| sessionId/);
  assert.match(server, /return `\$\{config\.agentBranchPrefix\}\/\$\{sessionId\}\/\$\{titleSlug\}`/);
  assert.match(server, /function isPlaceholderSessionBranch\(sessionId: string, branch: string\): boolean/);
  assert.match(server, /existing\.taskIds\.size === 0[\s\S]*isPlaceholderSessionBranch\(sessionId, existing\.branch\)/);
  assert.match(server, /prompt,\s*\}\);/);
  assert.doesNotMatch(server, /return `dev-thread\/\$\{sessionId\}/);
  assert.match(server, /processedTasksDir: process\.env\.PROCESSED_TASKS_DIR/);
  assert.match(server, /type TaskReceipt/);
  assert.match(server, /function taskReceiptPath\(taskId: string\): string/);
  assert.match(server, /async function readTaskReceipt\(taskId: string\)/);
  assert.match(server, /async function writeTaskReceipt\(receipt: TaskReceipt\)/);
  assert.match(server, /duplicate: true/);
  assert.doesNotMatch(server, /return reply\.code\(409\)\.send\(\{ error: 'task exists' \}\)/);
  assert.match(packageJson, /"packageManager": "pnpm@9\.15\.4"/);
  assert.match(entrypoint, /export CI="\$\{CI:-true\}"/);
  assert.match(entrypoint, /COREPACK_ENABLE_DOWNLOAD_PROMPT/);
  assert.match(entrypoint, /export PNPM_STORE_DIR="\$\{PNPM_STORE_DIR:-\$REPO_DIR\/\.pnpm-store\}"/);
  assert.match(entrypoint, /export npm_config_store_dir="\$\{npm_config_store_dir:-\$PNPM_STORE_DIR\}"/);
  assert.match(entrypoint, /pnpm --version/);
  assert.match(entrypoint, /pnpm store path --store-dir "\$PNPM_STORE_DIR"/);
  assert.match(entrypoint, /if \[\[ -f package\.json \]\]; then/);
  assert.match(entrypoint, /no root package\.json in workspace; skipping pnpm install/);
  assert.match(entrypoint, /pnpm install --store-dir "\$PNPM_STORE_DIR" --frozen-lockfile --offline/);
  assert.match(
    entrypoint,
    /pnpm install --store-dir "\$PNPM_STORE_DIR" --frozen-lockfile --prefer-offline/,
  );
  assert.match(entrypoint, /pnpm install --store-dir "\$PNPM_STORE_DIR" --prefer-offline/);
  assert.doesNotMatch(entrypoint, /pnpm install --frozen-lockfile --offline/);
  assert.doesNotMatch(entrypoint, /pnpm install --frozen-lockfile --prefer-offline/);
  assert.doesNotMatch(entrypoint, /pnpm install --prefer-offline/);
  assert.match(entrypoint, /find "\$REPO_DIR\/\.git" -maxdepth 1 -type f -name index\.lock -delete/);
  assert.match(server, /const providerOrder = \[\.\.\.config\.agentProviderRotation, state\.provider\]\.filter/);
  assert.match(server, /const attemptGroups: \{ provider: AgentProvider; candidates: AgentEnvCandidate\[\] \}\[\] = \[\]/);
  assert.match(server, /buildAgentEnvCandidates\(provider\)/);
  assert.match(server, /status: `agent-fallback:\$\{group\.provider\}`/);
  assert.match(server, /promptLikelyRequiresWorkspaceAccess\(state\.prompt\)/);
  assert.match(server, /providerCanAccessWorkspace\(provider\)/);
  assert.match(server, /formatAgentFailureSummary\(/);
  assert.match(server, /await runAgentAttempt\(attempt\)/);
  assert.match(server, /function shouldForwardAgentRunnerEvent\(event: AgentRunnerEvent\): boolean/);
  assert.match(server, /agentEventHasProviderError\(event\.raw\)/);
  assert.match(server, /agentEventIsProviderMetadataOnly\(event\.raw\)/);
  assert.match(server, /if \(shouldForwardAgentRunnerEvent\(ev\)\) \{[\s\S]*emit\(state, ev\);/);
  assert.doesNotMatch(server, /agent-fallback:echo/);
  assert.doesNotMatch(server, /runSelectedAgent\('echo'\)/);
  assert.match(server, /\['commit', '--no-verify', '-m'/);
  assert.match(server, /\['push', '--no-verify', '--set-upstream', 'origin'/);
  assert.match(server, /status: `opening draft PR against \$\{config\.baseBranch\} from \$\{gitBranchTarget\(session\.branch\)\}`/);
  assert.match(server, /status: `completed PR request: \$\{result\.reused \? 'reused' : 'created'\} draft PR against \$\{result\.baseBranch\}`/);
  assert.match(server, /status: `pushing to \$\{gitBranchTarget\(state\.branch\)\}`/);
  assert.match(server, /status: `pushed to \$\{gitBranchTarget\(state\.branch\)\}`/);
  assert.match(server, /status: `completed task on \$\{gitBranchTarget\(state\.branch\)\}`/);
  assert.match(server, /const GENERATED_GIT_EXCLUDES = \[':!\.pnpm-store'/);
  assert.match(server, /async function gitWorkspaceStatus\(workspacePath: string\): Promise<string>/);
  assert.match(server, /async function gitAddWorkspaceChanges\(workspacePath: string\): Promise<void>/);
  assert.match(server, /function stripNegatedWorkspaceChangePhrases\(prompt: string\): string/);
  assert.match(server, /const editablePrompt = stripNegatedWorkspaceChangePhrases\(prompt\)/);
  assert.match(server, /function promptLikelyRequiresWorkspaceChange\(prompt: string\): boolean/);
  assert.match(server, /function providerCanEditWorkspace\(provider: AgentProvider\): boolean/);
  assert.match(server, /type DeterministicAppendFileEdit = \{/);
  assert.match(server, /function parseDeterministicAppendFilePrompt\(prompt: string\): DeterministicAppendFileEdit \| null/);
  assert.match(server, /function safeRepoRelativePath\(workspacePath: string, rawPath: string\): string/);
  assert.match(server, /async function applyDeterministicWorkspaceEdit\(/);
  assert.match(server, /status: 'deterministic-edit:append-file'/);
  assert.match(server, /await applyDeterministicWorkspaceEdit\(state\)/);
  assert.match(server, /blockedSegments = new Set\(\['\.git', 'node_modules', '\.pnpm-store', '\.next', '\.turbo'\]\)/);
  assert.match(agentTypes, /\| ['"]gemini-sdk['"]/);
  assert.match(agentTypes, /\| ['"]generic-ai-sdk['"]/);
  assert.match(agentTypes, /\| ['"]opencode-ai-sdk['"]/);
  assert.doesNotMatch(agentTypes, /\| ['"]echo['"]/);
  assert.match(agentIndex, /genericAiSdkRunner/);
  assert.match(agentIndex, /DEFAULT_GENERIC_AI_SDK_SOURCES/);
  assert.match(agentIndex, /defaultGenericAiSdkModels/);
  assert.match(agentIndex, /import \{ opencodeAiSdkRunner, DEFAULT_OPENCODE_MODELS \} from '\.\/opencode-ai-sdk\.js'/);
  assert.match(agentIndex, /'generic-ai-sdk': genericAiSdkRunner/);
  assert.match(agentIndex, /'opencode-ai-sdk': opencodeAiSdkRunner/);
  assert.match(agentIndex, /const DEFAULT_GEMINI_MODEL = 'gemini-3\.1-pro-preview'/);
  assert.match(agentIndex, /const DEFAULT_GEMINI_FALLBACK_MODEL = 'gemini-3\.1-flash-lite'/);
  assert.match(agentIndex, /configuredSecretList\('OPENAI_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('ANTHROPIC_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('OPENCODE_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('DEEPSEEK_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('DASHSCOPE_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('XAI_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /configuredSecretList\('GEMINI_API_KEYS_JSON'\)/);
  assert.match(agentIndex, /export function buildAgentEnvCandidates\(provider: AgentProvider\): AgentEnvCandidate\[\]/);
  assert.match(agentIndex, /if \(provider === 'generic-ai-sdk'\) \{/);
  assert.match(agentIndex, /base\.OPENCODE_BASE_URL = process\.env\.OPENCODE_BASE_URL \?\? 'https:\/\/opencode\.ai\/zen\/v1'/);
  assert.match(agentIndex, /base\.OPENCODE_MODELS =[\s\S]*DEFAULT_OPENCODE_MODELS\.join\(','\)/);
  assert.match(agentIndex, /base\.GEMINI_FALLBACK_MODEL =[\s\S]*DEFAULT_GEMINI_FALLBACK_MODEL/);
  assert.match(agentIndex, /OPENCODE_API_KEY not set/);
  assert.match(agentIndex, /GOOGLE_API_KEY or GEMINI_API_KEY not set/);
  assert.match(agentIndex, /chosen = isAgentProvider\(fromEnv\) \? fromEnv : 'openai-sdk'/);
  assert.doesNotMatch(agentIndex, /echoRunner|echo: echoRunner|provider: ['"]echo['"]/);
  assert.match(server, /threadTitle:\s*z\.string\(\)\.min\(1\)\.max\(200\)\.nullish\(\)/);
  assert.match(
    server,
    /provider:\s*z[\s\S]*\.enum\(\[[\s\S]*'generic-ai-sdk'[\s\S]*'opencode-ai-sdk'[\s\S]*'openai-sdk'[\s\S]*\]\)[\s\S]*\.nullish\(\)/,
  );
  assert.match(server, /threadTitle:\s*parsed\.data\.threadTitle \?\? undefined/);
  assert.match(server, /resolveAgentProvider\(parsed\.data\.provider \?\? undefined\)/);
  assert.match(
    geminiRunner,
    /const primaryModel = opts\.env\.GEMINI_MODEL \?\? 'gemini-3\.1-pro-preview'/,
  );
  assert.match(geminiRunner, /GEMINI_FALLBACK_MODEL/);
  assert.match(geminiRunner, /isQuotaFailure/);
  assert.match(geminiRunner, /if \(text\.trim\(\)\) \{[\s\S]*opts\.emit\(\{[\s\S]*provider: 'gemini-sdk'/);
  assert.doesNotMatch(geminiRunner, /quota\/rate limit failed; retrying/);
  assert.match(geminiRunner, /MALFORMED_FUNCTION_CALL/);
  assert.match(geminiRunner, /produced no text output/);
  assert.match(opencodeRunner, /import \{ generateText \} from 'ai'/);
  assert.match(opencodeRunner, /createOpenAICompatible/);
  assert.match(opencodeRunner, /DEFAULT_OPENCODE_MODELS = \[[\s\S]*'big-pickle'[\s\S]*'deepseek-v4-flash-free'[\s\S]*'minimax-m2\.5-free'[\s\S]*'nemotron-3-super-free'[\s\S]*'qwen3\.6-plus-free'/);
  assert.match(opencodeRunner, /const baseURL = opts\.env\.OPENCODE_BASE_URL \?\? DEFAULT_OPENCODE_BASE_URL/);
  assert.match(opencodeRunner, /model: provider\(modelId\)/);
  assert.match(opencodeRunner, /provider: 'opencode-ai-sdk'/);
  assert.match(genericRunner, /DEFAULT_GENERIC_AI_SDK_SOURCES = \[[\s\S]*id: 'deepseek'[\s\S]*'deepseek-v4-pro'[\s\S]*id: 'qwen'[\s\S]*'qwen3\.6-max-preview'[\s\S]*id: 'xai'[\s\S]*'grok-4\.3'/);
  assert.match(genericRunner, /createOpenAICompatible/);
  assert.match(genericRunner, /model: provider\(modelId\)/);
  assert.match(genericRunner, /provider: 'generic-ai-sdk'/);
  assert.match(config, /AGENT_PROVIDER:\s*'openai-sdk'/);
  assert.match(config, /AGENT_FALLBACK_PROVIDER:\s*'openai-sdk'/);
  assert.match(config, /AGENT_SECONDARY_FALLBACK_PROVIDER:\s*'claude-sdk'/);
  assert.match(config, /AGENT_PROVIDER_ROTATION:\s*'openai-sdk,claude-sdk,generic-ai-sdk,gemini-sdk'/);
  assert.match(config, /AGENT_BRANCH_PREFIX:\s*'agent\/k8s\/openai-5\.5'/);
  assert.match(secretsTemplate, /OPENAI_API_KEYS_JSON/);
  assert.match(secretsTemplate, /ANTHROPIC_API_KEYS_JSON/);
  assert.match(secretsTemplate, /OPENCODE_API_KEYS_JSON/);
  assert.match(secretsTemplate, /OPENCODE_MODELS:\s*"big-pickle,deepseek-v4-flash-free,minimax-m2\.5-free,nemotron-3-super-free,qwen3\.6-plus-free"/);
  assert.match(secretsTemplate, /DEEPSEEK_API_KEYS_JSON/);
  assert.match(secretsTemplate, /DEEPSEEK_MODELS:\s*"deepseek-v4-pro,deepseek-v4-flash"/);
  assert.match(secretsTemplate, /DASHSCOPE_API_KEYS_JSON/);
  assert.match(secretsTemplate, /QWEN_MODELS:\s*"qwen3\.6-max-preview,qwen3\.6-plus,qwen3\.6-flash"/);
  assert.match(secretsTemplate, /XAI_API_KEYS_JSON/);
  assert.match(secretsTemplate, /XAI_MODELS:\s*"grok-4\.3,grok-code-fast-1,grok-4-fast"/);
  assert.match(secretsTemplate, /GEMINI_API_KEYS_JSON/);
  assert.match(secretsTemplate, /GEMINI_MODEL:\s*"gemini-3\.1-pro-preview"/);
  assert.match(secretsTemplate, /GEMINI_FALLBACK_MODEL:\s*"gemini-3\.1-flash-lite"/);

  assert.match(dockerfile, /Optionally bake a "recent baseline" repo/);
  assert.match(dockerfile, /corepack prepare pnpm@9\.15\.4 --activate/);
  assert.doesNotMatch(dockerfile, /corepack prepare pnpm@9 --activate/);
  assert.match(dockerfile, /ARG DD_REPO_CACHE_BUST=manual/);
  assert.match(dockerfile, /ARG DD_REPO_URL\s*\n/);
  assert.doesNotMatch(dockerfile, /ARG DD_REPO_URL=git@github\.com/);
  assert.doesNotMatch(dockerfile, /ENV DD_REPO_URL=/);
  assert.doesNotMatch(dockerfile, /test -n "\$DD_REPO_URL"/);
  assert.match(dockerfile, /if \[ -n "\$DD_REPO_URL" \]; then/);
  assert.match(dockerfile, /DD_REPO_URL not provided; building generic repo-configured worker base/);
  assert.match(localDockerfile, /ARG DD_REPO_URL\s*\n/);
  assert.doesNotMatch(localDockerfile, /ARG DD_REPO_URL=git@github\.com/);
  assert.match(localDockerfile, /test -n "\$DD_REPO_URL"/);
  assert.match(dockerfile, /echo "\$DD_REPO_CACHE_BUST" > \/tmp\/dd-repo-cache-bust/);
  assert.match(
    dockerfile,
    /if \[ -f package\.json \]; then[\s\S]*pnpm install --store-dir \/home\/node\/repo-template\/\.pnpm-store --frozen-lockfile/,
  );
  assert.match(dockerfile, /no root package\.json in repo-template; skipping pnpm install/);
  assert.match(localDockerfile, /if \[ -f package\.json \]; then[\s\S]*pnpm install --frozen-lockfile/);
  assert.doesNotMatch(
    dockerfile,
    /PNPM_STORE_DIR=\/home\/node\/repo-template\/\.pnpm-store pnpm install --frozen-lockfile/,
  );
  assert.match(dockerfile, /ENV HOME=\/home\/node \\\s+USER=node/);
  assert.match(dockerfile, /git clone --depth 50 --branch "\$DD_REPO_REF" "\$DD_REPO_URL" repo-template/);
  assert.match(dockerfile, /WORKSPACE_REPO=\/home\/node\/workspace\/repo/);
  assert.doesNotMatch(dockerfile, /workspace\/repo-template/);
  assert.match(entrypoint, /TEMPLATE_DIR="\$\{REPO_TEMPLATE_DIR:-\/home\/node\/repo-template\}"/);
  assert.match(entrypoint, /REPO_URL="\$\{DD_REPO_URL:-\}"/);
  assert.match(entrypoint, /DD_REPO_URL is required/);
  assert.match(entrypoint, /github_https_to_ssh\(\)/);
  assert.match(entrypoint, /GIT_REPO_URL="\$\(github_https_to_ssh "\$REPO_URL"\)"/);
  assert.match(entrypoint, /git clone --depth 50 --branch "\$BASE_BRANCH" "\$GIT_REPO_URL" "\$REPO_DIR"/);
  assert.match(entrypoint, /git remote set-url origin "\$GIT_REPO_URL"/);
  assert.match(entrypoint, /if \[\[ ! -d "\$REPO_DIR\/\.git" && -d "\$TEMPLATE_DIR\/\.git" \]\]; then/);
  assert.match(entrypoint, /cp -a "\$TEMPLATE_DIR\/\." "\$REPO_DIR\/"/);
  assert.match(entrypoint, /==> git fetch \+ switch starting/);
  assert.doesNotMatch(entrypoint, /GIT_READY_PID/);

  assert.match(readme, /git fetch origin <BASE_BRANCH>/);
  assert.match(readme, /switch from it;[\s\S]*otherwise create from `origin\/<BASE_BRANCH>`/);
  assert.match(readme, /brand-new thread start from fresh `origin\/<BASE_BRANCH>`/);
  assert.match(
    readme,
    /pnpm install --ignore-workspace --frozen-lockfile[\s\S]*standalone package instead of the root workspace/,
  );
  assert.match(readme, /Before the first build you need a `pnpm-lock\.yaml`/);
  assert.match(lockfile, /^lockfileVersion: '9\.0'$/m);
  assert.match(lockfile, /^importers:\s*$/m);

  assert.doesNotMatch(packageJson, /@opentelemetry\/instrumentation/);
  assert.doesNotMatch(packageJson, /@opentelemetry\/auto-instrumentations-node/);
  assert.match(telemetry, /class ExplicitSpan implements TelemetrySpan/);
  assert.match(telemetry, /await fetch\(otlpTraceUrl/);
  assert.doesNotMatch(telemetry, /NodeSDK/);
  assert.doesNotMatch(telemetry, /registerInstrumentations/);
  assert.doesNotMatch(telemetry, /require-in-the-middle|shimmer|diagnostics_channel|async_hooks/);
  assert.doesNotMatch(telemetry, /globalThis\.fetch\s*=|http\.request\s*=|https\.request\s*=/);

  assert.match(deployment, /image: docker\.io\/library\/dd-dev-server:dev/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /runAsUser: 1000/);
  assert.match(deployment, /mountPath: \/home\/node\/workspace/);
  assert.match(deployment, /name: DD_REPO_URL[\s\S]*secretKeyRef:[\s\S]*name: dd-agent-secrets[\s\S]*key: DD_REPO_URL/);
  assert.doesNotMatch(deployment, /git .* clone --depth 1 --branch dev/);
  assert.doesNotMatch(deployment, /apt-get update/);
  assert.match(brokerServer, /repo: Option<String>/);
  assert.match(brokerServer, /fn required_repo\(request: &DispatchTaskRequest\) -> Result<String, String>/);
  assert.match(brokerServer, /"repo": repo/);
  assert.match(brokerServer, /"baseBranch": base_branch/);
  assert.doesNotMatch(brokerServer, /git@github\.com:ORESoftware\/k8s-cluster\.git/);
  assert.match(idleReaper, /worker image build disabled: WORKER_IMAGE_BUILD_REPO_URL missing/);
  assert.doesNotMatch(idleReaper, /WORKER_IMAGE_BUILD_REPO_URL"\)[\s\S]*unwrap_or_else\(\|\| "git@github\.com/);
  assert.match(threadTemplate, /envFrom:[\s\S]*configMapRef:[\s\S]*name: dd-agent-config/);
  assert.match(threadTemplate, /envFrom:[\s\S]*secretRef:[\s\S]*name: dd-agent-secrets/);
  assert.match(threadTemplate, /requests:[\s\S]*cpu:\s*"1m"[\s\S]*memory:\s*"512Mi"/);
  assert.doesNotMatch(threadTemplate, /dd-k8s-home/);
  assert.match(
    config,
    /GIT_SSH_COMMAND: "ssh -F \/dev\/null -i \/home\/node\/\.ssh\/id_ed25519 -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\/home\/node\/\.ssh\/known_hosts"/,
  );
  assert.match(restServer, /fn thread_runtime_image\(\) -> String/);
  assert.match(restServer, /docker\.io\/library\/dd-dev-server:dev/);
  assert.match(restServer, /"mountPath": "\/home\/node\/workspace"/);
  assert.match(restServer, /"runAsUser": 1000/);
  assert.match(restServer, /"requests": \{ "cpu": "1m", "memory": "512Mi" \}/);
  assert.match(restServer, /"limits": \{ "cpu": "2", "memory": "4Gi" \}/);
  assert.match(restServer, /"containerSpecs": container_specs/);
  assert.match(restServer, /"startupProbe": \{[\s\S]*"failureThreshold": 180/);
  assert.match(webHome, /PodScheduled/);
  assert.match(webHome, /worker pending:/);
  assert.match(restServer, /"THREAD_CONTEXT_BASE_URL", "value": "http:\/\/dd-remote-rest-api\.default\.svc\.cluster\.local:8082"/);
  assert.match(restServer, /"NATS_EVENT_SUBJECT", "value": "dd\.remote\.events"/);
  assert.match(restServer, /"envFrom": \[/);
  assert.match(
    restServer,
    /"secretRef": \{ "name": "dd-agent-secrets", "optional": true \}/,
  );
  assert.doesNotMatch(restServer, /git .* clone --depth 1 --branch dev/);
  assert.doesNotMatch(restServer, /apt-get update/);
});
