import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/dev-server/src/server.ts'))) {
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
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const entrypoint = await readRepoFile('remote/deployments/dev-server/entrypoint.sh');
  const packageJson = await readRepoFile('remote/deployments/dev-server/package.json');
  const telemetry = await readRepoFile('remote/deployments/dev-server/src/telemetry.ts');
  const agentTypes = await readRepoFile('remote/deployments/dev-server/src/agents/types.ts');
  const agentIndex = await readRepoFile('remote/deployments/dev-server/src/agents/index.ts');
  const genericRunner = await readRepoFile('remote/deployments/dev-server/src/agents/generic-ai-sdk.ts');
  const geminiRunner = await readRepoFile('remote/deployments/dev-server/src/agents/gemini-sdk.ts');
  const opencodeRunner = await readRepoFile('remote/deployments/dev-server/src/agents/opencode-ai-sdk.ts');
  const openaiSdkRunner = await readRepoFile('remote/deployments/dev-server/src/agents/openai-sdk.ts');
  const claudeSdkRunner = await readRepoFile('remote/deployments/dev-server/src/agents/claude-sdk.ts');
  const clusterMcp = await readRepoFile('remote/deployments/dev-server/src/agents/cluster-mcp.ts');
  const workspaceTools = await readRepoFile('remote/deployments/dev-server/src/agents/workspace-tools.ts');
  const dockerfile = await readRepoFile('remote/deployments/dev-server/Dockerfile');
  const localDockerfile = await readRepoFile('remote/dev-server-local/Dockerfile');
  const readme = await readRepoFile('remote/deployments/dev-server/readme.md');
  const lockfile = await readRepoFile('remote/deployments/dev-server/pnpm-lock.yaml');
  const brokerServer = await readRepoFile('remote/deployments/agent-worker-broker-rs/src/main.rs');
  const idleReaper = await readRepoFile('remote/deployments/idle-reaper-rs/src/main.rs');
  const webHome = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );
  const config = await readRepoFile('remote/k8s/01-configmap.yaml');
  const localMinikube = await readRepoFile('remote/dev-server-local/k8s/minikube-dev-server.yaml');
  const secretsTemplate = await readRepoFile('remote/k8s/02-secrets.template.yaml');
  const threadTemplate = await readRepoFile('remote/k8s/07-thread-deployment.template.yaml');
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  const agentsMd = await readRepoFile('AGENTS.md');

  assert.match(server, /async function remoteBranchExists\(branch: string\): Promise<boolean>/);
  assert.match(
    server,
    /async function installWorkspaceDependencies\(workspacePath: string\): Promise<\{\s*ok: boolean;\s*error\?: string;\s*\}>\s*\{/,
  );
  assert.match(server, /\['ls-remote', '--heads', 'origin', branch\]/);
  assert.match(server, /async function fetchRemoteBranch\(workspacePath: string, branch: string, depth = 1\): Promise<void>/);
  assert.match(server, /'--prune'[\s\S]*`--depth=\$\{depth\}`[\s\S]*`\+refs\/heads\/\$\{branch\}:refs\/remotes\/origin\/\$\{branch\}`/);
  assert.match(server, /await fetchRemoteBranch\(config\.workspaceRepo, config\.baseBranch, 1\)/);
  assert.match(server, /await fetchRemoteBranch\(config\.workspaceRepo, session\.branch, 1\)/);
  assert.match(server, /const hasRemoteBranch = await remoteBranchExists\(session\.branch\)/);
  assert.match(server, /const switchSource = hasRemoteBranch \? `origin\/\$\{session\.branch\}` : `origin\/\$\{config\.baseBranch\}`/);
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
  assert.match(server, /DEEPSEEK_API_KEYS_JSON/);
  assert.match(server, /XAI_API_KEYS_JSON/);
  assert.match(server, /GROK_API_KEYS_JSON/);
  assert.match(server, /redacted-xai-key/);
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
  assert.match(server, /const DEFAULT_AGENT_PROVIDER: AgentProvider = 'generic-ai-sdk'/);
  assert.match(server, /const AGENT_FALLBACK_PROVIDER: AgentProvider = 'generic-ai-sdk'/);
  assert.match(server, /const AGENT_SECONDARY_FALLBACK_PROVIDER: AgentProvider = 'opencode-ai-sdk'/);
  assert.match(server, /function configAgentProvider\(value: string \| undefined, fallback: AgentProvider\): AgentProvider/);
  assert.match(server, /function configAgentProviderList\(value: string \| undefined, fallback: AgentProvider\[\]\): AgentProvider\[\]/);
  assert.match(server, /const configuredAgentFallbackProvider = configAgentProvider\(/);
  assert.match(server, /const configuredAgentSecondaryFallbackProvider = configAgentProvider\(/);
  assert.match(server, /agentFallbackProvider: configuredAgentFallbackProvider/);
  assert.match(server, /agentSecondaryFallbackProvider: configuredAgentSecondaryFallbackProvider/);
  assert.match(server, /agentProviderRotation: configAgentProviderList\([\s\S]*'generic-ai-sdk'[\s\S]*'gemini-sdk'/);
  assert.match(server, /agentBranchPrefix: process\.env\.AGENT_BRANCH_PREFIX \?\? 'agent\/k8s\/openai-5\.5'/);
  assert.match(server, /titleHint\?\.trim\(\) \|\| promptHint\?\.trim\(\) \|\| sessionId/);
  assert.match(server, /const branch = `\$\{config\.agentBranchPrefix\}\/\$\{sessionId\}\/\$\{titleSlug\}`/);
  assert.match(server, /assertSafeGitBranchName\(branch, 'session branch'\)/);
  assert.match(server, /return branch/);
  assert.match(server, /function isPlaceholderSessionBranch\(sessionId: string, branch: string\): boolean/);
  assert.match(server, /existing\.taskIds\.size === 0[\s\S]*isPlaceholderSessionBranch\(sessionId, existing\.branch\)/);
  assert.match(server, /prompt,\s*\}\);/);
  assert.doesNotMatch(server, /return `dev-thread\/\$\{sessionId\}/);
  assert.match(server, /processedTasksDir: process\.env\.PROCESSED_TASKS_DIR/);
  assert.match(server, /import \{ clusterMcpPromptSection \} from '\.\/agents\/cluster-mcp\.js'/);
  assert.match(server, /repoContextMaxChars: Number\(process\.env\.REPO_CONTEXT_MAX_CHARS \?\? 24_000\)/);
  assert.match(server, /agentOptimisticMode: process\.env\.AGENT_OPTIMISTIC_MODE !== 'false'/);
  assert.match(server, /agentMcpUrl: process\.env\.AGENT_MCP_ENABLED === 'false' \? null : process\.env\.AGENT_MCP_URL \?\? null/);
  assert.match(server, /async function readRepoContextEntrypoint\(workspacePath: string\): Promise<string>/);
  assert.match(server, /const rootAgents = await existingContextFiles\(workspacePath, \['AGENTS\.md'\]\)/);
  assert.match(server, /const agentDocs = await listMarkdownChildren\(workspacePath, 'agents'\)/);
  assert.match(server, /const docs = await listMarkdownChildren\(workspacePath, 'docs'\)/);
  assert.match(server, /thread-context:repo-files/);
  assert.match(server, /<repo_context_files>/);
  assert.match(server, /<agent_operating_mode>/);
  assert.match(server, /Do not stop to ask the human user a question before acting/);
  assert.match(server, /<local_thread_log_tail>/);
  assert.match(server, /const runtimeContext = clusterMcpPromptSection\(config\.agentMcpUrl\)/);
  assert.match(server, /thread-context:cluster-mcp/);
  assert.match(server, /<runtime_context>/);
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
  assert.match(server, /const providerOrder = \[state\.provider, \.\.\.config\.agentProviderRotation\]\.filter/);
  assert.match(server, /const attemptGroups: \{ provider: AgentProvider; candidates: AgentEnvCandidate\[\] \}\[\] = \[\]/);
  assert.match(server, /buildAgentEnvCandidates\(provider\)/);
  assert.match(server, /DEEPSEEK_API_KEYS_JSON, XAI_API_KEYS_JSON/);
  assert.match(server, /const requestsPullRequest = promptRequestsPullRequest\(state\.prompt\)/);
  assert.match(server, /const pullRequestOnly = requestsPullRequest && !requiresWorkspaceChange && !requiresWorkspaceAccess/);
  assert.match(server, /status: 'deterministic-pr-only'/);
  assert.match(server, /without spending model credentials/);
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
  assert.match(server, /const GENERATED_GIT_EXCLUDE_PATHS = \['\.pnpm-store', 'node_modules', '\.next', '\.turbo'\]/);
  assert.match(server, /const GENERATED_GIT_STATUS_EXCLUDES = GENERATED_GIT_EXCLUDE_PATHS\.map/);
  assert.match(server, /\['add', '-A', '--', '\.'\]/);
  assert.match(server, /\['reset', '-q', 'HEAD', '--', \.\.\.GENERATED_GIT_EXCLUDE_PATHS\]/);
  assert.match(server, /async function gitWorkspaceStatus\(workspacePath: string\): Promise<string>/);
  assert.match(server, /async function gitAddWorkspaceChanges\(workspacePath: string\): Promise<void>/);
  assert.match(server, /function stripNegatedWorkspaceChangePhrases\(prompt: string\): string/);
  assert.match(server, /const editablePrompt = stripNegatedWorkspaceChangePhrases\(prompt\)/);
  assert.match(server, /function promptLikelyRequiresWorkspaceChange\(prompt: string\): boolean/);
  assert.match(server, /\(add\|append\|change\|create\|delete\|edit\|fix\|implement\|modify\|move\|patch\|refactor\|remove\|rename\|replace\|update\|write\)/);
  assert.match(server, /function providerCanEditWorkspace\(provider: AgentProvider\): boolean/);
  assert.match(server, /return provider !== 'gemini-sdk'/);
  assert.doesNotMatch(server, /provider !== 'opencode-ai-sdk' && provider !== 'generic-ai-sdk'/);
  assert.match(server, /function stripNegatedPullRequestPhrases\(prompt: string\): string/);
  assert.match(server, /const prPrompt = stripNegatedPullRequestPhrases\(prompt\)/);
  assert.match(server, /do\\s\+not\|don't\|dont\|never/);
  assert.match(server, /without\|no/);
  assert.match(server, /function promptRequestsPullRequest\(prompt: string\): boolean/);
  assert.match(server, /ensurePullRequestForSession\(\{[\s\S]*session: state\.session,[\s\S]*taskId: state\.taskId,[\s\S]*prompt: state\.prompt/);
  assert.match(server, /kind: 'pr_open'/);
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
  assert.match(agentIndex, /if \(provider === 'opencode-ai-sdk'\) \{/);
  assert.match(agentIndex, /if \(provider === 'generic-ai-sdk'\) \{/);
  assert.match(agentIndex, /env\.OPENCODE_SOURCE = source\.id/);
  assert.match(agentIndex, /env\.OPENCODE_BASE_URL = genericAiSdkBaseUrl\(source\.id, source\.baseURL\)/);
  assert.match(agentIndex, /env\.OPENCODE_MODELS = genericAiSdkModels\(source\.id\)/);
  assert.match(agentIndex, /hasOpenCodeCompatibleApiKey/);
  assert.match(agentIndex, /hasGenericAiSdkApiKey/);
  assert.match(agentIndex, /OpenCode, DeepSeek, Qwen\/DashScope, or xAI\/Grok API key not set/);
  assert.match(agentIndex, /base\.OPENCODE_BASE_URL = process\.env\.OPENCODE_BASE_URL \?\? 'https:\/\/opencode\.ai\/zen\/v1'/);
  assert.match(agentIndex, /base\.OPENCODE_MODELS =[\s\S]*DEFAULT_OPENCODE_MODELS\.join\(','\)/);
  assert.match(agentIndex, /base\.GEMINI_FALLBACK_MODEL =[\s\S]*DEFAULT_GEMINI_FALLBACK_MODEL/);
  assert.match(agentIndex, /AGENT_MCP_ENABLED/);
  assert.match(agentIndex, /AGENT_MCP_URL/);
  assert.match(agentIndex, /AGENT_MCP_CONNECT_TIMEOUT_MS/);
  assert.doesNotMatch(agentIndex, /OPENCODE_API_KEY not set/);
  assert.match(agentIndex, /GOOGLE_API_KEY or GEMINI_API_KEY not set/);
  assert.match(agentIndex, /chosen = isAgentProvider\(fromEnv\) \? fromEnv : 'generic-ai-sdk'/);
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
  assert.match(opencodeRunner, /import \{ generateText, stepCountIs \} from 'ai'/);
  assert.match(opencodeRunner, /stepCountIs/);
  assert.match(opencodeRunner, /createWorkspaceTools/);
  assert.match(opencodeRunner, /createOpenAICompatible/);
  assert.match(opencodeRunner, /DEFAULT_OPENCODE_MODELS = \[[\s\S]*'big-pickle'[\s\S]*'deepseek-v4-flash-free'[\s\S]*'minimax-m2\.5-free'[\s\S]*'nemotron-3-super-free'[\s\S]*'qwen3\.6-plus-free'/);
  assert.match(opencodeRunner, /const baseURL = opts\.env\.OPENCODE_BASE_URL \?\? DEFAULT_OPENCODE_BASE_URL/);
  assert.match(opencodeRunner, /const source = opts\.env\.OPENCODE_SOURCE \?\? 'opencode'/);
  assert.match(opencodeRunner, /name: source/);
  assert.match(opencodeRunner, /model: provider\(modelId\)/);
  assert.match(opencodeRunner, /provider: 'opencode-ai-sdk'/);
  assert.match(opencodeRunner, /tools: createWorkspaceTools\(opts\.cwd, opts\.emit\)/);
  assert.match(opencodeRunner, /stopWhen: stepCountIs\(8\)/);
  assert.match(genericRunner, /DEFAULT_GENERIC_AI_SDK_SOURCES = \[[\s\S]*id: 'deepseek'[\s\S]*'deepseek-v4-flash'[\s\S]*'deepseek-v4-pro'[\s\S]*id: 'qwen'[\s\S]*'qwen3\.6-max-preview'[\s\S]*id: 'xai'[\s\S]*'grok-4\.3'/);
  assert.match(genericRunner, /code-fast slugs retired on 2026-05-15/);
  assert.match(genericRunner, /createOpenAICompatible/);
  assert.match(genericRunner, /model: provider\(modelId\)/);
  assert.match(genericRunner, /provider: 'generic-ai-sdk'/);
  assert.match(genericRunner, /stepCountIs/);
  assert.match(genericRunner, /createWorkspaceTools/);
  assert.match(genericRunner, /tools: createWorkspaceTools\(opts\.cwd, opts\.emit\)/);
  assert.match(genericRunner, /stopWhen: stepCountIs\(8\)/);
  assert.match(workspaceTools, /BLOCKED_PATH_SEGMENTS = new Set\(\['\.git', 'node_modules', '\.pnpm-store', '\.next', '\.turbo'\]\)/);
  assert.match(workspaceTools, /relativePath: relativePath \|\| '\.'/);
  assert.match(workspaceTools, /if \(segment && !pathSegmentAllowed\(segment\)\)/);
  assert.match(workspaceTools, /list_files: tool/);
  assert.match(workspaceTools, /read_file: tool/);
  assert.match(workspaceTools, /write_file: tool/);
  assert.match(workspaceTools, /replace_in_file: tool/);
  assert.match(workspaceTools, /append_file: tool/);
  assert.match(workspaceTools, /workspace_status: tool/);
  assert.match(workspaceTools, /execFileAsync\('git', \['status', '--short'\]/);
  assert.doesNotMatch(workspaceTools, /execFileAsync\([^'"]/);
  assert.match(clusterMcp, /CLUSTER_MCP_SERVER_NAME = 'dd_cluster'/);
  assert.match(clusterMcp, /kubernetes_inventory/);
  assert.match(clusterMcp, /kubernetes_deployments/);
  assert.match(clusterMcp, /human_access_policy/);
  assert.match(clusterMcp, /AGENT_MCP_URL/);
  assert.match(clusterMcp, /read-only DD EC2 Kubernetes cluster MCP server/);
  assert.match(clusterMcp, /authenticated gateway, VPN, and bastion flow/);
  assert.match(openaiSdkRunner, /MCPServerStreamableHttp/);
  assert.match(openaiSdkRunner, /connectMcpServers/);
  assert.match(openaiSdkRunner, /mcpServers/);
  assert.match(openaiSdkRunner, /clusterMcpInstructions\(\)/);
  assert.match(claudeSdkRunner, /mcpServers/);
  assert.match(claudeSdkRunner, /strictMcpConfig:\s*true/);
  assert.match(claudeSdkRunner, /mcp__\$\{CLUSTER_MCP_SERVER_NAME\}__\$\{name\}/);
  assert.match(config, /AGENT_PROVIDER:\s*'generic-ai-sdk'/);
  assert.match(config, /AGENT_FALLBACK_PROVIDER:\s*'generic-ai-sdk'/);
  assert.match(config, /AGENT_SECONDARY_FALLBACK_PROVIDER:\s*'opencode-ai-sdk'/);
  assert.match(config, /AGENT_PROVIDER_ROTATION:\s*'generic-ai-sdk,opencode-ai-sdk,openai-sdk,claude-sdk,gemini-sdk'/);
  assert.match(config, /AGENT_BRANCH_PREFIX:\s*'agent\/k8s\/openai-5\.5'/);
  assert.match(config, /AGENT_MCP_URL:\s*'http:\/\/dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090\/mcp'/);
  assert.match(config, /AGENT_MCP_CONNECT_TIMEOUT_MS:\s*'3000'/);
  assert.match(secretsTemplate, /OPENAI_API_KEYS_JSON/);
  assert.match(secretsTemplate, /ANTHROPIC_API_KEYS_JSON/);
  assert.match(secretsTemplate, /OPENCODE_API_KEYS_JSON/);
  assert.match(secretsTemplate, /OPENCODE_MODELS:\s*"big-pickle,deepseek-v4-flash-free,minimax-m2\.5-free,nemotron-3-super-free,qwen3\.6-plus-free"/);
  assert.match(secretsTemplate, /DEEPSEEK_API_KEYS_JSON/);
  assert.match(secretsTemplate, /DEEPSEEK_MODELS:\s*"deepseek-v4-flash,deepseek-v4-pro"/);
  assert.match(secretsTemplate, /DASHSCOPE_API_KEYS_JSON/);
  assert.match(secretsTemplate, /QWEN_MODELS:\s*"qwen3\.6-max-preview,qwen3\.6-plus,qwen3\.6-flash"/);
  assert.match(secretsTemplate, /XAI_API_KEYS_JSON/);
  assert.match(secretsTemplate, /GROK_API_KEY and GROK_API_KEYS_JSON are accepted aliases/);
  assert.match(secretsTemplate, /XAI_MODELS:\s*"grok-4\.3"/);
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
  assert.match(dockerfile, /git clone --depth 1 --branch "\$DD_REPO_REF" "\$DD_REPO_URL" repo-template/);
  assert.match(dockerfile, /WORKSPACE_REPO=\/home\/node\/workspace\/repo/);
  assert.doesNotMatch(dockerfile, /workspace\/repo-template/);
  assert.match(entrypoint, /TEMPLATE_DIR="\$\{REPO_TEMPLATE_DIR:-\/home\/node\/repo-template\}"/);
  assert.match(entrypoint, /REPO_URL="\$\{DD_REPO_URL:-\}"/);
  assert.match(entrypoint, /DD_REPO_URL is required/);
  assert.match(entrypoint, /github_https_to_ssh\(\)/);
  assert.match(entrypoint, /GIT_REPO_URL="\$\(github_https_to_ssh "\$REPO_URL"\)"/);
  assert.match(entrypoint, /git clone --depth 1 --branch "\$BASE_BRANCH" "\$GIT_REPO_URL" "\$REPO_DIR"/);
  assert.match(entrypoint, /git remote set-url origin "\$GIT_REPO_URL"/);
  assert.match(entrypoint, /git fetch --quiet --depth=1 origin "\+refs\/heads\/\$BASE_BRANCH:refs\/remotes\/origin\/\$BASE_BRANCH"/);
  assert.match(entrypoint, /if \[\[ ! -d "\$REPO_DIR\/\.git" && -d "\$TEMPLATE_DIR\/\.git" \]\]; then/);
  assert.match(entrypoint, /cp -a "\$TEMPLATE_DIR\/\." "\$REPO_DIR\/"/);
  assert.match(entrypoint, /==> git fetch starting/);
  assert.match(entrypoint, /git fetch --quiet --depth=1 origin "\+refs\/heads\/\$BASE_BRANCH:refs\/remotes\/origin\/\$BASE_BRANCH"/);
  assert.doesNotMatch(entrypoint, /GIT_READY_PID/);

  assert.match(readme, /Runtime clone or baked-template clone uses `git clone --depth=1 --branch <BASE_BRANCH>`/);
  assert.match(readme, /Warm boots only refresh `origin\/<BASE_BRANCH>` with a depth-1 fetch/);
  assert.match(readme, /switch from it;[\s\S]*otherwise create the feature branch from[\s\S]*`origin\/<BASE_BRANCH>`/);
  assert.match(readme, /If a reused workspace is still on the parent branch, the worker fails[\s\S]*closed/);
  assert.match(readme, /Install repo dependencies only after the feature branch is prepared/);
  assert.match(readme, /Before the first build you need a `pnpm-lock\.yaml`/);
  assert.match(readme, /cluster-mcp\.ts/);
  assert.match(readme, /AGENT_MCP_URL/);
  assert.match(readme, /REPO_CONTEXT_MAX_CHARS/);
  assert.match(readme, /AGENT_OPTIMISTIC_MODE/);
  assert.match(readme, /AGENTS\.md`\/`agents\/\*\.md`\/`docs\/\*\.md/);
  assert.match(readme, /local `tmp\/convos\/thread\.log` tail/);
  assert.match(readme, /Generic AI SDK and OpenCode receive bounded workspace\s+tools/);
  assert.match(readme, /dd_cluster/);
  assert.match(readme, /CLI runners still get the prompt hint/);
  assert.match(agentsMd, /docs\/agent-context-memory\.md/);
  assert.match(agentsMd, /tmp\/convos\/thread\.log/);
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
  assert.match(deployment, /name:\s*AGENT_MCP_URL[\s\S]*dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090\/mcp/);
  assert.match(deployment, /name:\s*AGENT_MCP_CONNECT_TIMEOUT_MS[\s\S]*value:\s*"3000"/);
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
  assert.match(restServer, /"AGENT_MCP_URL", "value": "http:\/\/dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090\/mcp"/);
  assert.match(restServer, /"AGENT_MCP_CONNECT_TIMEOUT_MS", "value": "3000"/);
  assert.match(restServer, /"NATS_EVENT_SUBJECT", "value": "dd\.remote\.events"/);
  assert.match(restServer, /"envFrom": \[/);
  assert.match(
    restServer,
    /"secretRef": \{ "name": "dd-agent-secrets", "optional": true \}/,
  );
  assert.doesNotMatch(restServer, /git .* clone --depth 1 --branch dev/);
  assert.doesNotMatch(restServer, /apt-get update/);
  assert.doesNotMatch(localMinikube, /AGENT_MCP_URL|AGENT_MCP_CONNECT_TIMEOUT_MS/);
  assert.match(agentsMd, /docs\/\*\.md/);
  assert.match(agentsMd, /agents\/\*\.md/);
  assert.match(agentsMd, /dd_cluster/);
  assert.match(agentsMd, /WireGuard VPN plus `dd-bastion`/);
});
