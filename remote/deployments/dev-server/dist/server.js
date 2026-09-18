/* eslint-disable security/detect-non-literal-fs-filename -- remote-dev manages configured workspace, log, and artifact paths. */
// dd-dev-server — API/worker runtime that runs Claude/OpenAI coding agents
// inside a warm configured git workspace, then streams events.
//
// Endpoints (all auth'd via X-Server-Auth header except /healthz):
//   POST /tasks                  — { taskId?, threadId?, prompt } → queues task
//   GET  /stream/:taskId         — Server-Sent Events of agent activity
//   GET  /ws                     — WebSocket replay/live stream for pinned thread tasks
//   POST /tasks/:taskId/cancel   — abort an in-flight task
//   POST /thread/merge-upstream  — merge configured base branch into the pinned thread branch
//   POST /thread/make-commit     — commit current workspace changes and push the thread branch
//   POST /thread/open-pr         — explicitly open/reuse a draft WIP PR
//   GET  /terminal               — browser terminal for the pinned worker container
//   GET  /healthz                — liveness probe
//
// Per thread, the server prepares/reuses a stable branch
// agent/k8s/openai-5.5/<threadId>/<slugified-title>.
// Per task, it runs the selected provider, streams sequenced events,
// posts breadcrumbs to rest-api (persisted in Postgres
// `agent_remote_dev_breadcrumbs`; see @dd/redis-interfaces for the
// optional cache shape), and pushes the branch. PR creation is an
// explicit UI/API action.
//
// Worktrees + finished tasks are GC'd one hour after completion.
import Fastify from 'fastify';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { basename, dirname, isAbsolute, join, normalize, relative, resolve, sep } from 'node:path';
import { access, appendFile, mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { ReplaySubject, Subject, interval } from 'rxjs';
import { filter, takeUntil } from 'rxjs/operators';
import { z } from 'zod';
import { EventBus } from './event-bus.js';
import { containerPoolConfigFromEnv, containerPoolConfigured, dispatchContainerPool, } from './container-pool.js';
import { buildAgentEnvCandidates, getCachedAvailability, getRunner, probeAllProviders, resolveAgentProvider, } from './agents/index.js';
import { publishArtifact } from './storage/index.js';
import { acquireUserChannel, destroyChannelPool, isRealtimeEnabled, publishUserEvent, releaseUserChannel, } from './realtime.js';
import { initTelemetry, shutdownTelemetry, withSpan } from './telemetry.js';
import { verifyDirectStreamToken } from './token.js';
import { NatsPublisher } from './nats-publisher.js';
import { WorkerFanoutWebSocket, workerFanoutWsUrlFromEnv } from './ws-fanout.js';
import { clusterMcpPromptSection } from './agents/cluster-mcp.js';
import { registerRuntimeConfigRoutes, registerWithControlPlane } from './runtime-config.js';
import { annotateError, getRequestContext, readErrorRequestContext, runWithRequestContext, setContextField, snapshotRequestContext, } from './request-context.js';
import { contextFetch } from './wrapped-fetch.js';
// ---------- Config ----------
const DEFAULT_AGENT_PROVIDER = 'generic-ai-sdk';
const AGENT_FALLBACK_PROVIDER = 'generic-ai-sdk';
const AGENT_SECONDARY_FALLBACK_PROVIDER = 'opencode-ai-sdk';
const CONFIG_AGENT_PROVIDERS = new Set([
    'claude-cli',
    'claude-sdk',
    'generic-ai-sdk',
    'gemini-sdk',
    'opencode-ai-sdk',
    'openai-codex-cli',
    'openai-sdk',
]);
// Paths the dev-server owns by contract and must never treat as user
// repo content (commits, dirty-state guards, install-artifact cleans):
// dependency caches that should not pollute commits or dirty checks. The
// per-thread breadcrumb log is no longer written into the workspace; it
// is sent to rest-api and persisted in Postgres via
// agent_remote_dev_breadcrumbs (see @dd/redis-interfaces +
// remote/libs/pg-defs).
const GENERATED_GIT_EXCLUDE_PATHS = [
    '.pnpm-store',
    'node_modules',
    '.next',
    '.turbo',
];
const GENERATED_GIT_STATUS_EXCLUDES = GENERATED_GIT_EXCLUDE_PATHS.map((path) => `:(exclude)${path}`);
const GENERATED_GIT_CLEAN_EXCLUDE_FLAGS = GENERATED_GIT_EXCLUDE_PATHS.flatMap((path) => [
    '--exclude',
    path,
]);
function configAgentProvider(value, fallback) {
    return value && CONFIG_AGENT_PROVIDERS.has(value) ? value : fallback;
}
// Normalize a git remote so that equivalent forms compare equal:
//   git@github.com:Owner/Repo.git  <->  https://github.com/Owner/Repo
//   git+https://github.com/Owner/Repo.git  <->  https://github.com/owner/repo.git
// Returns null for an empty/unparseable input. Host and owner-or-org/path are
// lowercased; trailing `.git` and trailing `/` are stripped. Userinfo is
// dropped so credentials embedded in the URL never affect equality.
function canonicalRepoKey(input) {
    if (!input)
        return null;
    let value = String(input).trim();
    if (!value)
        return null;
    value = value.replace(/^git\+/i, '');
    const scpMatch = value.match(/^([^@\s]+)@([^:\s]+):(.+)$/);
    if (scpMatch && scpMatch[2] && scpMatch[3]) {
        const host = scpMatch[2].toLowerCase();
        const pathPart = scpMatch[3].replace(/^\/+/, '');
        value = `https://${host}/${pathPart}`;
    }
    try {
        const url = new URL(value);
        const host = url.hostname.toLowerCase();
        let pathPart = url.pathname.replace(/^\/+/, '').replace(/\/+$/, '');
        if (pathPart.toLowerCase().endsWith('.git')) {
            pathPart = pathPart.slice(0, -4);
        }
        return `${host}/${pathPart.toLowerCase()}`;
    }
    catch {
        let stripped = value.replace(/\/+$/, '');
        if (stripped.toLowerCase().endsWith('.git')) {
            stripped = stripped.slice(0, -4);
        }
        return stripped.toLowerCase();
    }
}
function repoUrlsMatch(a, b) {
    const ka = canonicalRepoKey(a);
    const kb = canonicalRepoKey(b);
    return ka !== null && kb !== null && ka === kb;
}
function configAgentProviderList(value, fallback) {
    const requested = value
        ? value
            .split(/[,\s]+/)
            .map((item) => item.trim())
            .filter(Boolean)
        : fallback;
    const seen = new Set();
    const providers = [];
    for (const item of requested) {
        if (CONFIG_AGENT_PROVIDERS.has(item) && !seen.has(item)) {
            seen.add(item);
            providers.push(item);
        }
    }
    return providers.length > 0 ? providers : fallback;
}
const configuredAgentFallbackProvider = configAgentProvider(process.env.AGENT_FALLBACK_PROVIDER, AGENT_FALLBACK_PROVIDER);
const configuredAgentSecondaryFallbackProvider = configAgentProvider(process.env.AGENT_SECONDARY_FALLBACK_PROVIDER, AGENT_SECONDARY_FALLBACK_PROVIDER);
const config = {
    port: Number(process.env.PORT ?? 8080),
    host: process.env.HOST ?? '0.0.0.0',
    workspaceRepo: process.env.WORKSPACE_REPO ?? '/home/node/workspace/repo',
    repoUrl: process.env.DD_REPO_URL ?? null,
    // Per-thread pods set REMOTE_DEV_THREAD_ID. Repo-scoped warm pool workers
    // leave it unset and accept tasks for any thread in the configured repo.
    threadId: process.env.REMOTE_DEV_THREAD_ID ?? process.env.THREAD_ID ?? null,
    threadTitle: process.env.REMOTE_DEV_THREAD_TITLE ?? process.env.THREAD_TITLE ?? null,
    workerBindMode: process.env.WORKER_BIND_MODE ?? (process.env.REMOTE_DEV_THREAD_ID || process.env.THREAD_ID ? 'thread' : 'repo'),
    userId: process.env.USER_ID ?? null,
    // Each agent run writes publishable files into ${OUTPUTS_DIR}/<taskId>/.
    // After claude exits, runTask scans that dir and uploads each file via
    // the configured storage adapter, emitting an `artifact` event per file.
    outputsDir: process.env.OUTPUTS_DIR ?? '/home/node/workspace/outputs',
    defaultStorageProvider: (process.env.DEFAULT_STORAGE_PROVIDER ?? 'local'),
    // Periodic heartbeat to Vercel — lets the UI poll an "is the docker
    // alive?" endpoint backed by our most recent ping. Disabled if
    // HEARTBEAT_URL is unset.
    heartbeatUrl: process.env.HEARTBEAT_URL ?? null,
    heartbeatSecret: process.env.HEARTBEAT_SECRET ?? null,
    heartbeatIntervalMs: Number(process.env.HEARTBEAT_INTERVAL_MS ?? 20_000),
    idleTimeoutMs: Number(process.env.IDLE_TIMEOUT_MS ?? 30 * 60 * 1000),
    agentRunTimeoutMs: Number(process.env.AGENT_RUN_TIMEOUT_MS ?? 2 * 60 * 60_000),
    ghDeployKeyPath: process.env.GH_DEPLOY_KEY_PATH ?? '/home/node/.ssh/id_ed25519',
    ghDeployKey: process.env.GH_DEPLOY_KEY ?? null,
    ghPat: process.env.GH_PAT ?? null,
    anthropicApiKey: process.env.ANTHROPIC_API_KEY ?? null,
    serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
    eventIngestUrl: process.env.EVENT_INGEST_URL ?? null,
    eventIngestSecret: process.env.EVENT_INGEST_SECRET ?? null,
    natsUrl: process.env.NATS_URL ?? null,
    natsEventSubject: process.env.NATS_EVENT_SUBJECT ?? 'dd.remote.events',
    workerFanoutWsUrl: workerFanoutWsUrlFromEnv(process.env),
    workerFanoutWsMaxQueueDepth: Number(process.env.WORKER_FANOUT_WS_MAX_QUEUE_DEPTH ?? 500),
    workerFanoutWsReconnectMs: Number(process.env.WORKER_FANOUT_WS_RECONNECT_MS ?? 1000),
    threadContextBaseUrl: process.env.THREAD_CONTEXT_BASE_URL ??
        process.env.REMOTE_REST_API_BASE_URL ??
        'http://dd-remote-rest-api.default.svc.cluster.local:8082',
    threadContextLimit: Number(process.env.THREAD_CONTEXT_LIMIT ?? 20),
    threadContextMaxChars: Number(process.env.THREAD_CONTEXT_MAX_CHARS ?? 48_000),
    repoContextMaxChars: Number(process.env.REPO_CONTEXT_MAX_CHARS ?? 24_000),
    agentOptimisticMode: process.env.AGENT_OPTIMISTIC_MODE !== 'false',
    agentMcpUrl: process.env.AGENT_MCP_ENABLED === 'false' ? null : process.env.AGENT_MCP_URL ?? null,
    agentFallbackProvider: configuredAgentFallbackProvider,
    agentSecondaryFallbackProvider: configuredAgentSecondaryFallbackProvider,
    agentProviderRotation: configAgentProviderList(process.env.AGENT_PROVIDER_ROTATION, [
        'generic-ai-sdk',
        'opencode-ai-sdk',
        'openai-sdk',
        'claude-sdk',
        'gemini-sdk',
    ]),
    agentBranchPrefix: process.env.AGENT_BRANCH_PREFIX ?? 'agent/k8s/openai-5.5',
    baseBranch: process.env.BASE_BRANCH ?? 'dev',
    // Breadcrumb writes still go to rest-api-rs in the background. Tail reads
    // are no longer pulled from this server: prompt context for breadcrumbs is
    // selected via the /agents/threads picker and arrives in `contextBlobs`.
    breadcrumbWriteTimeoutMs: Number(process.env.THREAD_BREADCRUMB_WRITE_TIMEOUT_MS ?? 5_000),
    skipBootGitSync: process.env.SKIP_BOOT_GIT_SYNC === 'true',
    sessionIdleGcAfterMs: Number(process.env.SESSION_IDLE_GC_AFTER_MS ?? 6 * 60 * 60 * 1000),
    prAuthor: {
        name: process.env.GIT_AUTHOR_NAME ?? 'DD Agent',
        email: process.env.GIT_AUTHOR_EMAIL ?? 'agent@dancingdragons.dev',
    },
    logDir: process.env.LOG_DIR ?? '/tmp/convos',
    processedTasksDir: process.env.PROCESSED_TASKS_DIR ?? join(process.env.LOG_DIR ?? '/tmp/convos', 'processed-tasks'),
    containerPool: containerPoolConfigFromEnv(process.env),
    taskGcAfterMs: 60 * 60 * 1000, // 1 hour
    taskGcIntervalMs: 5 * 60 * 1000, // 5 min sweep
};
const tasks = new Map();
const sessions = new Map();
const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();
// ---- RxJS EventBus — reactive pipeline for events ----
const eventBus = new EventBus();
const natsPublisher = new NatsPublisher(config.natsUrl);
const workerFanout = new WorkerFanoutWebSocket({
    url: config.workerFanoutWsUrl,
    logger: console,
    maxQueueDepth: config.workerFanoutWsMaxQueueDepth,
    reconnectMs: config.workerFanoutWsReconnectMs,
});
// ---------- Helpers ----------
function pruneSessionQueueState(session) {
    if (session.runningTaskId) {
        const running = tasks.get(session.runningTaskId);
        if (!running || running.finished) {
            session.runningTaskId = undefined;
        }
    }
    session.queuedTaskIds = session.queuedTaskIds.filter((id) => {
        const task = tasks.get(id);
        return Boolean(task && !task.finished && session.runningTaskId !== id);
    });
}
function sessionBlockingTaskIds(session) {
    pruneSessionQueueState(session);
    const ids = [
        session.runningTaskId,
        ...session.queuedTaskIds,
    ].filter((id) => Boolean(id));
    return Array.from(new Set(ids));
}
function totalQueuedTaskCount() {
    let count = 0;
    for (const session of sessions.values()) {
        pruneSessionQueueState(session);
        count += session.queuedTaskIds.length;
    }
    return count;
}
function taskQueueSnapshot(task) {
    const session = task.session;
    pruneSessionQueueState(session);
    const queuedIndex = session.queuedTaskIds.indexOf(task.taskId);
    return {
        running: session.runningTaskId === task.taskId && !task.finished,
        queued: queuedIndex >= 0 && !task.finished,
        queuePosition: queuedIndex >= 0 ? queuedIndex + 1 : undefined,
    };
}
function shCapture(cmd, args, cwd, optsOrExtraEnv) {
    // Backwards-compat: callers passing a plain extraEnv object still work.
    const opts = optsOrExtraEnv &&
        typeof optsOrExtraEnv === 'object' &&
        ('timeoutMs' in optsOrExtraEnv ||
            'extraEnv' in optsOrExtraEnv ||
            'isolatedEnv' in optsOrExtraEnv)
        ? optsOrExtraEnv
        : { extraEnv: optsOrExtraEnv };
    const env = opts.isolatedEnv
        ? opts.isolatedEnv
        : { ...process.env, ...(opts.extraEnv ?? {}) };
    return new Promise((resolve, reject) => {
        const child = spawn(cmd, args, { cwd, env });
        let stdout = '';
        let stderr = '';
        let timedOut = false;
        let killTimer = null;
        if (opts.timeoutMs && opts.timeoutMs > 0) {
            killTimer = setTimeout(() => {
                timedOut = true;
                try {
                    child.kill('SIGKILL');
                }
                catch {
                    /* ignore */
                }
            }, opts.timeoutMs);
        }
        child.stdout.on('data', (d) => {
            stdout += d.toString('utf8');
        });
        child.stderr.on('data', (d) => {
            stderr += d.toString('utf8');
        });
        child.on('close', (code) => {
            if (killTimer) {
                clearTimeout(killTimer);
            }
            if (timedOut) {
                reject(new Error(`${cmd} ${args.join(' ')} timed out after ${opts.timeoutMs}ms`));
                return;
            }
            if (code === 0) {
                resolve(stdout);
            }
            else {
                reject(new Error(`${cmd} ${args.join(' ')} exited ${code}: ${stderr.slice(0, 1000)}`));
            }
        });
        child.on('error', (err) => {
            if (killTimer) {
                clearTimeout(killTimer);
            }
            reject(err);
        });
    });
}
// Per-operation timeouts. Network-bound work (git fetch/push, pnpm
// install) gets the most headroom; quick git plumbing gets the least.
const TIMEOUT_GIT_QUICK = 60_000; // 1 min
const TIMEOUT_GIT_NETWORK = 5 * 60_000; // 5 min
const TIMEOUT_PNPM_INSTALL = 10 * 60_000; // 10 min
const TIMEOUT_GH_PR = 60_000; // 1 min
function getSessionId(threadId, taskId) {
    return threadId ?? config.threadId ?? taskId;
}
function getSessionWorkspacePath(_sessionId) {
    // The container is pinned to one thread; every task on this thread
    // shares the same workspace.
    return config.workspaceRepo;
}
function slugifyBranchFragment(value) {
    const slug = value
        .normalize('NFKD')
        .replace(/[\u0300-\u036f]/g, '')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
        .replace(/-{2,}/g, '-')
        .slice(0, 80);
    return slug || 'thread';
}
function repoDisplayName() {
    const repo = config.repoUrl?.trim();
    if (!repo) {
        return 'unknown repo';
    }
    const githubMatch = repo.match(/github\.com[:/]([^/]+)\/([^/#?]+?)(?:\.git)?$/i);
    if (githubMatch) {
        return `${githubMatch[1]}/${githubMatch[2]}`;
    }
    return repo;
}
function assertSafeGitBranchName(branch, label = 'branch') {
    const invalid = !/^[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(branch) ||
        branch.startsWith('-') ||
        branch.startsWith('/') ||
        branch.endsWith('/') ||
        branch.endsWith('.lock') ||
        branch.includes('..') ||
        branch.includes('//') ||
        branch.includes('@{') ||
        branch.includes('\\') ||
        branch.split('/').some((part) => !part || part === '.' || part === '..' || part.endsWith('.lock'));
    if (invalid) {
        throw new Error(`invalid git ${label}: ${branch}`);
    }
}
function gitBranchTarget(branch) {
    return `${repoDisplayName()} branch ${branch}`;
}
function providerCanEditWorkspace(provider) {
    return provider !== 'gemini-sdk';
}
function providerCanAccessWorkspace(provider) {
    return providerCanEditWorkspace(provider);
}
function providerCanUseShell(provider) {
    return (provider === 'openai-sdk' ||
        provider === 'openai-codex-cli' ||
        provider === 'claude-sdk' ||
        provider === 'claude-cli');
}
function firstConfiguredModel(value) {
    return value
        ?.split(/[,\n;]/)
        .map((item) => item.trim())
        .find(Boolean);
}
function modelForAgentEnv(provider, env) {
    if (provider === 'claude-cli' || provider === 'claude-sdk') {
        return env.ANTHROPIC_MODEL;
    }
    if (provider === 'gemini-sdk') {
        return env.GEMINI_MODEL;
    }
    if (provider === 'openai-codex-cli') {
        return env.CODEX_MODEL ?? env.OPENAI_MODEL;
    }
    if (provider === 'openai-sdk') {
        return env.OPENAI_MODEL;
    }
    if (provider === 'opencode-ai-sdk') {
        return firstConfiguredModel(env.OPENCODE_MODELS ?? env.OPENCODE_MODEL);
    }
    if (provider === 'generic-ai-sdk') {
        return firstConfiguredModel(env.GENERIC_AI_SDK_MODELS);
    }
    return undefined;
}
function isAgentProviderValue(value) {
    return typeof value === 'string' && CONFIG_AGENT_PROVIDERS.has(value);
}
function modelLabel(provider, model) {
    if (!model) {
        return provider?.replace(/-sdk|-cli/g, '').replace(/-/g, ' ');
    }
    let label = model.trim();
    if (!label) {
        return undefined;
    }
    if (/^gpt-/i.test(label)) {
        return label.replace(/^gpt-/i, 'chatgpt-').replace(/_/g, ' ').replace(/\s+/g, ' ').trim().toLowerCase();
    }
    label = label
        .replace(/claude-([a-z]+)-(\d+)-(\d+)/i, 'claude $1 $2.$3')
        .replace(/([a-z])(\d)/gi, '$1 $2')
        .replace(/[_-]+/g, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .toLowerCase();
    return label;
}
function stripNegatedPullRequestPhrases(prompt) {
    return prompt
        .replace(/\b(?:do\s+not|don't|dont|never)\s+(?:(?:open|create|submit|make|raise)\s+)?(?:a\s+)?(?:draft\s+)?(?:pr|pull\s+request|merge\s+request)\b/gi, ' ')
        .replace(/\b(?:without|no)\s+(?:(?:opening|creating|submitting|making|raising)\s+)?(?:a\s+)?(?:draft\s+)?(?:pr|pull\s+request|merge\s+request)s?\b/gi, ' ')
        .replace(/\b(?:without|no)\s+(?:pr|pull\s+request|merge\s+request)s?\b/gi, ' ');
}
function promptRequestsPullRequest(prompt) {
    const prPrompt = stripNegatedPullRequestPhrases(prompt);
    return /\b(?:open|create|submit|make|raise)\s+(?:a\s+)?(?:draft\s+)?(?:pr|pull\s+request|merge\s+request)\b/i.test(prPrompt) || /\b(?:pr|pull\s+request|merge\s+request)\b/i.test(prPrompt);
}
function stripNegatedWorkspaceChangePhrases(prompt) {
    return prompt
        .replace(/\b(?:do\s+not|don't|dont|never)\s+(?:make\s+)?(?:any\s+)?(?:file|code|workspace|repo(?:sitory)?|source)?\s*(?:changes?|edits?|modifications?)\b/gi, ' ')
        .replace(/\b(?:do\s+not|don't|dont|never)\s+(?:edit|change|modify|write|update|create|delete|remove|patch|fix|implement)(?:\s+(?:the|a|any|files?|code|workspace|repo(?:sitory)?|source|project|readme(?:\.md)?|package(?:\.json)?)){0,4}\b/gi, ' ')
        .replace(/\b(?:without|no)\s+(?:making\s+)?(?:any\s+)?(?:file|code|workspace|repo(?:sitory)?|source)?\s*(?:changes?|edits?|modifications?)\b/gi, ' ');
}
function promptLikelyRequiresWorkspaceChange(prompt) {
    const editablePrompt = stripNegatedWorkspaceChangePhrases(prompt);
    return /\b(add|append|change|create|delete|edit|fix|implement|modify|move|patch|refactor|remove|rename|replace|update|write)\b/i.test(editablePrompt);
}
function promptLikelyRequiresWorkspaceAccess(prompt) {
    const workspacePrompt = stripNegatedWorkspaceChangePhrases(prompt);
    if (promptLikelyRequiresWorkspaceChange(prompt)) {
        return true;
    }
    if (/\b(readme(?:\.md)?|package(?:\.json)?|pnpm-lock|dockerfile|makefile|tsconfig|cargo\.toml|go\.mod)\b/i.test(workspacePrompt)) {
        return true;
    }
    const hasWorkspaceNoun = /\b(repo(?:sitory)?|codebase|workspace|working tree|source tree|folders?|directories|dirs?|files?|top[- ]level|root)\b/i.test(workspacePrompt);
    const hasInspectionVerb = /\b(count|find|grep|how many|inspect|list|look|open|read|search|show|tree|what|where|which)\b/i.test(workspacePrompt);
    return hasWorkspaceNoun && hasInspectionVerb;
}
function promptLikelyRequiresShellAccess(prompt) {
    const workspacePrompt = stripNegatedWorkspaceChangePhrases(prompt);
    return (/\bgit\s+(?:fetch|merge|push|commit|branch)\b/i.test(workspacePrompt) ||
        /\b(?:fetch|push)\s+(?:origin|the\s+current\s+branch|current\s+branch|branches?)\b/i.test(workspacePrompt) ||
        /\bcommit\s+(?:the\s+integrated\s+result|current\s+changes?|workspace\s+changes?|merge\s+result)\b/i.test(workspacePrompt) ||
        /\bmerge\s+(?:with\s+)?sibling\b/i.test(workspacePrompt) ||
        /\bsibling\s+feature\s+branches?\b/i.test(workspacePrompt));
}
function parseDeterministicAppendFilePrompt(prompt) {
    const quoted = prompt.match(/\b(?:append(?:ing)?|add(?:ing)?)\s+(?:"([^"]+)"|'([^']+)'|`([^`]+)`)\s+(?:to|into)\s+(?:the\s+)?(?:file\s+)?([A-Za-z0-9][A-Za-z0-9._/-]*)(?:\b|$)/i);
    if (quoted) {
        return {
            action: 'append-file',
            text: quoted[1] ?? quoted[2] ?? quoted[3] ?? '',
            relativePath: quoted[4],
        };
    }
    const unquoted = prompt.match(/\b(?:append(?:ing)?|add(?:ing)?)\s+([A-Za-z0-9][A-Za-z0-9._-]*)\s+(?:to|into)\s+(?:the\s+)?(?:file\s+)?([A-Za-z0-9][A-Za-z0-9._/-]*)(?:\b|$)/i);
    if (!unquoted) {
        return null;
    }
    return {
        action: 'append-file',
        text: unquoted[1],
        relativePath: unquoted[2],
    };
}
function safeRepoRelativePath(workspacePath, rawPath) {
    const trimmed = rawPath.trim().replace(/^[.][/\\]+/, '').replace(/[),.;:]+$/g, '');
    const normalized = normalize(trimmed);
    if (!trimmed || trimmed.includes('\0') || isAbsolute(trimmed) || normalized === '.' || normalized === '..') {
        throw new Error(`refusing unsafe deterministic append path: ${rawPath}`);
    }
    if (normalized.startsWith(`..${sep}`) || normalized.split(sep).some((part) => part === '..')) {
        throw new Error(`refusing deterministic append outside ${repoDisplayName()}: ${rawPath}`);
    }
    const blockedSegments = new Set(['.git', 'node_modules', '.pnpm-store', '.next', '.turbo']);
    const segments = normalized.split(sep).filter(Boolean);
    if (segments.some((part) => blockedSegments.has(part))) {
        throw new Error(`refusing deterministic append into generated or git-managed path: ${rawPath}`);
    }
    const workspaceRoot = resolve(workspacePath);
    const resolvedTarget = resolve(workspaceRoot, normalized);
    const repoRelative = relative(workspaceRoot, resolvedTarget);
    if (!repoRelative || repoRelative.startsWith(`..${sep}`) || isAbsolute(repoRelative)) {
        throw new Error(`refusing deterministic append outside ${repoDisplayName()}: ${rawPath}`);
    }
    return repoRelative.split(sep).join('/');
}
async function applyDeterministicWorkspaceEdit(state) {
    const appendEdit = parseDeterministicAppendFilePrompt(state.prompt);
    if (!appendEdit) {
        return null;
    }
    const relativePath = safeRepoRelativePath(state.worktreePath, appendEdit.relativePath);
    const targetPath = resolve(state.worktreePath, relativePath);
    await mkdir(dirname(targetPath), { recursive: true });
    let existing = '';
    try {
        existing = await readFile(targetPath, 'utf8');
    }
    catch (err) {
        if (!(err instanceof Error && 'code' in err && err.code === 'ENOENT')) {
            throw err;
        }
    }
    const prefix = existing.length > 0 && !existing.endsWith('\n') ? '\n' : '';
    const suffix = appendEdit.text.endsWith('\n') ? '' : '\n';
    await appendFile(targetPath, `${prefix}${appendEdit.text}${suffix}`, 'utf8');
    const result = {
        action: 'append-file',
        relativePath,
        appendedChars: appendEdit.text.length,
    };
    void postTaskBreadcrumb(state, 'deterministic-edit', {
        action: result.action,
        relativePath: result.relativePath,
        appendedChars: result.appendedChars,
    });
    emit(state, {
        kind: 'status',
        status: 'deterministic-edit:append-file',
        message: `Appended ${result.appendedChars} character(s) to ${relativePath} in ${repoDisplayName()}.\n` +
            `Workspace: ${gitBranchTarget(state.branch)}`,
    });
    return result;
}
async function gitWorkspaceStatus(workspacePath) {
    return shCapture('git', ['status', '--porcelain', '--untracked-files=all', '--', '.', ...GENERATED_GIT_STATUS_EXCLUDES], workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
}
async function fetchRemoteBranch(workspacePath, branch, depth = 1) {
    assertSafeGitBranchName(branch, 'remote branch');
    await shCapture('git', [
        'fetch',
        '--quiet',
        '--prune',
        `--depth=${depth}`,
        'origin',
        `+refs/heads/${branch}:refs/remotes/origin/${branch}`,
    ], workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
}
async function deepenRemoteBranch(workspacePath, branch, commits) {
    assertSafeGitBranchName(branch, 'remote branch');
    await shCapture('git', [
        'fetch',
        '--quiet',
        `--deepen=${commits}`,
        'origin',
        `+refs/heads/${branch}:refs/remotes/origin/${branch}`,
    ], workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
}
async function currentGitBranch(workspacePath) {
    try {
        const out = await shCapture('git', ['symbolic-ref', '--quiet', '--short', 'HEAD'], workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
        return out.trim() || null;
    }
    catch {
        return null;
    }
}
async function currentGitCommit(workspacePath) {
    return (await shCapture('git', ['rev-parse', 'HEAD'], workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    })).trim();
}
async function assertSessionOnFeatureBranch(session) {
    if (session.branch === config.baseBranch || session.branch === `origin/${config.baseBranch}`) {
        throw new Error(`refusing to run ${session.sessionId} on parent branch ${config.baseBranch}; expected a feature branch`);
    }
    const branch = await currentGitBranch(session.workspacePath);
    if (branch === config.baseBranch) {
        throw new Error(`workspace is still on parent branch ${config.baseBranch}; refusing to start queued task for ${session.branch}`);
    }
    if (branch !== session.branch) {
        const commit = await currentGitCommit(session.workspacePath).catch(() => 'unknown');
        throw new Error(`workspace branch mismatch: expected ${session.branch}, got ${branch ?? `detached at ${commit}`}`);
    }
}
async function ensureMergeBaseWithBaseBranch(session) {
    const deepenSteps = [50, 200, 1000];
    for (let attempt = 0; attempt <= deepenSteps.length; attempt += 1) {
        try {
            await shCapture('git', ['merge-base', 'HEAD', `origin/${config.baseBranch}`], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
            return;
        }
        catch (err) {
            if (attempt >= deepenSteps.length) {
                throw err;
            }
            const deepenBy = deepenSteps[attempt];
            await deepenRemoteBranch(session.workspacePath, config.baseBranch, deepenBy);
            if (await remoteBranchExists(session.branch)) {
                await deepenRemoteBranch(session.workspacePath, session.branch, deepenBy);
            }
        }
    }
}
async function gitUnmergedFiles(workspacePath) {
    const out = await shCapture('git', ['diff', '--name-only', '--diff-filter=U'], workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    return out
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean);
}
async function abortMergeIfInProgress(workspacePath) {
    try {
        await shCapture('git', ['rev-parse', '--quiet', '--verify', 'MERGE_HEAD'], workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
    }
    catch {
        return;
    }
    try {
        await shCapture('git', ['merge', '--abort'], workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
    }
    catch (err) {
        process.stderr.write(`[remote-dev] merge --abort failed: ${err instanceof Error ? err.message : String(err)}\n`);
    }
}
async function pushSessionBranch(session) {
    assertSafeGitBranchName(session.branch, 'session branch');
    await shCapture('git', ['push', '--no-verify', '--set-upstream', 'origin', session.branch], session.workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
}
async function gitAddWorkspaceChanges(workspacePath) {
    await shCapture('git', ['add', '-A', '--', '.'], workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture('git', ['reset', '-q', 'HEAD', '--', ...GENERATED_GIT_EXCLUDE_PATHS], workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
}
async function resetDependencyInstallArtifacts(workspacePath) {
    await shCapture('git', ['restore', '--staged', '--worktree', '--', '.'], workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture('git', ['clean', '-fdx', ...GENERATED_GIT_CLEAN_EXCLUDE_FLAGS], workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
}
function getSessionBranch(sessionId, branchHint, titleHint, promptHint) {
    const hinted = branchHint?.trim();
    if (hinted) {
        assertSafeGitBranchName(hinted, 'session branch');
        return hinted;
    }
    const titleSlug = slugifyBranchFragment(titleHint?.trim() || promptHint?.trim() || sessionId);
    const branch = `${config.agentBranchPrefix}/${sessionId}/${titleSlug}`;
    assertSafeGitBranchName(branch, 'session branch');
    return branch;
}
function isPlaceholderSessionBranch(sessionId, branch) {
    return branch === getSessionBranch(sessionId);
}
async function remoteBranchExists(branch) {
    assertSafeGitBranchName(branch, 'remote branch');
    const out = await shCapture('git', ['ls-remote', '--heads', 'origin', branch], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_NETWORK });
    return out.trim().length > 0;
}
async function installWorkspaceDependencies(workspacePath) {
    try {
        await access(join(workspacePath, 'package.json'));
    }
    catch {
        return { ok: true };
    }
    try {
        await shCapture('pnpm', ['install', '--frozen-lockfile'], workspacePath, {
            timeoutMs: TIMEOUT_PNPM_INSTALL,
        });
        return { ok: true };
    }
    catch (err) {
        const frozenMessage = err instanceof Error ? err.message : String(err);
        process.stderr.write(`[remote-dev] frozen pnpm install failed: ${frozenMessage}\n`);
        try {
            await shCapture('pnpm', ['install', '--no-frozen-lockfile'], workspacePath, {
                timeoutMs: TIMEOUT_PNPM_INSTALL,
            });
            return { ok: true, error: frozenMessage };
        }
        catch (fallbackErr) {
            const fallbackMessage = fallbackErr instanceof Error ? fallbackErr.message : String(fallbackErr);
            process.stderr.write(`[remote-dev] fallback pnpm install failed: ${fallbackMessage}\n`);
            return { ok: false, error: `${frozenMessage}; fallback: ${fallbackMessage}` };
        }
    }
}
async function prepareSessionWorkspace(session) {
    if (config.threadId && session.sessionId !== config.threadId) {
        throw new Error(`container is pinned to thread ${config.threadId}, got ${session.sessionId}`);
    }
    if (config.skipBootGitSync) {
        await configureGitIdentity(session.workspacePath);
        void postSessionBreadcrumb(session, 'session-ready', {
            workspacePath: session.workspacePath,
            repo: config.repoUrl,
            baseBranch: config.baseBranch,
            skippedBootGitSync: true,
        });
        return;
    }
    await fetchRemoteBranch(config.workspaceRepo, config.baseBranch, 1);
    const hasRemoteBranch = await remoteBranchExists(session.branch);
    const switchSource = hasRemoteBranch ? `origin/${session.branch}` : `origin/${config.baseBranch}`;
    if (hasRemoteBranch) {
        await fetchRemoteBranch(config.workspaceRepo, session.branch, 1);
    }
    const currentBranch = await currentGitBranch(session.workspacePath);
    if (currentBranch !== session.branch) {
        const status = await gitWorkspaceStatus(session.workspacePath);
        if (status.trim()) {
            throw new Error(`workspace has uncommitted changes while on ${currentBranch ?? 'detached HEAD'}; refusing to switch to ${session.branch}`);
        }
        await shCapture('git', [
            'switch',
            '--discard-changes',
            '-C',
            session.branch,
            switchSource,
        ], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_QUICK });
    }
    await assertSessionOnFeatureBranch(session);
    const preInstallStatus = await gitWorkspaceStatus(session.workspacePath);
    if (preInstallStatus.trim()) {
        throw new Error(`workspace has uncommitted changes before dependency preparation on ${session.branch}; refusing to discard them`);
    }
    const installResult = await installWorkspaceDependencies(session.workspacePath);
    await resetDependencyInstallArtifacts(session.workspacePath);
    await configureGitIdentity(session.workspacePath);
    void postSessionBreadcrumb(session, 'session-ready', {
        workspacePath: session.workspacePath,
        repo: config.repoUrl,
        baseBranch: config.baseBranch,
        dependencyInstallOk: installResult.ok,
        dependencyInstallError: installResult.error,
    });
}
function getOrCreateSession(input) {
    const sessionId = getSessionId(input.threadId, input.taskId);
    const desiredBranch = getSessionBranch(sessionId, input.branch, input.threadTitle, input.prompt);
    const existing = sessions.get(sessionId);
    if (existing) {
        existing.lastActiveAt = Date.now();
        if (!existing.userId && input.userId) {
            existing.userId = input.userId;
        }
        if (existing.taskIds.size === 0 &&
            existing.branch !== desiredBranch &&
            !input.branch?.trim() &&
            isPlaceholderSessionBranch(sessionId, existing.branch)) {
            existing.branch = desiredBranch;
            existing.ready = existing.ready.catch(() => undefined).then(() => prepareSessionWorkspace(existing));
        }
        return existing;
    }
    const workspacePath = getSessionWorkspacePath(sessionId);
    const session = {
        sessionId,
        userId: input.userId,
        workspacePath,
        branch: desiredBranch,
        ready: Promise.resolve(),
        queue: Promise.resolve(),
        taskIds: new Set(),
        queuedTaskIds: [],
        createdAt: Date.now(),
        lastActiveAt: Date.now(),
    };
    session.ready = prepareSessionWorkspace(session);
    sessions.set(sessionId, session);
    return session;
}
async function postBreadcrumb(input) {
    if (!input.threadId)
        return;
    const base = config.threadContextBaseUrl?.replace(/\/+$/, '');
    if (!base)
        return;
    const headers = { 'content-type': 'application/json' };
    if (config.eventIngestSecret) {
        headers.authorization = `Bearer ${config.eventIngestSecret}`;
    }
    else if (config.serverAuthSecret) {
        headers.authorization = `Bearer ${config.serverAuthSecret}`;
    }
    try {
        await contextFetch(`${base}/api/agents/threads/${encodeURIComponent(input.threadId)}/breadcrumbs`, {
            method: 'POST',
            headers,
            body: JSON.stringify({
                threadId: input.threadId,
                taskId: input.taskId ?? null,
                kind: input.kind,
                payload: sanitizeBreadcrumbPayload(input.payload),
                podName: process.env.HOSTNAME ?? null,
                branch: input.branch ?? null,
                provider: input.provider ?? null,
            }),
            signal: AbortSignal.timeout(config.breadcrumbWriteTimeoutMs),
        });
    }
    catch (err) {
        process.stderr.write(`[remote-dev breadcrumb] post failed (kind=${input.kind}, thread=${input.threadId}): ${err instanceof Error ? err.message : String(err)}\n`);
    }
}
function sanitizeBreadcrumbPayload(payload) {
    const text = JSON.stringify(payload);
    const scrubbed = sanitizeEventText(text);
    if (scrubbed === text)
        return payload;
    try {
        return JSON.parse(scrubbed);
    }
    catch {
        return { redacted: 'breadcrumb payload contained sensitive content; replaced with placeholder' };
    }
}
async function postSessionBreadcrumb(session, kind, payload) {
    const threadId = config.threadId ?? session.sessionId;
    return postBreadcrumb({
        threadId,
        taskId: undefined,
        kind,
        payload: { sessionId: session.sessionId, branch: session.branch, ...payload },
        branch: session.branch,
    });
}
async function postTaskBreadcrumb(state, kind, payload) {
    return postBreadcrumb({
        threadId: state.threadId,
        taskId: state.taskId,
        kind,
        payload,
        branch: state.branch,
        provider: state.provider,
    });
}
function taskReceiptPath(taskId) {
    return join(config.processedTasksDir, `${basename(taskId)}.json`);
}
async function readTaskReceipt(taskId) {
    try {
        return JSON.parse(await readFile(taskReceiptPath(taskId), 'utf8'));
    }
    catch {
        return undefined;
    }
}
async function writeTaskReceipt(receipt) {
    await mkdir(config.processedTasksDir, { recursive: true });
    await writeFile(taskReceiptPath(receipt.taskId), `${JSON.stringify(receipt, null, 2)}\n`);
}
function sanitizeEventText(value) {
    let output = value;
    for (const key of [
        'OPENAI_API_KEY',
        'OPENAI_API_KEYS',
        'OPENAI_API_KEYS_JSON',
        'ANTHROPIC_API_KEY',
        'ANTHROPIC_API_KEYS',
        'ANTHROPIC_API_KEYS_JSON',
        'CLAUDE_API_KEY',
        'CLAUDE_API_KEYS',
        'CLAUDE_API_KEYS_JSON',
        'GEMINI_API_KEY',
        'GEMINI_API_KEYS',
        'GEMINI_API_KEYS_JSON',
        'GOOGLE_API_KEY',
        'GOOGLE_API_KEYS',
        'GOOGLE_API_KEYS_JSON',
        'OPENCODE_API_KEY',
        'OPENCODE_API_KEYS',
        'OPENCODE_API_KEYS_JSON',
        'OPENCODE_ZEN_API_KEY',
        'OPENCODE_ZEN_API_KEYS',
        'OPENCODE_ZEN_API_KEYS_JSON',
        'DEEPSEEK_API_KEY',
        'DEEPSEEK_API_KEYS',
        'DEEPSEEK_API_KEYS_JSON',
        'DASHSCOPE_API_KEY',
        'DASHSCOPE_API_KEYS',
        'DASHSCOPE_API_KEYS_JSON',
        'QWEN_API_KEY',
        'QWEN_API_KEYS',
        'QWEN_API_KEYS_JSON',
        'ALIBABA_API_KEY',
        'ALIBABA_API_KEYS',
        'ALIBABA_API_KEYS_JSON',
        'XAI_API_KEY',
        'XAI_API_KEYS',
        'XAI_API_KEYS_JSON',
        'GROK_API_KEY',
        'GROK_API_KEYS',
        'GROK_API_KEYS_JSON',
        'GH_PAT',
        'GH_DEPLOY_KEY',
        'SERVER_AUTH_SECRET',
        'EVENT_INGEST_SECRET',
        'SUPABASE_SERVICE_ROLE_KEY',
    ]) {
        const secret = process.env[key];
        if (secret && secret.length >= 8) {
            output = output.split(secret).join('[redacted-secret]');
        }
    }
    return output
        .replace(/\bsk-ant-[A-Za-z0-9_*.-]{8,}\b/g, '[redacted-anthropic-key]')
        .replace(/\bsk-[A-Za-z0-9_*.-]{8,}\b/g, '[redacted-openai-key]')
        .replace(/\bAIza[A-Za-z0-9_*\-]{12,}\b/g, '[redacted-google-key]')
        .replace(/\bsk-oc-[A-Za-z0-9_*.-]{8,}\b/g, '[redacted-opencode-key]')
        .replace(/\bxai-[A-Za-z0-9_*.-]{24,}\b/g, '[redacted-xai-key]')
        .replace(/\b(?:ghp|github_pat)_[A-Za-z0-9_*.-]{8,}\b/g, '[redacted-github-token]');
}
function compactAgentErrorMessage(provider, err) {
    const raw = sanitizeEventText(err instanceof Error ? err.message : String(err));
    if (/You exceeded your current quota|insufficient_quota|quota/i.test(raw)) {
        return provider === 'gemini-sdk' ? 'quota or rate limit' : 'quota exceeded';
    }
    if (/Credit balance is too low/i.test(raw)) {
        return 'credit balance is too low';
    }
    if (/SERVICE_DISABLED|Gemini API has not been used/i.test(raw)) {
        return 'Gemini API disabled for this key project';
    }
    if (/API_KEY_HTTP_REFERRER_BLOCKED|Requests from referer/i.test(raw)) {
        return 'Google API key blocks server-side requests';
    }
    if (/API_KEY_SERVICE_BLOCKED|StreamGenerateContent are blocked|Requests to this API/i.test(raw)) {
        return 'Google API key blocks the Gemini API';
    }
    if (/API_KEY_INVALID|API key not valid/i.test(raw)) {
        return 'invalid API key';
    }
    if (/MALFORMED_FUNCTION_CALL/i.test(raw)) {
        return 'model returned malformed function call';
    }
    return raw.replace(/\s+/g, ' ').slice(0, 240);
}
function formatAgentFailureSummary(provider, failures, attempted, total) {
    const reasons = Array.from(failures.entries())
        .sort((a, b) => b[1] - a[1])
        .map(([reason, count]) => `${reason} (${count})`)
        .slice(0, 4)
        .join('; ');
    return `${provider} failed ${attempted}/${total} configured key(s): ${reasons || 'unknown error'}`;
}
function isWebSocketJsonObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function sanitizeEventValue(value) {
    if (typeof value === 'string') {
        return sanitizeEventText(value);
    }
    if (Array.isArray(value)) {
        return value.map((item) => sanitizeEventValue(item));
    }
    if (value !== null && typeof value === 'object') {
        return Object.fromEntries(Object.entries(value).map(([key, item]) => [
            key,
            sanitizeEventValue(item),
        ]));
    }
    return value;
}
function sanitizeEvent(event) {
    if (event.kind === 'claude') {
        return { ...event, raw: sanitizeEventValue(event.raw) };
    }
    if (event.kind === 'error') {
        return { ...event, message: sanitizeEventText(event.message) };
    }
    if (event.kind === 'stderr') {
        return { ...event, text: sanitizeEventText(event.text) };
    }
    return event;
}
function recordValue(value) {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
        return value;
    }
    return undefined;
}
function nestedRecord(value, key) {
    return recordValue(recordValue(value)?.[key]);
}
function stringValue(value) {
    return typeof value === 'string' ? value : '';
}
function agentRawType(raw) {
    const obj = recordValue(raw);
    const data = nestedRecord(raw, 'data');
    const event = nestedRecord(raw, 'event');
    const dataEvent = recordValue(data?.event);
    const providerData = nestedRecord(raw, 'providerData');
    const message = nestedRecord(raw, 'message');
    return [
        obj?.type,
        data?.type,
        event?.type,
        dataEvent?.type,
        providerData?.type,
        message?.type,
    ]
        .filter((value) => typeof value === 'string' && value.length > 0)
        .join(' ');
}
function agentEventVisibleText(raw) {
    const obj = recordValue(raw);
    if (!obj)
        return '';
    const directText = stringValue(obj.text).trim();
    if (directText)
        return directText;
    const data = nestedRecord(raw, 'data');
    const event = recordValue(data?.event) ?? nestedRecord(raw, 'event') ?? data ?? {};
    const rawType = agentRawType(raw);
    if (/output_text\.delta|text_delta|message_delta|content_block_delta/i.test(rawType)) {
        const eventRecord = recordValue(event);
        const content = Array.isArray(eventRecord?.content) ? eventRecord.content[0] : undefined;
        return stringValue(eventRecord?.delta || eventRecord?.text || recordValue(content)?.text).trim();
    }
    if (/message|assistant/i.test(rawType)) {
        const eventRecord = recordValue(event);
        const message = recordValue(eventRecord?.message) ?? nestedRecord(raw, 'message');
        const content = message?.content ?? obj.content;
        if (Array.isArray(content)) {
            return content
                .map((item) => stringValue(recordValue(item)?.text))
                .filter(Boolean)
                .join('');
        }
    }
    return '';
}
function agentEventHasProviderError(raw) {
    const obj = recordValue(raw);
    const message = nestedRecord(raw, 'message');
    return Boolean(obj?.error ||
        obj?.is_error === true ||
        message?.error ||
        /billing_error|api_error|permission_denied|quota|rate limit/i.test([obj?.error, obj?.result, obj?.terminal_reason].filter(Boolean).join(' ')));
}
function agentEventIsProviderMetadataOnly(raw) {
    const obj = recordValue(raw);
    if (!obj)
        return false;
    return Boolean(obj.provider && obj.model && !agentEventVisibleText(raw));
}
function shouldForwardAgentRunnerEvent(event) {
    if (event.kind !== 'claude')
        return true;
    const rawType = agentRawType(event.raw);
    const visibleText = agentEventVisibleText(event.raw);
    if (agentEventHasProviderError(event.raw))
        return false;
    if (agentEventIsProviderMetadataOnly(event.raw))
        return false;
    if (/raw_model_stream_event|response\.created|response\.in_progress|response_started|response\.completed|system|tool/i.test(rawType) &&
        !visibleText) {
        return false;
    }
    return true;
}
function agentRawProvider(raw) {
    const obj = recordValue(raw);
    const provider = obj?.provider ?? nestedRecord(raw, 'providerData')?.provider;
    return isAgentProviderValue(provider) ? provider : undefined;
}
function agentRawModel(raw) {
    const obj = recordValue(raw);
    const data = nestedRecord(raw, 'data');
    const event = nestedRecord(raw, 'event');
    const dataEvent = recordValue(data?.event);
    const providerData = nestedRecord(raw, 'providerData');
    const message = nestedRecord(raw, 'message');
    const candidates = [
        obj?.model,
        obj?.modelId,
        obj?.model_id,
        data?.model,
        event?.model,
        dataEvent?.model,
        providerData?.model,
        providerData?.modelId,
        message?.model,
    ];
    return candidates.find((value) => typeof value === 'string' && value.trim().length > 0)?.trim();
}
function annotateEventWithAgentMetadata(state, event) {
    const raw = event.kind === 'claude' ? event.raw : undefined;
    const provider = event.provider ?? agentRawProvider(raw) ?? state.activeProvider ?? state.provider;
    const model = event.model ?? agentRawModel(raw) ?? state.activeModel;
    const label = event.modelLabel ?? modelLabel(provider, model) ?? state.activeModelLabel;
    return {
        ...event,
        provider,
        ...(model ? { model } : {}),
        ...(label ? { modelLabel: label } : {}),
    };
}
function emit(state, event) {
    event = annotateEventWithAgentMetadata(state, event);
    event = sanitizeEvent(event);
    // Once a task has emitted `done`, it's terminal — late events from a
    // race (e.g. cancel firing while claude was already exiting cleanly)
    // would corrupt the seq stream and confuse downstream consumers.
    if (state.finished && !['done', 'pr_open'].includes(event.kind)) {
        return;
    }
    if (state.finished && event.kind === 'done') {
        return; // dedupe duplicate done emits (cancel + natural close race)
    }
    const stored = { seq: state.events.length, event };
    state.events.push(stored);
    state.event$.next(stored);
    incCounter('dd_runtime_events_total', {
        service: 'dd-dev-server-api',
        kind: event.kind,
        provider: state.provider,
    });
    void postTaskBreadcrumb(state, 'event', { seq: stored.seq, event });
    if (event.kind === 'done') {
        incCounter('dd_runtime_tasks_total', {
            service: 'dd-dev-server-api',
            provider: state.provider,
            exitReason: event.exitReason,
        });
        state.finished = true;
        state.finishedAt = Date.now();
        state.session.lastActiveAt = Date.now();
        state.event$.complete();
        if (state.userId) {
            releaseUserChannel(state.userId);
        }
    }
    // Push into the RxJS EventBus. All downstream side effects (Vercel
    // ingest with retry, Supabase broadcast with retry, log file sink)
    // are now handled by reactive pipelines in event-bus.ts.
    eventBus.emit({
        taskId: state.taskId,
        threadId: state.threadId,
        userId: state.userId,
        seq: stored.seq,
        event,
    });
    const fanoutPayload = {
        type: 'task-event',
        messageId: randomUUID(),
        taskId: state.taskId,
        threadId: state.threadId,
        userId: state.userId,
        provider: state.provider,
        activeProvider: event.provider ?? state.activeProvider ?? state.provider,
        model: event.model ?? state.activeModel,
        modelLabel: event.modelLabel ?? state.activeModelLabel,
        branch: state.branch,
        seq: stored.seq,
        emittedAt: new Date().toISOString(),
        event,
    };
    natsPublisher.publish(config.natsEventSubject, fanoutPayload);
    workerFanout.publish(fanoutPayload);
}
async function ensureDeployKey() {
    if (!config.ghDeployKey) {
        return;
    }
    await mkdir(dirname(config.ghDeployKeyPath), { recursive: true });
    try {
        await access(config.ghDeployKeyPath);
        return; // already on disk
    }
    catch {
        /* missing — write it */
    }
    await writeFile(config.ghDeployKeyPath, config.ghDeployKey, { mode: 0o600 });
}
function githubCliEnv() {
    if (!config.ghPat) {
        throw new Error('GH_PAT is required for GitHub CLI pull request creation');
    }
    return { GH_TOKEN: config.ghPat };
}
async function configureGitIdentity(cwd) {
    await shCapture('git', ['config', 'user.name', config.prAuthor.name], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture('git', ['config', 'user.email', config.prAuthor.email], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
}
async function waitForBootGitReady() {
    // Older entrypoints exposed a background git-ready PID. Keep the wait
    // path for rolling deploy compatibility; the current entrypoint runs
    // synchronously before Node starts.
    const gitReadyPid = process.env.GIT_READY_PID;
    if (!gitReadyPid) {
        return;
    }
    try {
        // waitpid via polling — Node doesn't expose waitpid() natively.
        // Once the PID is gone from /proc, the background git is done.
        await new Promise((resolve) => {
            const check = () => {
                try {
                    process.kill(Number(gitReadyPid), 0); // signal 0 = existence check
                    setTimeout(check, 500);
                }
                catch {
                    resolve();
                }
            };
            check();
        });
        delete process.env.GIT_READY_PID;
    }
    catch {
        /* if the PID was already gone, that's fine */
    }
}
async function mergeUpstreamForThread(input) {
    const threadId = input.threadId ?? config.threadId ?? undefined;
    const userId = input.userId ?? config.userId ?? undefined;
    if (!threadId) {
        throw new Error('threadId required');
    }
    if (config.threadId && threadId !== config.threadId) {
        throw new Error(`container is pinned to thread ${config.threadId}, got ${threadId}`);
    }
    if (config.userId && userId && userId !== config.userId) {
        throw new Error(`container is pinned to user ${config.userId}, got ${userId}`);
    }
    const session = getOrCreateSession({
        taskId: randomUUID(),
        threadId,
        userId,
        branch: input.branch,
        threadTitle: input.threadTitle,
    });
    await waitForBootGitReady();
    await session.ready;
    const queuedMerge = session.queue
        .catch(() => undefined)
        .then(async () => {
        session.lastActiveAt = Date.now();
        void postSessionBreadcrumb(session, 'merge-upstream-start', {
            baseBranch: config.baseBranch,
        });
        const before = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await assertSessionOnFeatureBranch(session);
        const status = await gitWorkspaceStatus(session.workspacePath);
        if (status.trim()) {
            throw new Error(`workspace has uncommitted changes before merging ${config.baseBranch}: ${status.trim()}`);
        }
        await fetchRemoteBranch(session.workspacePath, config.baseBranch, 1);
        await ensureMergeBaseWithBaseBranch(session);
        try {
            await shCapture('git', ['merge', '--no-edit', `origin/${config.baseBranch}`], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        }
        catch (err) {
            const conflicts = await gitUnmergedFiles(session.workspacePath).catch(() => []);
            await abortMergeIfInProgress(session.workspacePath);
            throw new Error(conflicts.length > 0
                ? `merge-upstream hit conflicts and was aborted: ${conflicts.join(', ')}`
                : err instanceof Error
                    ? err.message
                    : String(err));
        }
        const after = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await pushSessionBranch(session);
        void postSessionBreadcrumb(session, 'merge-upstream-done', {
            baseBranch: config.baseBranch,
            before,
            after,
        });
        return {
            ok: true,
            threadId,
            branch: session.branch,
            baseBranch: config.baseBranch,
            before,
            after,
            fastForward: before !== after,
        };
    });
    session.queue = queuedMerge.then(() => undefined, () => undefined);
    return queuedMerge;
}
function manualCommitMessage(input) {
    const message = (input.reason ?? input.threadTitle ?? '').trim().replace(/\s+/g, ' ');
    if (message) {
        return message.slice(0, 200);
    }
    return `agent(${input.threadId}): manual commit`;
}
async function makeCommitForThread(input) {
    const threadId = input.threadId ?? config.threadId ?? undefined;
    const userId = input.userId ?? config.userId ?? undefined;
    if (!threadId) {
        throw new Error('threadId required');
    }
    if (config.threadId && threadId !== config.threadId) {
        throw new Error(`container is pinned to thread ${config.threadId}, got ${threadId}`);
    }
    if (config.userId && userId && userId !== config.userId) {
        throw new Error(`container is pinned to user ${config.userId}, got ${userId}`);
    }
    const taskId = input.taskId ?? randomUUID();
    const session = getOrCreateSession({
        taskId,
        threadId,
        userId,
        branch: input.branch,
        threadTitle: input.threadTitle ?? input.reason,
    });
    const taskState = input.taskId ? tasks.get(input.taskId) : undefined;
    if (taskState) {
        emit(taskState, {
            kind: 'status',
            status: `manual commit: preparing ${gitBranchTarget(session.branch)}`,
            message: `Base branch: ${config.baseBranch}`,
        });
    }
    await waitForBootGitReady();
    await session.ready;
    const queuedCommit = session.queue
        .catch(() => undefined)
        .then(async () => {
        session.lastActiveAt = Date.now();
        const before = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        void postSessionBreadcrumb(session, 'make-commit-start', { taskId });
        const status = await gitWorkspaceStatus(session.workspacePath);
        const hasChanges = status.trim().length > 0;
        if (hasChanges) {
            await gitAddWorkspaceChanges(session.workspacePath);
            await shCapture('git', ['commit', '--no-verify', '-m', manualCommitMessage({ ...input, threadId })], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        }
        const after = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await assertSessionOnFeatureBranch(session);
        await pushSessionBranch(session);
        void postSessionBreadcrumb(session, 'make-commit-done', {
            taskId,
            before,
            after,
            committed: hasChanges,
        });
        if (taskState) {
            emit(taskState, {
                kind: 'status',
                status: hasChanges
                    ? `manual commit pushed to ${gitBranchTarget(session.branch)}`
                    : `pushed ${gitBranchTarget(session.branch)} without new commit`,
                message: `Base branch: ${config.baseBranch}`,
            });
        }
        return {
            ok: true,
            threadId,
            branch: session.branch,
            before,
            after,
            committed: hasChanges,
            pushed: true,
            status: hasChanges ? 'committed-and-pushed' : 'pushed-without-new-commit',
        };
    });
    session.queue = queuedCommit.then(() => undefined, () => undefined);
    return queuedCommit;
}
async function openPullRequestForThread(input) {
    const threadId = input.threadId ?? config.threadId ?? undefined;
    const userId = input.userId ?? config.userId ?? undefined;
    if (!threadId) {
        throw new Error('threadId required');
    }
    if (config.threadId && threadId !== config.threadId) {
        throw new Error(`container is pinned to thread ${config.threadId}, got ${threadId}`);
    }
    if (config.userId && userId && userId !== config.userId) {
        throw new Error(`container is pinned to user ${config.userId}, got ${userId}`);
    }
    const taskId = input.taskId ?? randomUUID();
    const session = getOrCreateSession({
        taskId,
        threadId,
        userId,
        branch: input.branch,
        threadTitle: input.threadTitle ?? input.reason,
    });
    const taskState = input.taskId ? tasks.get(input.taskId) : undefined;
    if (taskState) {
        emit(taskState, {
            kind: 'status',
            status: `opening draft PR against ${config.baseBranch} from ${gitBranchTarget(session.branch)}`,
            message: `Repo: ${repoDisplayName()}`,
        });
    }
    await waitForBootGitReady();
    await session.ready;
    const queuedOpen = session.queue
        .catch(() => undefined)
        .then(async () => {
        session.lastActiveAt = Date.now();
        void postSessionBreadcrumb(session, 'open-pr-start', {
            baseBranch: config.baseBranch,
            taskId,
        });
        const result = await ensurePullRequestForSession({
            session,
            taskId,
            threadTitle: input.threadTitle ?? input.reason,
        });
        void postSessionBreadcrumb(session, 'open-pr-done', {
            baseBranch: config.baseBranch,
            taskId,
            prUrl: result.prUrl,
            draft: result.draft,
            reused: result.reused,
        });
        if (taskState) {
            emit(taskState, {
                kind: 'pr_open',
                branch: result.branch,
                prUrl: result.prUrl,
                draft: result.draft,
            });
            emit(taskState, {
                kind: 'status',
                status: `completed PR request: ${result.reused ? 'reused' : 'created'} draft PR against ${result.baseBranch}`,
                message: `${result.prUrl}\nHead: ${repoDisplayName()} branch ${result.branch}`,
            });
        }
        return result;
    });
    session.queue = queuedOpen.then(() => undefined, () => undefined);
    return queuedOpen;
}
function truncateContext(value, maxChars) {
    if (value.length <= maxChars) {
        return value;
    }
    return value.slice(value.length - maxChars);
}
function truncateContextBlob(value, maxChars) {
    if (value.length <= maxChars) {
        return value;
    }
    return `${value.slice(0, maxChars)}\n[context blob truncated]`;
}
function truncateRepoContextFile(value, maxChars) {
    if (value.length <= maxChars) {
        return value;
    }
    return `${value.slice(0, maxChars)}\n[repo context file truncated]`;
}
async function listMarkdownChildren(workspacePath, relativeDir) {
    try {
        const dirents = await readdir(resolve(workspacePath, relativeDir), { withFileTypes: true });
        return dirents
            .filter((dirent) => dirent.isFile() && /\.md$/i.test(dirent.name))
            .map((dirent) => `${relativeDir}/${dirent.name}`)
            .sort((left, right) => left.localeCompare(right));
    }
    catch {
        return [];
    }
}
async function existingContextFiles(workspacePath, relativePaths) {
    const out = [];
    for (const relativePath of relativePaths) {
        const absolutePath = resolve(workspacePath, relativePath);
        const repoRelative = relative(workspacePath, absolutePath);
        if (repoRelative.startsWith(`..${sep}`) || isAbsolute(repoRelative)) {
            continue;
        }
        try {
            const fileStat = await stat(absolutePath);
            if (fileStat.isFile()) {
                out.push(relativePath);
            }
        }
        catch {
            /* optional context file */
        }
    }
    return out;
}
async function readRepoContextEntrypoint(workspacePath) {
    const rootAgents = await existingContextFiles(workspacePath, ['AGENTS.md']);
    const agentDocs = await listMarkdownChildren(workspacePath, 'agents');
    const docs = await listMarkdownChildren(workspacePath, 'docs');
    const allContextFiles = [...rootAgents, ...agentDocs, ...docs];
    const inlineFiles = [...rootAgents, ...agentDocs, ...docs.slice(0, 12)];
    const sections = [];
    let usedChars = 0;
    for (const relativePath of inlineFiles) {
        const absolutePath = resolve(workspacePath, relativePath);
        const repoRelative = relative(workspacePath, absolutePath);
        if (repoRelative.startsWith(`..${sep}`) || isAbsolute(repoRelative)) {
            continue;
        }
        let text = '';
        try {
            const fileStat = await stat(absolutePath);
            if (!fileStat.isFile()) {
                continue;
            }
            text = await readFile(absolutePath, 'utf8');
        }
        catch {
            continue;
        }
        const remaining = config.repoContextMaxChars - usedChars;
        if (remaining <= 0) {
            break;
        }
        const capped = truncateRepoContextFile(text.trim(), Math.min(remaining, 8_000));
        const section = [`File: ${relativePath}`, '', capped].join('\n');
        sections.push(section);
        usedChars += section.length;
    }
    if (allContextFiles.length === 0 && sections.length === 0) {
        return '';
    }
    const catalog = allContextFiles.length
        ? `Available repo context files:\n${allContextFiles.map((file) => `- ${file}`).join('\n')}`
        : 'No AGENTS.md, agents/*.md, or docs/*.md files were found in this checkout.';
    return [catalog, ...sections].join('\n\n---\n\n');
}
function formatThreadContextTasks(tasksFromContext, currentTaskId) {
    const tasksForPrompt = tasksFromContext.filter((task) => task.id !== currentTaskId);
    if (tasksForPrompt.length === 0) {
        return '';
    }
    return tasksForPrompt
        .map((task, index) => {
        const parts = [
            `Task ${index + 1}: ${task.id ?? 'unknown'}`,
            `status: ${task.status ?? 'unknown'}`,
        ];
        if (task.branch) {
            parts.push(`branch: ${task.branch}`);
        }
        if (task.exitReason) {
            parts.push(`exit: ${task.exitReason}`);
        }
        if (task.errorMessage) {
            parts.push(`error: ${task.errorMessage}`);
        }
        const prompt = task.prompt ? `prompt: ${task.prompt}` : '';
        const latest = task.latestPayload ? `latest: ${task.latestPayload}` : '';
        return [parts.join(', '), prompt, latest].filter(Boolean).join('\n');
    })
        .join('\n\n');
}
function isBreadcrumbContextItem(item) {
    return item.kind === 'breadcrumb';
}
function isThreadTaskContextItem(item) {
    return item.kind === 'thread-task';
}
function formatSelectedContextBlobs(state) {
    if (state.contextMode === 'none' || !state.contextBlobs?.length) {
        return '';
    }
    const maxTotalChars = Math.min(config.threadContextMaxChars, 40_000);
    const maxBlobChars = 6_000;
    let usedChars = 0;
    const sections = [];
    let index = 0;
    for (const item of state.contextBlobs) {
        if (isBreadcrumbContextItem(item) || isThreadTaskContextItem(item))
            continue;
        index += 1;
        const title = item.contextTitle.trim() || item.contextId;
        const blob = truncateContextBlob(item.contextBlob.trim(), maxBlobChars);
        const section = [
            `Context ${index}: ${title}`,
            `contextId: ${item.contextId}`,
            item.projectId ? `projectId: ${item.projectId}` : '',
            item.matchSource ? `matchSource: ${item.matchSource}` : '',
            Number.isFinite(item.score) ? `score: ${item.score}` : '',
            '',
            blob,
        ]
            .filter(Boolean)
            .join('\n');
        if (usedChars + section.length > maxTotalChars) {
            break;
        }
        sections.push(section);
        usedChars += section.length;
    }
    return sections.join('\n\n---\n\n');
}
/**
 * The picker can hand us breadcrumb rows alongside long-lived context blobs by
 * sending `kind: 'breadcrumb'` with a JSON-serialized AgentBreadcrumbRow in
 * `contextBlob`. We render those into <thread_breadcrumb_tail> rather than
 * <selected_context_blobs> so prompt structure matches operator intent.
 */
function formatSelectedBreadcrumbs(state) {
    if (state.contextMode === 'none' || !state.contextBlobs?.length) {
        return '';
    }
    const lines = [];
    for (const item of state.contextBlobs) {
        if (!isBreadcrumbContextItem(item))
            continue;
        let payload = item.contextBlob;
        try {
            payload = JSON.parse(item.contextBlob);
        }
        catch {
            // fall back to raw blob string
        }
        lines.push(JSON.stringify(payload));
    }
    if (lines.length === 0)
        return '';
    return truncateContext(lines.join('\n'), Math.min(config.threadContextMaxChars, 24_000));
}
function formatSelectedThreadTasks(state) {
    if (state.contextMode === 'none' || !state.contextBlobs?.length) {
        return '';
    }
    const sections = state.contextBlobs
        .filter(isThreadTaskContextItem)
        .map((item) => item.contextBlob.trim())
        .filter(Boolean);
    if (sections.length === 0)
        return '';
    return truncateContext(sections.join('\n\n---\n\n'), config.threadContextMaxChars);
}
async function fetchSelectedContextBlobs(state) {
    if (state.contextBlobs?.length)
        return state.contextBlobs;
    if (state.contextMode !== 'selected' || !state.contextIds?.length || !state.threadId)
        return [];
    const base = config.threadContextBaseUrl?.replace(/\/+$/, '');
    if (!base)
        return [];
    try {
        const response = await contextFetch(`${base}/api/agents/threads/${encodeURIComponent(state.threadId)}/context-candidates`, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({
                prompt: state.prompt,
                repo: config.repoUrl,
                baseBranch: config.baseBranch,
                contextIds: state.contextIds,
                limit: Math.max(1, Math.min(50, state.contextIds.length)),
            }),
            signal: AbortSignal.timeout(10_000),
        });
        if (!response.ok)
            return [];
        const body = (await response.json());
        return body.candidates ?? [];
    }
    catch (err) {
        emit(state, {
            kind: 'stderr',
            text: `selected context lookup failed: ${err instanceof Error ? err.message : String(err)}`,
        });
        return [];
    }
}
// Breadcrumb prompt context is now driven by explicit picker selection. Workers
// no longer fetch the full thread tail unconditionally; instead, the dispatch
// payload carries a `contextBlobs` array containing the breadcrumb rows the
// operator kept checked in the /agents/threads picker. Unchecked rows simply
// never reach the worker, which is how "remove from context" is implemented.
async function buildPromptWithThreadContext(state) {
    const base = config.threadContextBaseUrl?.replace(/\/+$/, '');
    let restContextText = '';
    let restContextSource = 'none';
    const hasSelectedThreadTasks = Boolean(state.contextBlobs?.some(isThreadTaskContextItem));
    const automaticThreadContext = (!state.contextMode || state.contextMode === 'auto') && !hasSelectedThreadTasks;
    if (state.threadId && base && automaticThreadContext) {
        try {
            const response = await contextFetch(`${base}/api/agents/threads/${encodeURIComponent(state.threadId)}/context?limit=${config.threadContextLimit}`, { signal: AbortSignal.timeout(10_000) });
            if (response.ok) {
                const body = (await response.json());
                restContextText = formatThreadContextTasks(body.tasks ?? [], state.taskId);
                restContextSource = body.source ?? 'rest-api';
            }
        }
        catch (err) {
            emit(state, {
                kind: 'stderr',
                text: `thread context lookup failed: ${err instanceof Error ? err.message : String(err)}`,
            });
        }
    }
    const repoContext = await readRepoContextEntrypoint(state.worktreePath);
    const fetchedContextBlobs = await fetchSelectedContextBlobs(state);
    if (fetchedContextBlobs.length && !state.contextBlobs?.length) {
        state.contextBlobs = fetchedContextBlobs;
    }
    const selectedContext = formatSelectedContextBlobs(state);
    const selectedThreadTasks = formatSelectedThreadTasks(state);
    const selectedBreadcrumbs = formatSelectedBreadcrumbs(state);
    const runtimeContext = clusterMcpPromptSection(config.agentMcpUrl);
    const promptSections = [
        state.threadId ? `You are continuing remote development thread ${state.threadId}.` : 'You are starting a remote development task.',
        `Current task UUID: ${state.taskId}.`,
        'Use the supplied context when it is relevant, but let the current user prompt decide the work.',
        'Do not repeat completed work unless the current user prompt asks you to.',
    ];
    if (config.agentOptimisticMode) {
        promptSections.push('', '<agent_operating_mode>', [
            'Optimistic/autonomous mode is enabled for this fire-and-forget remote agent run.',
            'Do not stop to ask the human user a question before acting.',
            'Make the safest reasonable assumption, keep the change scoped, and continue.',
            'If a decision is genuinely blocked, record the question and your assumption in the final summary so the user can answer in a later task prompt.',
            'Prefer concrete repo inspection, tests, and small reversible changes over waiting for clarification.',
        ].join('\n'), '</agent_operating_mode>');
    }
    if (repoContext) {
        emit(state, {
            kind: 'status',
            status: 'thread-context:repo-files',
            message: 'AGENTS.md/docs/agents repo context injected into the task prompt.',
        });
        promptSections.push('', '<repo_context_files>', repoContext, '</repo_context_files>');
    }
    if (selectedContext) {
        const blobCount = state.contextBlobs?.filter((item) => !isBreadcrumbContextItem(item) && !isThreadTaskContextItem(item)).length ?? 0;
        emit(state, {
            kind: 'status',
            status: 'thread-context:selected-blobs',
            message: `${blobCount} selected context blob(s) injected into the task prompt.`,
        });
        promptSections.push('', '<selected_context_blobs>', selectedContext, '</selected_context_blobs>');
    }
    if (selectedThreadTasks) {
        const taskCount = state.contextBlobs?.filter(isThreadTaskContextItem).length ?? 0;
        emit(state, {
            kind: 'status',
            status: 'thread-context:selected-tasks',
            message: `${taskCount} selected previous task context item(s) injected into the task prompt.`,
        });
        promptSections.push('', '<previous_thread_context>', selectedThreadTasks, '</previous_thread_context>');
    }
    if (restContextText) {
        const cappedContext = truncateContext(restContextText, config.threadContextMaxChars);
        emit(state, { kind: 'status', status: `thread-context:${restContextSource}` });
        promptSections.push('', '<previous_thread_context>', cappedContext, '</previous_thread_context>');
    }
    if (selectedBreadcrumbs) {
        const breadcrumbCount = state.contextBlobs?.filter(isBreadcrumbContextItem).length ?? 0;
        emit(state, {
            kind: 'status',
            status: 'thread-context:selected-breadcrumbs',
            message: `${breadcrumbCount} selected breadcrumb row(s) injected into the task prompt.`,
        });
        promptSections.push('', '<thread_breadcrumb_tail>', selectedBreadcrumbs, '</thread_breadcrumb_tail>');
    }
    if (runtimeContext) {
        emit(state, {
            kind: 'status',
            status: 'thread-context:cluster-mcp',
            message: 'Cluster MCP runtime context injected into the task prompt.',
        });
        promptSections.push('', '<runtime_context>', runtimeContext, '</runtime_context>');
    }
    promptSections.push('', '<current_user_prompt>', state.prompt, '</current_user_prompt>');
    return promptSections.join('\n');
}
async function runInternalWorkspaceAgent(input) {
    const providerOrder = [input.state.provider, ...config.agentProviderRotation].filter((provider, index, values) => values.indexOf(provider) === index);
    const attemptGroups = [];
    for (const provider of providerOrder) {
        if (!providerCanEditWorkspace(provider)) {
            emit(input.state, {
                kind: 'status',
                status: `agent-skip:${provider}`,
                message: `${provider} cannot edit files for ${input.purpose} in ${repoDisplayName()}`,
            });
            continue;
        }
        if (input.requireShellAccess && !providerCanUseShell(provider)) {
            emit(input.state, {
                kind: 'status',
                status: `agent-skip:${provider}`,
                message: `${provider} cannot run shell/git commands for ${input.purpose} in ${repoDisplayName()}`,
            });
            continue;
        }
        const candidates = buildAgentEnvCandidates(provider);
        if (candidates.length === 0) {
            emit(input.state, {
                kind: 'status',
                status: `agent-skip:${provider}`,
                message: `No configured API keys for ${provider}`,
            });
            continue;
        }
        attemptGroups.push({ provider, candidates });
    }
    if (attemptGroups.length === 0) {
        throw new Error(`no configured workspace-capable agent API keys for ${input.purpose} in ${repoDisplayName()}`);
    }
    let lastErr = null;
    for (const [groupIndex, group] of attemptGroups.entries()) {
        if (input.state.cancelled || input.state.abortController.signal.aborted) {
            throw lastErr ?? new Error(`${input.purpose} cancelled`);
        }
        if (groupIndex > 0) {
            emit(input.state, {
                kind: 'status',
                status: `agent-fallback:${group.provider}`,
                message: `Switching to ${group.provider} for ${input.purpose}`,
            });
        }
        input.state.activeProvider = group.provider;
        input.state.activeModel = modelForAgentEnv(group.provider, group.candidates[0]?.env ?? {});
        input.state.activeModelLabel = modelLabel(group.provider, input.state.activeModel);
        emit(input.state, {
            kind: 'status',
            status: `agent-running:${group.provider}`,
            message: `${input.purpose}\n` +
                `Workspace: ${gitBranchTarget(input.state.branch)}\n` +
                `Base branch: ${config.baseBranch}\n` +
                `Credentials: ${group.candidates.length} configured key(s)`,
        });
        const failures = new Map();
        let attempted = 0;
        for (const attempt of group.candidates) {
            attempted += 1;
            try {
                input.state.activeProvider = attempt.provider;
                input.state.activeModel = modelForAgentEnv(attempt.provider, attempt.env);
                input.state.activeModelLabel = modelLabel(attempt.provider, input.state.activeModel);
                const statusBeforeAttempt = input.requireWorkspaceChange
                    ? await gitWorkspaceStatus(input.state.worktreePath)
                    : '';
                const runner = getRunner(attempt.provider);
                await runner.run({
                    prompt: input.prompt,
                    cwd: input.state.worktreePath,
                    env: attempt.env,
                    signal: input.state.abortController.signal,
                    timeoutMs: config.agentRunTimeoutMs,
                    emit: (ev) => {
                        if (shouldForwardAgentRunnerEvent(ev)) {
                            emit(input.state, ev);
                        }
                    },
                    setChild: (child) => {
                        input.state.child = child;
                    },
                });
                if (input.requireWorkspaceChange) {
                    const statusAfterAttempt = await gitWorkspaceStatus(input.state.worktreePath);
                    if (!statusAfterAttempt.trim() || statusAfterAttempt.trim() === statusBeforeAttempt.trim()) {
                        throw new Error(`${attempt.provider} completed without workspace changes for ${input.purpose}`);
                    }
                }
                return;
            }
            catch (err) {
                lastErr = err;
                const reason = compactAgentErrorMessage(attempt.provider, err);
                failures.set(reason, (failures.get(reason) ?? 0) + 1);
            }
        }
        if (attempted > 0) {
            emit(input.state, {
                kind: 'error',
                message: formatAgentFailureSummary(group.provider, failures, attempted, group.candidates.length),
            });
        }
    }
    throw lastErr ?? new Error(`${input.purpose} failed`);
}
async function resolveMergeConflictsSemantically(state, conflictedFiles) {
    const conflictPrompt = [
        `Resolve the current Git merge conflicts in ${repoDisplayName()} before task ${state.taskId} starts.`,
        '',
        `The worker already ran: git merge --no-edit origin/${config.baseBranch}`,
        `Feature branch: ${state.branch}`,
        `Base branch: ${config.baseBranch}`,
        '',
        'Resolve only the merge conflicts. Preserve the intended changes from both the base branch and the feature branch, remove all conflict markers, and do not implement the user task yet.',
        'Leave the repository ready for the server to stage and commit the merge resolution.',
        '',
        'Conflicted files:',
        ...conflictedFiles.map((file) => `- ${file}`),
    ].join('\n');
    await runInternalWorkspaceAgent({
        state,
        prompt: conflictPrompt,
        purpose: 'merge-conflict-resolution',
    });
    await gitAddWorkspaceChanges(state.worktreePath);
    const remainingConflicts = await gitUnmergedFiles(state.worktreePath);
    if (remainingConflicts.length > 0) {
        throw new Error(`merge conflicts remain after agent resolution: ${remainingConflicts.join(', ')}`);
    }
    await shCapture('git', ['diff', '--cached', '--check'], state.worktreePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture('git', ['commit', '--no-verify', '--no-edit'], state.worktreePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
}
async function mergeBaseBranchBeforeTask(state) {
    const session = state.session;
    emit(state, {
        kind: 'status',
        status: 'verifying-thread-feature-branch',
        message: `Expected feature branch: ${session.branch}\nParent branch: ${config.baseBranch}`,
    });
    await assertSessionOnFeatureBranch(session);
    const statusBeforeMerge = await gitWorkspaceStatus(session.workspacePath);
    if (statusBeforeMerge.trim()) {
        throw new Error(`workspace has uncommitted changes before starting task ${state.taskId}; commit or resolve them before queue execution continues`);
    }
    void postTaskBreadcrumb(state, 'merge-base-before-task-start', {
        baseBranch: config.baseBranch,
    });
    emit(state, {
        kind: 'status',
        status: `fetching ${config.baseBranch} before task`,
        message: 'Using a depth-1 fetch first; the worker deepens only if Git needs more history to merge.',
    });
    await fetchRemoteBranch(session.workspacePath, config.baseBranch, 1);
    await ensureMergeBaseWithBaseBranch(session);
    const before = await currentGitCommit(session.workspacePath);
    try {
        await shCapture('git', ['merge', '--no-edit', `origin/${config.baseBranch}`], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
    }
    catch (err) {
        const conflicts = await gitUnmergedFiles(session.workspacePath).catch(() => []);
        if (conflicts.length === 0) {
            throw err;
        }
        emit(state, {
            kind: 'status',
            status: 'merge-conflict:resolving-before-task',
            message: `Resolving ${conflicts.length} conflicted file(s) before starting the user task.`,
        });
        void postTaskBreadcrumb(state, 'merge-base-conflicts-before-task', {
            baseBranch: config.baseBranch,
            conflicts,
        });
        try {
            await resolveMergeConflictsSemantically(state, conflicts);
        }
        catch (resolveErr) {
            await abortMergeIfInProgress(session.workspacePath);
            throw new Error(`failed to resolve upstream merge conflicts before task ${state.taskId}: ${resolveErr instanceof Error ? resolveErr.message : String(resolveErr)}`);
        }
    }
    await assertSessionOnFeatureBranch(session);
    const after = await currentGitCommit(session.workspacePath);
    const statusAfterMerge = await gitWorkspaceStatus(session.workspacePath);
    if (statusAfterMerge.trim()) {
        throw new Error(`upstream merge preflight left uncommitted changes before task ${state.taskId}: ${statusAfterMerge.trim()}`);
    }
    if (before !== after) {
        await pushSessionBranch(session);
    }
    void postTaskBreadcrumb(state, 'merge-base-before-task-done', {
        baseBranch: config.baseBranch,
        before,
        after,
        pushed: before !== after,
    });
    emit(state, {
        kind: 'status',
        status: before === after ? 'base-branch-already-merged' : 'base-branch-merged-before-task',
        message: `Feature branch ${session.branch} is ready for task ${state.taskId}.`,
    });
}
// ---------- Per-task workflow ----------
async function runTask(state) {
    return withSpan('remote-dev.run-task', {
        'dd.remote.task_id': state.taskId,
        'dd.remote.thread_id': state.threadId,
        'dd.remote.provider': state.provider,
        'dd.remote.branch': state.branch,
    }, async (span) => {
        if (state.userId) {
            acquireUserChannel(state.userId);
        }
        if (state.finished || state.cancelled) {
            if (!state.finished) {
                emit(state, {
                    kind: 'done',
                    branch: state.branch,
                    exitReason: 'cancelled',
                });
            }
            return;
        }
        emit(state, { kind: 'status', status: 'waiting-for-workspace' });
        await waitForBootGitReady();
        emit(state, { kind: 'status', status: 'syncing-thread-workspace' });
        await state.session.ready;
        state.session.lastActiveAt = Date.now();
        await mergeBaseBranchBeforeTask(state);
        // Per-task outputs dir — the agent writes publishable files here.
        // After claude exits we scan it and upload each file via the storage
        // adapter, emitting an `artifact` event per file.
        const taskOutputsDir = join(config.outputsDir, state.taskId);
        await mkdir(taskOutputsDir, { recursive: true });
        void postTaskBreadcrumb(state, 'prompt', {
            prompt: state.prompt,
            workspacePath: state.worktreePath,
            contextMode: state.contextMode,
            contextIds: state.contextIds,
            contextTitles: state.contextBlobs?.map((item) => ({
                contextId: item.contextId,
                title: item.contextTitle,
                matchSource: item.matchSource,
                score: item.score,
            })),
        });
        if (state.containerPool) {
            emit(state, {
                kind: 'status',
                status: `container-pool-dispatch:${state.containerPool.pool}`,
            });
            const response = await dispatchContainerPool(config.containerPool, state.containerPool.pool, {
                ...state.containerPool.request,
                requestId: state.containerPool.request.requestId ?? state.taskId,
                poolSlug: state.containerPool.request.poolSlug ?? state.containerPool.pool,
            });
            void postTaskBreadcrumb(state, 'container-pool-result', {
                pool: state.containerPool.pool,
                response,
            });
            emit(state, {
                kind: 'container_pool',
                pool: state.containerPool.pool,
                response,
            });
            emit(state, {
                kind: 'done',
                branch: state.branch,
                exitReason: response.ok ? 'completed' : 'failed',
            });
            return;
        }
        const requiresWorkspaceChange = promptLikelyRequiresWorkspaceChange(state.prompt);
        const requiresWorkspaceAccess = promptLikelyRequiresWorkspaceAccess(state.prompt);
        const requiresShellAccess = promptLikelyRequiresShellAccess(state.prompt);
        const requestsPullRequest = promptRequestsPullRequest(state.prompt);
        const pullRequestOnly = requestsPullRequest && !requiresWorkspaceChange && !requiresWorkspaceAccess;
        // Strict env allowlist owned by the runner module. Inheriting the full
        // process.env into the agent process would leak our GitHub deploy key,
        // Supabase service role key, ingest secret, etc. via any `env` or
        // `printenv` tool call. The runner adds only the API key its model
        // needs.
        const prompt = await buildPromptWithThreadContext(state);
        let lastErr = null;
        let completedAgentRun = false;
        const deterministicEdit = requiresWorkspaceChange ? await applyDeterministicWorkspaceEdit(state) : null;
        if (deterministicEdit) {
            completedAgentRun = true;
        }
        else if (pullRequestOnly) {
            completedAgentRun = true;
            emit(state, {
                kind: 'status',
                status: 'deterministic-pr-only',
                message: 'Prompt only requested a PR, so the worker will open/reuse a draft PR without spending model credentials.',
            });
        }
        else {
            const providerOrder = [state.provider, ...config.agentProviderRotation].filter((provider, index, values) => values.indexOf(provider) === index);
            const attemptGroups = [];
            for (const provider of providerOrder) {
                if (requiresWorkspaceChange && !providerCanEditWorkspace(provider)) {
                    emit(state, {
                        kind: 'status',
                        status: `agent-skip:${provider}`,
                        message: `${provider} is model-only and cannot edit files in ${repoDisplayName()}`,
                    });
                    continue;
                }
                if (requiresWorkspaceAccess && !providerCanAccessWorkspace(provider)) {
                    emit(state, {
                        kind: 'status',
                        status: `agent-skip:${provider}`,
                        message: `${provider} is model-only and cannot inspect the workspace for ${repoDisplayName()}`,
                    });
                    continue;
                }
                if (requiresShellAccess && !providerCanUseShell(provider)) {
                    emit(state, {
                        kind: 'status',
                        status: `agent-skip:${provider}`,
                        message: `${provider} cannot run shell/git commands required for this task in ${repoDisplayName()}`,
                    });
                    continue;
                }
                const candidates = buildAgentEnvCandidates(provider);
                if (candidates.length === 0) {
                    emit(state, {
                        kind: 'status',
                        status: `agent-skip:${provider}`,
                        message: `No configured API keys for ${provider}`,
                    });
                    continue;
                }
                attemptGroups.push({ provider, candidates });
            }
            if (attemptGroups.length === 0) {
                if (requiresShellAccess) {
                    throw new Error(`no configured shell-capable agent API keys for ${repoDisplayName()}; set OPENAI_API_KEYS_JSON or ANTHROPIC_API_KEYS_JSON`);
                }
                throw new Error(`no configured agent API keys for ${repoDisplayName()}; set OPENAI_API_KEYS_JSON, ANTHROPIC_API_KEYS_JSON, OPENCODE_API_KEYS_JSON, DEEPSEEK_API_KEYS_JSON, XAI_API_KEYS_JSON, or GEMINI_API_KEYS_JSON`);
            }
            const runAgentAttempt = async (attempt) => {
                state.activeProvider = attempt.provider;
                state.activeModel = modelForAgentEnv(attempt.provider, attempt.env);
                state.activeModelLabel = modelLabel(attempt.provider, state.activeModel);
                const runner = getRunner(attempt.provider);
                await runner.run({
                    prompt,
                    cwd: state.worktreePath,
                    env: attempt.env,
                    signal: state.abortController.signal,
                    timeoutMs: config.agentRunTimeoutMs,
                    emit: (ev) => {
                        if (shouldForwardAgentRunnerEvent(ev)) {
                            emit(state, ev);
                        }
                    },
                    setChild: (child) => {
                        state.child = child;
                    },
                });
            };
            for (const [groupIndex, group] of attemptGroups.entries()) {
                if (state.cancelled || state.abortController.signal.aborted) {
                    throw lastErr ?? new Error('agent run cancelled');
                }
                if (groupIndex > 0) {
                    emit(state, {
                        kind: 'status',
                        status: `agent-fallback:${group.provider}`,
                        message: `Switching to ${group.provider} after the previous provider failed`,
                    });
                }
                state.activeProvider = group.provider;
                state.activeModel = modelForAgentEnv(group.provider, group.candidates[0]?.env ?? {});
                state.activeModelLabel = modelLabel(group.provider, state.activeModel);
                emit(state, {
                    kind: 'status',
                    status: `agent-running:${group.provider}`,
                    message: `Workspace: ${gitBranchTarget(state.branch)}\n` +
                        `Base branch: ${config.baseBranch}\n` +
                        `Credentials: ${group.candidates.length} configured key(s)`,
                });
                const failures = new Map();
                let attempted = 0;
                for (const attempt of group.candidates) {
                    if (state.cancelled || state.abortController.signal.aborted) {
                        throw lastErr ?? new Error('agent run cancelled');
                    }
                    attempted += 1;
                    try {
                        const statusBeforeAttempt = requiresWorkspaceChange
                            ? await gitWorkspaceStatus(state.worktreePath)
                            : '';
                        await runAgentAttempt(attempt);
                        if (requiresWorkspaceChange) {
                            const statusAfterAttempt = await gitWorkspaceStatus(state.worktreePath);
                            if (!statusAfterAttempt.trim() || statusAfterAttempt.trim() === statusBeforeAttempt.trim()) {
                                throw new Error(`${attempt.provider} completed without workspace changes for a repo-edit prompt in ${repoDisplayName()}`);
                            }
                        }
                        completedAgentRun = true;
                        lastErr = null;
                        break;
                    }
                    catch (err) {
                        lastErr = err;
                        const reason = compactAgentErrorMessage(attempt.provider, err);
                        failures.set(reason, (failures.get(reason) ?? 0) + 1);
                    }
                }
                if (completedAgentRun) {
                    break;
                }
                if (attempted > 0) {
                    emit(state, {
                        kind: 'error',
                        message: formatAgentFailureSummary(group.provider, failures, attempted, group.candidates.length),
                    });
                }
            }
        }
        if (!completedAgentRun) {
            throw new Error('all configured agent providers failed; see provider summaries above');
        }
        if (state.cancelled || state.abortController.signal.aborted) {
            emit(state, {
                kind: 'done',
                branch: state.branch,
                exitReason: 'cancelled',
            });
            return;
        }
        // Stage + commit anything the agent left uncommitted, then push.
        const status = await gitWorkspaceStatus(state.worktreePath);
        if (!status.trim() && requiresWorkspaceChange) {
            throw new Error(`agent completed without workspace changes for a repo-edit prompt in ${repoDisplayName()}`);
        }
        emit(state, {
            kind: 'status',
            status: `pushing to ${gitBranchTarget(state.branch)}`,
            message: `Base branch: ${config.baseBranch}`,
        });
        if (status.trim()) {
            await gitAddWorkspaceChanges(state.worktreePath);
            await shCapture('git', ['commit', '--no-verify', '-m', `agent(${state.session.sessionId}): ${state.taskId}`], state.worktreePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        }
        await assertSessionOnFeatureBranch(state.session);
        await pushSessionBranch(state.session);
        emit(state, {
            kind: 'status',
            status: `pushed to ${gitBranchTarget(state.branch)}`,
            message: status.trim()
                ? `Committed ${status.trim().split('\n').length} changed path(s).`
                : 'No workspace changes were committed; branch push verified.',
        });
        let completionMessage = 'No PR was opened automatically; use Open draft PR to create one against the base branch.';
        if (requestsPullRequest) {
            emit(state, {
                kind: 'status',
                status: `opening draft PR against ${config.baseBranch} from ${gitBranchTarget(state.branch)}`,
                message: `Prompt requested a PR.\nRepo: ${repoDisplayName()}`,
            });
            const pr = await ensurePullRequestForSession({
                session: state.session,
                taskId: state.taskId,
                prompt: state.prompt,
            });
            emit(state, {
                kind: 'pr_open',
                branch: pr.branch,
                prUrl: pr.prUrl,
                draft: pr.draft,
            });
            emit(state, {
                kind: 'status',
                status: `completed PR request: ${pr.reused ? 'reused' : 'created'} draft PR against ${pr.baseBranch}`,
                message: `${pr.prUrl}\nHead: ${repoDisplayName()} branch ${pr.branch}`,
            });
            completionMessage = `Draft PR ${pr.reused ? 'reused' : 'created'}: ${pr.prUrl}`;
        }
        // Publish any files the agent dropped in the per-task outputs dir.
        // Failures uploading individual files are surfaced as `error` events
        // but do not fail the whole task.
        await publishOutputs(state, taskOutputsDir);
        emit(state, {
            kind: 'status',
            status: `completed task on ${gitBranchTarget(state.branch)}`,
            message: completionMessage,
        });
        emit(state, {
            kind: 'done',
            branch: state.branch,
            exitReason: 'completed',
        });
    });
}
async function ensurePullRequestForSession(input) {
    const ghEnv = githubCliEnv();
    try {
        const existing = await shCapture('gh', [
            'pr',
            'view',
            input.session.branch,
            '--json',
            'url,isDraft,title',
            '--jq',
            '[.url, (.isDraft | tostring), .title] | @tsv',
        ], input.session.workspacePath, { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv });
        const [url, isDraft, title] = existing.trim().split('\t');
        if (url) {
            if (isDraft !== 'true') {
                await shCapture('gh', ['pr', 'ready', input.session.branch, '--undo'], input.session.workspacePath, {
                    timeoutMs: TIMEOUT_GH_PR,
                    extraEnv: ghEnv,
                });
            }
            return {
                ok: true,
                threadId: input.session.sessionId,
                branch: input.session.branch,
                baseBranch: config.baseBranch,
                prUrl: url,
                title: title || `WIP - ${input.threadTitle || input.session.sessionId}`,
                draft: true,
                reused: true,
            };
        }
    }
    catch {
        /* no existing PR */
    }
    await assertSessionOnFeatureBranch(input.session);
    await fetchRemoteBranch(input.session.workspacePath, config.baseBranch, 1);
    await ensureMergeBaseWithBaseBranch(input.session);
    const [behindCountText = '0', aheadCountText = '0'] = (await shCapture('git', ['rev-list', '--left-right', '--count', `origin/${config.baseBranch}...HEAD`], input.session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK }))
        .trim()
        .split(/\s+/);
    const behindCount = Number.parseInt(behindCountText, 10) || 0;
    const aheadCount = Number.parseInt(aheadCountText, 10) || 0;
    if (aheadCount === 0) {
        const before = (await shCapture('git', ['rev-parse', 'HEAD'], input.session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        if (behindCount > 0) {
            await shCapture('git', ['merge', '--ff-only', `origin/${config.baseBranch}`], input.session.workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
        }
        await shCapture('git', [
            'commit',
            '--allow-empty',
            '--no-verify',
            '-m',
            `agent(${input.session.sessionId}): open draft PR`,
        ], input.session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        const after = (await shCapture('git', ['rev-parse', 'HEAD'], input.session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await pushSessionBranch(input.session);
        void postSessionBreadcrumb(input.session, 'open-pr-marker-commit', {
            baseBranch: config.baseBranch,
            before,
            after,
            fastForwardedBase: behindCount > 0,
        });
    }
    const commitTitle = (await shCapture('git', ['log', '-1', '--pretty=%s'], input.session.workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    }))
        .trim()
        .replace(/\s+/g, ' ');
    const rawTitle = input.threadTitle?.trim() || commitTitle || input.prompt?.trim() || input.session.sessionId;
    const title = rawTitle.startsWith('WIP - ') ? rawTitle : `WIP - ${rawTitle}`;
    const body = [
        'WIP',
        '',
        `Thread: ${input.session.sessionId}`,
        `Task: ${input.taskId ?? 'manual-open-pr'}`,
        `Repo: ${config.repoUrl ?? 'unknown'}`,
        `Branch: ${input.session.branch}`,
        '',
        input.prompt ? `Prompt:\n\n${input.prompt}` : 'Opened by dd-dev-server.',
    ].join('\n');
    const out = await shCapture('gh', [
        'pr',
        'create',
        '--draft',
        '--base',
        config.baseBranch,
        '--head',
        input.session.branch,
        '--title',
        title,
        '--body',
        body,
    ], input.session.workspacePath, { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv });
    const prUrl = out
        .trim()
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .pop();
    if (!prUrl) {
        throw new Error('gh pr create did not return a PR URL');
    }
    return {
        ok: true,
        threadId: input.session.sessionId,
        branch: input.session.branch,
        baseBranch: config.baseBranch,
        prUrl,
        title,
        draft: true,
        reused: false,
    };
}
/**
 * Walk the per-task outputs/ directory, publish every regular file via
 * the configured storage adapter, and emit an `artifact` event for each.
 */
async function publishOutputs(state, taskOutputsDir) {
    let dirents;
    try {
        dirents = await readdir(taskOutputsDir, {
            withFileTypes: true,
        });
    }
    catch {
        return; // dir absent / unreadable → nothing to publish, that's fine
    }
    if (dirents.length === 0) {
        return;
    }
    emit(state, { kind: 'status', status: 'publishing-artifacts' });
    // Recurse one level so flat-or-nested layouts both work.
    const filesToPublish = [];
    for (const e of dirents) {
        if (e.isFile()) {
            filesToPublish.push(join(taskOutputsDir, e.name));
        }
        else if (e.isDirectory()) {
            try {
                const sub = await readdir(join(taskOutputsDir, e.name), {
                    withFileTypes: true,
                });
                for (const s of sub) {
                    if (s.isFile()) {
                        filesToPublish.push(join(taskOutputsDir, e.name, s.name));
                    }
                }
            }
            catch {
                /* ignore */
            }
        }
    }
    for (const filePath of filesToPublish) {
        try {
            const st = await stat(filePath);
            const filename = basename(filePath);
            const published = await publishArtifact({
                taskId: state.taskId,
                filePath,
                filename,
                provider: config.defaultStorageProvider,
            });
            // Surface size if the adapter didn't (some stub paths return only
            // the URL); helpful for the UI.
            if (published.sizeBytes === undefined) {
                published.sizeBytes = st.size;
            }
            emit(state, { kind: 'artifact', artifact: published });
        }
        catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            emit(state, {
                kind: 'error',
                message: `failed to publish ${basename(filePath)}: ${message}`,
            });
        }
    }
}
// ---------- HTTP server ----------
const fastify = Fastify({
    logger: true,
    // Reuse caller-supplied x-request-id when present so dev-server joins
    // the upstream trace started by rest-api / Vercel / a sibling worker.
    // Otherwise mint a fresh UUID (Fastify's default is a small counter,
    // which collides across pods).
    genReqId: (req) => {
        const incoming = req.headers['x-request-id'];
        if (typeof incoming === 'string' && incoming.length > 0 && incoming.length <= 200) {
            return incoming;
        }
        if (Array.isArray(incoming) && incoming[0] && incoming[0].length <= 200) {
            return incoming[0];
        }
        return randomUUID();
    },
});
// Wrap every request in an AsyncLocalStorage context so handlers,
// outbound fetches (./wrapped-fetch.ts), and crash handlers can pin
// activity to the originating request without threading req everywhere.
fastify.addHook('onRequest', (req, reply, done) => {
    const path = req.url.split('?')[0] ?? req.url;
    runWithRequestContext({
        requestId: String(req.id),
        method: req.method,
        route: req.routeOptions?.url ?? path,
        path,
    }, () => {
        reply.header('x-request-id', String(req.id));
        done();
    });
});
// Surface request-pinned context on every error response. The default
// Fastify error handler logs the error but drops async context; this
// one re-tags via annotateError so process-level crash handlers can
// still recover the request id if the error escapes.
fastify.setErrorHandler((err, req, reply) => {
    const enriched = annotateError(err);
    const snapshot = snapshotRequestContext();
    req.log.error({
        err: enriched,
        requestContext: snapshot,
    }, 'request failed');
    if (reply.sent)
        return;
    const status = err.statusCode &&
        Number.isInteger(err.statusCode)
        ? err.statusCode
        : 500;
    reply.code(status).send({
        error: enriched.message,
        requestId: snapshot?.requestId,
    });
});
const counters = new Map();
const activeWorkerSockets = new Set();
const activeTerminalSockets = new Set();
function metricKey(name, labels) {
    return `${name}:${Object.entries(labels)
        .filter((entry) => entry[1] !== undefined)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, value]) => `${key}=${value}`)
        .join(',')}`;
}
function incCounter(name, labels = {}, amount = 1) {
    const key = metricKey(name, labels);
    const current = counters.get(key) ?? { labels, value: 0 };
    current.value += amount;
    counters.set(key, current);
}
function renderLabels(labels) {
    const entries = Object.entries(labels).filter((entry) => entry[1] !== undefined);
    if (entries.length === 0) {
        return '';
    }
    return `{${entries
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, value]) => `${key}="${String(value).replace(/"/g, '\\"')}"`)
        .join(',')}}`;
}
function renderCounter(name, help) {
    const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} counter`];
    for (const entry of counters.entries()) {
        if (!entry[0].startsWith(`${name}:`)) {
            continue;
        }
        lines.push(`${name}${renderLabels(entry[1].labels)} ${entry[1].value}`);
    }
    return lines;
}
function renderGauge(name, help, value) {
    return [`# HELP ${name} ${help}`, `# TYPE ${name} gauge`, `${name} ${value}`];
}
function renderMetrics() {
    const now = Date.now();
    const startedAtMs = Date.parse(serverStartedAt);
    const lines = [
        ...renderCounter('dd_runtime_http_requests_total', 'HTTP requests observed by the dd remote runtime.'),
        ...renderCounter('dd_runtime_events_total', 'Task stream events emitted by the dd remote runtime.'),
        ...renderCounter('dd_runtime_tasks_total', 'Task dispatches accepted by the dd remote runtime.'),
        ...renderCounter('dd_runtime_tasks_queued_behind_total', 'Task dispatches accepted while another task was running or already queued for the same worker session.'),
        ...renderGauge('dd_runtime_inflight_tasks', 'Tasks that are currently not finished.', Array.from(tasks.values()).filter((t) => !t.finished).length),
        ...renderGauge('dd_runtime_queued_tasks', 'Accepted tasks waiting behind a per-session workspace queue.', totalQueuedTaskCount()),
        ...renderGauge('dd_runtime_tracked_tasks', 'Tasks retained in memory for stream replay or GC.', tasks.size),
        ...renderGauge('dd_runtime_sessions', 'Thread sessions retained in this worker.', sessions.size),
        ...renderGauge('dd_runtime_uptime_seconds', 'Worker process uptime in seconds.', Math.max(0, Math.round((now - startedAtMs) / 1000))),
        ...renderCounter('dd_runtime_worker_ws_connections_total', 'Worker websocket connections accepted by the dd remote runtime.'),
        ...renderCounter('dd_runtime_worker_ws_messages_total', 'Worker websocket messages observed by the dd remote runtime.'),
        ...renderGauge('dd_runtime_worker_ws_active_connections', 'Currently active worker websocket connections.', activeWorkerSockets.size),
        ...renderGauge('dd_runtime_terminal_ws_active_connections', 'Currently active worker terminal websocket connections.', activeTerminalSockets.size),
    ];
    return `${lines.join('\n')}\n`;
}
function headerMatches(value, expected) {
    if (Array.isArray(value)) {
        return value.includes(expected);
    }
    return value === expected;
}
function rejectUpgrade(socket, status, message) {
    const body = JSON.stringify({ error: 'unauthorized', errMessage: message });
    socket.write(`HTTP/1.1 ${status} ${status === 401 ? 'Unauthorized' : 'Bad Request'}\r\n` +
        'Content-Type: application/json\r\n' +
        `Content-Length: ${Buffer.byteLength(body)}\r\n` +
        'Connection: close\r\n' +
        '\r\n' +
        body);
    socket.destroy();
}
function createWebSocketAcceptKey(key) {
    return createHash('sha1').update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');
}
function writeWebSocketFrame(socket, opcode, payload = Buffer.alloc(0)) {
    const body = Buffer.isBuffer(payload) ? payload : Buffer.from(payload, 'utf8');
    const length = body.length;
    let header;
    if (length < 126) {
        header = Buffer.from([0x80 | opcode, length]);
    }
    else if (length <= 0xffff) {
        header = Buffer.alloc(4);
        header[0] = 0x80 | opcode;
        header[1] = 126;
        header.writeUInt16BE(length, 2);
    }
    else {
        header = Buffer.alloc(10);
        header[0] = 0x80 | opcode;
        header[1] = 127;
        header.writeBigUInt64BE(BigInt(length), 2);
    }
    socket.write(Buffer.concat([header, body]));
}
function taskEventPayload(ev) {
    const task = tasks.get(ev.taskId);
    return {
        type: 'task-event',
        source: 'node-worker-ws',
        serverInstanceId,
        taskId: ev.taskId,
        threadId: ev.threadId ?? task?.threadId ?? config.threadId,
        userId: ev.userId ?? task?.userId,
        provider: task?.provider,
        branch: task?.branch,
        seq: ev.seq,
        emittedAt: new Date().toISOString(),
        event: ev.event,
    };
}
class WorkerWebSocketClient {
    socket;
    threadId;
    taskId;
    buffer = Buffer.alloc(0);
    closed = false;
    subscription;
    heartbeat;
    constructor(socket, threadId, taskId) {
        this.socket = socket;
        this.threadId = threadId;
        this.taskId = taskId;
        activeWorkerSockets.add(this);
        incCounter('dd_runtime_worker_ws_connections_total', {
            service: 'dd-dev-server-api',
            threadId: this.threadId,
        });
        this.subscription = eventBus.all$
            .pipe(filter((ev) => this.shouldForward(ev)))
            .subscribe((ev) => this.sendJson(taskEventPayload(ev)));
        this.heartbeat = setInterval(() => {
            this.sendJson({
                type: 'worker-heartbeat',
                source: 'node-worker-ws',
                serverInstanceId,
                threadId: this.threadId,
                taskId: this.taskId,
                pinnedThreadId: config.threadId,
                inFlightCount: Array.from(tasks.values()).filter((task) => !task.finished).length,
                queuedTaskCount: totalQueuedTaskCount(),
                totalTracked: tasks.size,
                sessionCount: sessions.size,
                atMs: Date.now(),
            });
        }, 25_000);
        this.socket.on('data', (chunk) => this.receive(chunk));
        this.socket.on('close', () => this.close());
        this.socket.on('error', () => this.close());
        this.sendJson({
            type: 'worker-welcome',
            source: 'node-worker-ws',
            serverInstanceId,
            startedAt: serverStartedAt,
            threadId: this.threadId,
            taskId: this.taskId,
            pinnedThreadId: config.threadId,
            inFlightCount: Array.from(tasks.values()).filter((task) => !task.finished).length,
            queuedTaskCount: totalQueuedTaskCount(),
            totalTracked: tasks.size,
            sessionCount: sessions.size,
            atMs: Date.now(),
        });
        this.replayExistingEvents();
    }
    receive(chunk) {
        if (this.closed) {
            return;
        }
        this.buffer = Buffer.concat([this.buffer, chunk]);
        while (this.buffer.length >= 2) {
            const first = this.buffer.readUInt8(0);
            const second = this.buffer.readUInt8(1);
            const opcode = first & 0x0f;
            const masked = (second & 0x80) === 0x80;
            let length = second & 0x7f;
            let offset = 2;
            if (length === 126) {
                if (this.buffer.length < 4) {
                    return;
                }
                length = this.buffer.readUInt16BE(2);
                offset = 4;
            }
            else if (length === 127) {
                if (this.buffer.length < 10) {
                    return;
                }
                const longLength = this.buffer.readBigUInt64BE(2);
                if (longLength > BigInt(Number.MAX_SAFE_INTEGER)) {
                    this.close();
                    return;
                }
                length = Number(longLength);
                offset = 10;
            }
            const maskOffset = offset;
            if (masked) {
                offset += 4;
            }
            const frameLength = offset + length;
            if (this.buffer.length < frameLength) {
                return;
            }
            let payload = this.buffer.subarray(offset, frameLength);
            if (masked) {
                const mask = this.buffer.subarray(maskOffset, maskOffset + 4);
                payload = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]));
            }
            this.buffer = this.buffer.subarray(frameLength);
            if (opcode === 0x8) {
                this.close();
                return;
            }
            if (opcode === 0x9) {
                writeWebSocketFrame(this.socket, 0x0a, payload);
                continue;
            }
            if (opcode === 0x1) {
                this.handleText(payload.toString('utf8'));
            }
        }
    }
    handleText(text) {
        let parsed;
        try {
            parsed = JSON.parse(text);
        }
        catch {
            this.sendJson({
                type: 'worker-error',
                source: 'node-worker-ws',
                code: 'invalid_json',
                message: 'send JSON text frames',
                atMs: Date.now(),
            });
            return;
        }
        const payload = isWebSocketJsonObject(parsed) ? parsed : {};
        const messageType = typeof payload.type === 'string' ? payload.type : 'message';
        incCounter('dd_runtime_worker_ws_messages_total', {
            service: 'dd-dev-server-api',
            messageType,
        });
        if (messageType === 'ping') {
            this.sendJson({
                type: 'worker-pong',
                source: 'node-worker-ws',
                threadId: this.threadId,
                taskId: this.taskId,
                atMs: Date.now(),
            });
            return;
        }
        if (messageType === 'subscribe' || messageType === 'status') {
            this.sendJson({
                type: 'worker-status',
                source: 'node-worker-ws',
                serverInstanceId,
                threadId: this.threadId,
                taskId: this.taskId,
                pinnedThreadId: config.threadId,
                taskExists: this.taskId ? tasks.has(this.taskId) : undefined,
                inFlightCount: Array.from(tasks.values()).filter((task) => !task.finished).length,
                queuedTaskCount: totalQueuedTaskCount(),
                totalTracked: tasks.size,
                sessionCount: sessions.size,
                atMs: Date.now(),
            });
            this.replayExistingEvents();
            return;
        }
        this.sendJson({
            type: 'worker-error',
            source: 'node-worker-ws',
            code: 'unsupported_type',
            message: 'supported types: subscribe, status, ping',
            receivedType: messageType,
            atMs: Date.now(),
        });
    }
    replayExistingEvents() {
        const candidates = this.taskId
            ? [tasks.get(this.taskId)].filter((task) => Boolean(task))
            : Array.from(tasks.values()).filter((task) => task.threadId === this.threadId);
        if (candidates.length === 0) {
            this.sendJson({
                type: 'worker-status',
                source: 'node-worker-ws',
                status: 'waiting-for-task',
                threadId: this.threadId,
                taskId: this.taskId,
                atMs: Date.now(),
            });
            return;
        }
        for (const task of candidates) {
            for (const stored of task.events) {
                this.sendJson(taskEventPayload({
                    taskId: task.taskId,
                    threadId: task.threadId,
                    userId: task.userId,
                    seq: stored.seq,
                    event: stored.event,
                }));
            }
        }
    }
    shouldForward(ev) {
        if (this.taskId && ev.taskId === this.taskId) {
            return true;
        }
        return Boolean(this.threadId && ev.threadId === this.threadId);
    }
    sendJson(payload) {
        if (this.closed || this.socket.destroyed) {
            return;
        }
        writeWebSocketFrame(this.socket, 0x1, JSON.stringify(payload));
    }
    close() {
        if (this.closed) {
            return;
        }
        this.closed = true;
        clearInterval(this.heartbeat);
        this.subscription.unsubscribe();
        activeWorkerSockets.delete(this);
        try {
            writeWebSocketFrame(this.socket, 0x8);
        }
        catch {
            /* socket may already be gone */
        }
        this.socket.destroy();
    }
}
function terminalPageHtml(threadId) {
    const encodedThreadId = JSON.stringify(threadId);
    return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Thread terminal</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css">
  <script defer src="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"></script>
  <style>
    :root { color-scheme: dark; --bg: #0d1117; --panel: #111827; --line: #263244; --text: #e5edf7; --muted: #9aa7b7; --accent: #7dd3fc; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100dvh; background: var(--bg); color: var(--text); font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    main { min-height: 100dvh; display: grid; grid-template-rows: auto minmax(0, 1fr); }
    header { display: flex; justify-content: space-between; gap: 12px; align-items: center; padding: 12px 14px; border-bottom: 1px solid var(--line); background: #0f172a; }
    h1 { margin: 0; font-size: 16px; font-weight: 650; }
    #status { color: var(--muted); font-size: 13px; }
    #terminal { min-height: 0; padding: 10px; background: #05080d; }
    #terminal .xterm { height: 100%; }
    #terminal .xterm-viewport { overflow-y: auto; }
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Thread terminal</h1>
      <span id="status">connecting</span>
    </header>
    <div id="terminal" aria-label="Thread worker terminal"></div>
  </main>
  <script>
    const threadId = ${encodedThreadId};
    const statusNode = document.getElementById("status");
    const terminalNode = document.getElementById("terminal");
    let socket;
    let term;
    function write(value) {
      if (term) term.write(String(value || ""));
    }
    function connect() {
      if (!window.Terminal) {
        statusNode.textContent = "terminal assets failed";
        terminalNode.textContent = "Terminal assets failed to load.";
        return;
      }
      term = new Terminal({
        cursorBlink: true,
        convertEol: true,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        fontSize: 13,
        rows: 32,
        scrollback: 5000,
        theme: {
          background: '#05080d',
          foreground: '#d5f5e3',
          cursor: '#7dd3fc',
          selectionBackground: '#1f6feb66'
        }
      });
      term.open(terminalNode);
      term.focus();
      const url = new URL("terminal/ws", window.location.href);
      url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      url.searchParams.set("threadId", threadId);
      socket = new WebSocket(url);
      socket.addEventListener("open", () => {
        statusNode.textContent = "connected";
        term.onData((data) => {
          if (!socket || socket.readyState !== WebSocket.OPEN) return;
          socket.send(JSON.stringify({ type: "input", data }));
        });
      });
      socket.addEventListener("message", (event) => {
        let message;
        try {
          message = JSON.parse(event.data);
        } catch {
          write(String(event.data));
          return;
        }
        if (message.type === "terminal-output") write(String(message.data || ""));
        if (message.type === "terminal-status") statusNode.textContent = String(message.status || "status");
        if (message.type === "terminal-error") {
          statusNode.textContent = "error";
          write("\\r\\n" + String(message.message || "terminal error") + "\\r\\n");
        }
        if (message.type === "terminal-exit") {
          statusNode.textContent = "closed";
        }
      });
      socket.addEventListener("close", () => {
        statusNode.textContent = "closed";
      });
      socket.addEventListener("error", () => {
        statusNode.textContent = "connection error";
      });
    }
    window.addEventListener("load", connect);
  </script>
</body>
</html>`;
}
async function executableAvailable(path) {
    try {
        await access(path);
        return true;
    }
    catch {
        return false;
    }
}
function terminalShellCommand(shell) {
    const normalized = shell.trim();
    if (normalized === 'bash' || normalized.endsWith('/bash'))
        return 'bash -i';
    if (normalized === 'sh' || normalized.endsWith('/sh'))
        return 'sh -i';
    return 'bash -i';
}
class TerminalWebSocketClient {
    socket;
    threadId;
    buffer = Buffer.alloc(0);
    closed = false;
    child;
    constructor(socket, threadId) {
        this.socket = socket;
        this.threadId = threadId;
        activeTerminalSockets.add(this);
        incCounter('dd_runtime_terminal_ws_connections_total', {
            service: 'dd-dev-server-api',
            threadId: this.threadId,
        });
        this.socket.on('data', (chunk) => this.receive(chunk));
        this.socket.on('close', () => this.close());
        this.socket.on('error', () => this.close());
        void this.start();
    }
    async start() {
        try {
            if (config.threadId && this.threadId !== config.threadId) {
                throw new Error(`container is pinned to thread ${config.threadId}, got ${this.threadId}`);
            }
            await waitForBootGitReady();
            const session = getOrCreateSession({
                taskId: randomUUID(),
                threadId: this.threadId,
                userId: config.userId ?? undefined,
            });
            await session.ready;
            session.lastActiveAt = Date.now();
            this.sendJson({
                type: 'terminal-status',
                source: 'node-worker-terminal',
                status: 'starting-shell',
                threadId: this.threadId,
                branch: session.branch,
                cwd: session.workspacePath,
                atMs: Date.now(),
            });
            const shell = process.env.SHELL || '/bin/bash';
            const scriptBin = process.env.TERMINAL_SCRIPT_BIN || '/usr/bin/script';
            const usePty = await executableAvailable(scriptBin);
            const terminalEnv = {
                ...process.env,
                SHELL: shell,
                TERM: process.env.TERM || 'xterm-256color',
                COLORTERM: process.env.COLORTERM || 'truecolor',
                COLUMNS: process.env.COLUMNS || '120',
                LINES: process.env.LINES || '32',
                FORCE_COLOR: process.env.FORCE_COLOR || '1',
                PS1: '\\w $ ',
            };
            const fallbackUsesBash = terminalShellCommand(shell).startsWith('bash');
            const fallbackShell = fallbackUsesBash ? 'bash' : 'sh';
            const fallbackArgs = fallbackUsesBash ? ['--noprofile', '--norc', '-i'] : ['-i'];
            this.child = usePty
                ? spawn(scriptBin, ['-q', '-f', '-e', '-c', terminalShellCommand(shell), '/dev/null'], {
                    cwd: session.workspacePath,
                    env: terminalEnv,
                    stdio: ['pipe', 'pipe', 'pipe'],
                })
                : spawn(fallbackShell, fallbackArgs, {
                    cwd: session.workspacePath,
                    env: terminalEnv,
                    stdio: ['pipe', 'pipe', 'pipe'],
                });
            this.child.stdout?.on('data', (chunk) => {
                this.sendOutput(chunk.toString('utf8'));
            });
            this.child.stderr?.on('data', (chunk) => {
                this.sendOutput(chunk.toString('utf8'));
            });
            this.child.on('close', (code, signal) => {
                this.sendJson({
                    type: 'terminal-exit',
                    source: 'node-worker-terminal',
                    code,
                    signal,
                    atMs: Date.now(),
                });
                this.close();
            });
            this.child.on('error', (error) => {
                this.sendJson({
                    type: 'terminal-error',
                    source: 'node-worker-terminal',
                    message: error.message,
                    atMs: Date.now(),
                });
                this.close();
            });
            this.sendJson({
                type: 'terminal-status',
                source: 'node-worker-terminal',
                status: 'connected',
                threadId: this.threadId,
                branch: session.branch,
                cwd: session.workspacePath,
                transport: usePty ? 'pty-script' : 'pipe-fallback',
                atMs: Date.now(),
            });
        }
        catch (error) {
            this.sendJson({
                type: 'terminal-error',
                source: 'node-worker-terminal',
                message: error instanceof Error ? error.message : String(error),
                atMs: Date.now(),
            });
            this.close();
        }
    }
    receive(chunk) {
        if (this.closed) {
            return;
        }
        this.buffer = Buffer.concat([this.buffer, chunk]);
        while (this.buffer.length >= 2) {
            const first = this.buffer.readUInt8(0);
            const second = this.buffer.readUInt8(1);
            const opcode = first & 0x0f;
            const masked = (second & 0x80) === 0x80;
            let length = second & 0x7f;
            let offset = 2;
            if (length === 126) {
                if (this.buffer.length < 4) {
                    return;
                }
                length = this.buffer.readUInt16BE(2);
                offset = 4;
            }
            else if (length === 127) {
                if (this.buffer.length < 10) {
                    return;
                }
                const longLength = this.buffer.readBigUInt64BE(2);
                if (longLength > BigInt(Number.MAX_SAFE_INTEGER)) {
                    this.close();
                    return;
                }
                length = Number(longLength);
                offset = 10;
            }
            const maskOffset = offset;
            if (masked) {
                offset += 4;
            }
            const frameLength = offset + length;
            if (this.buffer.length < frameLength) {
                return;
            }
            let payload = this.buffer.subarray(offset, frameLength);
            if (masked) {
                const mask = this.buffer.subarray(maskOffset, maskOffset + 4);
                payload = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]));
            }
            this.buffer = this.buffer.subarray(frameLength);
            if (opcode === 0x8) {
                this.close();
                return;
            }
            if (opcode === 0x9) {
                writeWebSocketFrame(this.socket, 0x0a, payload);
                continue;
            }
            if (opcode === 0x1) {
                this.handleText(payload.toString('utf8'));
            }
        }
    }
    handleText(text) {
        let parsed;
        try {
            parsed = JSON.parse(text);
        }
        catch {
            this.sendJson({
                type: 'terminal-error',
                source: 'node-worker-terminal',
                message: 'send JSON text frames',
                atMs: Date.now(),
            });
            return;
        }
        const payload = isWebSocketJsonObject(parsed) ? parsed : {};
        const messageType = typeof payload.type === 'string' ? payload.type : 'message';
        incCounter('dd_runtime_terminal_ws_messages_total', {
            service: 'dd-dev-server-api',
            messageType,
        });
        if (messageType === 'ping') {
            this.sendJson({
                type: 'terminal-pong',
                source: 'node-worker-terminal',
                threadId: this.threadId,
                atMs: Date.now(),
            });
            return;
        }
        if (messageType === 'input') {
            const data = typeof payload.data === 'string' ? payload.data : '';
            if (data && this.child?.stdin?.writable) {
                this.child.stdin.write(data);
            }
            return;
        }
        this.sendJson({
            type: 'terminal-error',
            source: 'node-worker-terminal',
            message: 'supported types: input, ping',
            receivedType: messageType,
            atMs: Date.now(),
        });
    }
    sendOutput(data) {
        this.sendJson({
            type: 'terminal-output',
            source: 'node-worker-terminal',
            data,
            atMs: Date.now(),
        });
    }
    sendJson(payload) {
        if (this.closed || this.socket.destroyed) {
            return;
        }
        writeWebSocketFrame(this.socket, 0x1, JSON.stringify(payload));
    }
    close() {
        if (this.closed) {
            return;
        }
        this.closed = true;
        activeTerminalSockets.delete(this);
        if (this.child && !this.child.killed) {
            try {
                this.child.kill('SIGHUP');
            }
            catch {
                /* shell may already be gone */
            }
        }
        try {
            writeWebSocketFrame(this.socket, 0x8);
        }
        catch {
            /* socket may already be gone */
        }
        this.socket.destroy();
    }
}
function registerWorkerWebSocketUpgrade() {
    fastify.server.on('upgrade', (request, socket, head) => {
        const requestUrl = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);
        if (requestUrl.pathname !== '/ws' && requestUrl.pathname !== '/terminal/ws') {
            rejectUpgrade(socket, 404, 'websocket path not found');
            return;
        }
        if (!config.serverAuthSecret ||
            !headerMatches(request.headers['x-server-auth'], config.serverAuthSecret)) {
            rejectUpgrade(socket, 401, 'missing required dd header');
            return;
        }
        const key = request.headers['sec-websocket-key'];
        if (!key || Array.isArray(key)) {
            rejectUpgrade(socket, 400, 'missing websocket key');
            return;
        }
        const requestedThreadId = requestUrl.searchParams.get('threadId') ?? config.threadId;
        const taskId = requestUrl.searchParams.get('taskId') ?? undefined;
        if (!requestedThreadId) {
            rejectUpgrade(socket, 400, 'threadId is required');
            return;
        }
        if (config.threadId && requestedThreadId !== config.threadId) {
            rejectUpgrade(socket, 409, 'container is bound to a different thread');
            return;
        }
        socket.write('HTTP/1.1 101 Switching Protocols\r\n' +
            'Upgrade: websocket\r\n' +
            'Connection: Upgrade\r\n' +
            `Sec-WebSocket-Accept: ${createWebSocketAcceptKey(key)}\r\n` +
            '\r\n');
        const client = requestUrl.pathname === '/terminal/ws'
            ? new TerminalWebSocketClient(socket, requestedThreadId)
            : new WorkerWebSocketClient(socket, requestedThreadId, taskId);
        if (head.length > 0) {
            client.receive(head);
        }
    });
}
fastify.addHook('preHandler', async (req, reply) => {
    const requestPath = req.url.split('?')[0] ?? req.url;
    if (requestPath === '/healthz' ||
        requestPath === '/metrics' ||
        requestPath === '/favicon.ico' ||
        requestPath === '/docs/api' ||
        requestPath === '/api/docs' ||
        requestPath === '/api/docs.json') {
        return;
    }
    // GET /stream/:taskId may auth via short-lived HMAC token (?token=)
    // for direct browser → docker SSE connections that bypass Vercel's
    // 800s function cap. Defer that check to the route handler.
    if (req.method === 'GET' && requestPath.startsWith('/stream/')) {
        return;
    }
    // The runtime-config receive helper does its own X-Server-Auth check
    // against RUNTIME_CONFIG_SERVER_SECRET; defer to it so operators can use a
    // different secret for the config control plane if they want to.
    if (requestPath.startsWith('/internal/runtime-config') ||
        requestPath === '/internal/update-runtime-config') {
        return;
    }
    if (!config.serverAuthSecret || req.headers['x-server-auth'] !== config.serverAuthSecret) {
        return reply.code(401).send({ error: 'unauthorized' });
    }
});
fastify.addHook('onResponse', async (req, reply) => {
    const requestPath = req.url.split('?')[0] ?? req.url;
    incCounter('dd_runtime_http_requests_total', {
        service: 'dd-dev-server-api',
        method: req.method,
        path: requestPath,
        status: reply.statusCode,
    });
});
fastify.get('/favicon.ico', async (_req, reply) => {
    return reply.code(204).send();
});
fastify.get('/healthz', async () => ({
    ok: true,
    startedAt: serverStartedAt,
    serverInstanceId,
    pinnedThreadId: config.threadId,
    pinnedUserId: config.userId,
    inFlightCount: Array.from(tasks.values()).filter((t) => !t.finished).length,
    queuedTaskCount: totalQueuedTaskCount(),
    totalTracked: tasks.size,
    sessionCount: sessions.size,
    containerPoolConfigured: containerPoolConfigured(config.containerPool),
}));
fastify.get('/metrics', async (_req, reply) => {
    reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
    return renderMetrics();
});
fastify.get('/docs/api', async (_req, reply) => {
    reply.header('content-type', 'text/html; charset=utf-8');
    return readFile(new URL('../generated/api-docs.html', import.meta.url), 'utf8');
});
fastify.get('/api/docs', async (_req, reply) => {
    reply.header('content-type', 'text/html; charset=utf-8');
    return readFile(new URL('../generated/api-docs.html', import.meta.url), 'utf8');
});
fastify.get('/api/docs.json', async (_req, reply) => {
    reply.header('content-type', 'application/json; charset=utf-8');
    return readFile(new URL('../generated/api-docs.json', import.meta.url), 'utf8');
});
registerRuntimeConfigRoutes(fastify);
void registerWithControlPlane();
fastify.get('/status', async () => ({
    ok: true,
    serverInstanceId,
    startedAt: serverStartedAt,
    pinnedThreadId: config.threadId,
    pinnedUserId: config.userId,
    inFlightCount: Array.from(tasks.values()).filter((t) => !t.finished).length,
    queuedTaskCount: totalQueuedTaskCount(),
    totalTracked: tasks.size,
    sessionCount: sessions.size,
    containerPoolConfigured: containerPoolConfigured(config.containerPool),
    ingestCircuit: eventBus.getCircuitState(),
    idleTimeoutMs: config.threadId ? config.idleTimeoutMs : undefined,
}));
fastify.get('/terminal', async (req, reply) => {
    const parsed = TerminalQuerySchema.safeParse(req.query ?? {});
    if (!parsed.success) {
        return reply.code(400).send({ error: parsed.error.format() });
    }
    const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
    if (!threadId) {
        return reply.code(400).send({ error: 'threadId is required' });
    }
    if (config.threadId && threadId !== config.threadId) {
        return reply.code(409).send({
            error: 'container is bound to a different thread',
            boundThreadId: config.threadId,
        });
    }
    reply.header('content-type', 'text/html; charset=utf-8');
    return terminalPageHtml(threadId);
});
// Provider availability — boot-probed list of which AGENT_PROVIDER
// values can actually be used in this image (binaries on PATH, SDKs
// installed, API keys set). UI uses this to grey out unavailable
// options instead of letting the user pick something that fails with
// ENOENT mid-run.
fastify.get('/agents', async () => {
    const cached = getCachedAvailability();
    const list = cached ?? (await probeAllProviders());
    return {
        default: resolveAgentProvider(),
        providers: list,
    };
});
// Snapshot of every task currently in memory (live + finished-but-not-GC'd).
// Vercel calls this from the page server-side on first load to merge with
// what's in NeonDB — so the UI shows the absolute latest state even if the
// last few events haven't been written through to NeonDB yet.
fastify.get('/tasks', async () => {
    const snapshot = Array.from(tasks.values()).map((t) => {
        const queue = taskQueueSnapshot(t);
        return {
            taskId: t.taskId,
            threadId: t.threadId,
            userId: t.userId,
            branch: t.branch,
            sessionId: t.session.sessionId,
            running: queue.running,
            queued: queue.queued,
            queuePosition: queue.queuePosition,
            finished: t.finished,
            finishedAt: t.finishedAt,
            eventCount: t.events.length,
            lastSeq: t.events.length > 0 ? t.events[t.events.length - 1].seq : -1,
        };
    });
    return { tasks: snapshot, serverStartedAt };
});
const ContainerPoolRequestSchema = z.object({
    requestId: z.string().min(1).max(200).optional(),
    poolId: z.string().min(1).max(120).optional(),
    poolSlug: z.string().min(1).max(120).optional(),
    path: z.string().min(1).max(256).optional(),
    headers: z.record(z.string(), z.string()).optional(),
    payload: z.unknown().optional(),
    body: z.unknown().optional(),
});
const ContainerPoolTaskSchema = ContainerPoolRequestSchema.extend({
    pool: z.string().min(1).max(120),
});
const SelectedContextBlobSchema = z.object({
    contextId: z.string().min(1).max(200),
    projectId: z.string().max(120).optional(),
    repoId: z.string().uuid().nullish(),
    contextTitle: z.string().min(1).max(300),
    contextBlob: z.string().min(1).max(200_000),
    score: z.number().optional(),
    matchSource: z.string().min(1).max(80).optional(),
    embeddingModel: z.string().min(1).max(120).nullish(),
    updatedAt: z.string().min(1).max(80).nullish(),
    // 'context-blob' (long-lived agent_context_blobs row), 'thread-task'
    // (previous agent_remote_dev_tasks row), or 'breadcrumb'
    // (agent_remote_dev_breadcrumbs row). Older dispatchers don't set this;
    // treat omitted as 'context-blob'.
    kind: z.enum(['context-blob', 'thread-task', 'breadcrumb']).optional(),
});
fastify.post('/container-pools/:pool/dispatch', async (req, reply) => {
    const params = z.object({ pool: z.string().min(1).max(120) }).safeParse(req.params);
    const parsed = ContainerPoolRequestSchema.safeParse(req.body);
    if (!params.success || !parsed.success) {
        return reply.code(400).send({
            error: 'invalid container pool dispatch',
            params: params.success ? undefined : params.error.format(),
            body: parsed.success ? undefined : parsed.error.format(),
        });
    }
    if (parsed.data.requestId) {
        // Container-pool dispatchers carry their own opaque request id (used
        // for dedupe upstream). Surface it on the context.extra so log lines
        // and downstream fetches can pivot by it.
        const ctx = getRequestContext();
        if (ctx)
            ctx.extra.containerPoolRequestId = parsed.data.requestId;
    }
    try {
        return await dispatchContainerPool(config.containerPool, params.data.pool, {
            ...parsed.data,
            poolSlug: parsed.data.poolSlug ?? params.data.pool,
        });
    }
    catch (error) {
        return reply.code(502).send({
            ok: false,
            error: error instanceof Error ? error.message : String(error),
        });
    }
});
const DispatchSchema = z.object({
    taskId: z.string().uuid().optional(),
    prompt: z.string().min(1).max(64_000),
    /** dd-user UUID. When present, events also fan out via Supabase Realtime. */
    userId: z.string().uuid().optional(),
    /** Vercel-side thread id, included in published events for client routing. */
    threadId: z.string().uuid().optional(),
    repo: z.string().min(1).max(2048).optional(),
    baseBranch: z.string().min(1).max(120).optional(),
    /** Stable remote-dev thread branch, if the dispatcher already knows it. */
    branch: z.string().max(200).nullish(),
    /** Human-readable thread title / branch slug fallback. */
    threadTitle: z.string().min(1).max(200).nullish(),
    /**
     * Which agent runner to drive the task. Falls back to AGENT_PROVIDER env
     * then the configured provider rotation. Validated by the selector —
     * unknown values fall back to default rather than 400ing.
     */
    provider: z
        .enum([
        'claude-cli',
        'claude-sdk',
        'generic-ai-sdk',
        'gemini-sdk',
        'opencode-ai-sdk',
        'openai-codex-cli',
        'openai-sdk',
    ])
        .nullish(),
    contextMode: z.enum(['none', 'selected', 'auto']).nullish(),
    contextIds: z.array(z.string().min(1).max(200)).max(50).nullish(),
    contextBlobs: z.array(SelectedContextBlobSchema).max(50).nullish(),
    containerPool: ContainerPoolTaskSchema.nullish(),
});
const MergeUpstreamSchema = z.object({
    kind: z.literal('thread-control').optional(),
    action: z.literal('merge-upstream').optional(),
    taskId: z.string().uuid().optional(),
    threadId: z.string().uuid().optional(),
    userId: z.string().uuid().optional(),
    branch: z.string().max(200).optional(),
    threadTitle: z.string().min(1).max(200).optional(),
    requestedBy: z.string().max(120).optional(),
    reason: z.string().max(300).optional(),
});
const MakeCommitSchema = z.object({
    kind: z.literal('thread-control').optional(),
    action: z.literal('make-commit').optional(),
    taskId: z.string().uuid().optional(),
    threadId: z.string().uuid().optional(),
    userId: z.string().uuid().optional(),
    branch: z.string().max(200).optional(),
    threadTitle: z.string().min(1).max(200).optional(),
    requestedBy: z.string().max(120).optional(),
    reason: z.string().max(300).optional(),
});
const OpenPullRequestSchema = z.object({
    kind: z.literal('thread-control').optional(),
    action: z.literal('open-pr').optional(),
    taskId: z.string().uuid().optional(),
    threadId: z.string().uuid().optional(),
    userId: z.string().uuid().optional(),
    branch: z.string().max(200).optional(),
    threadTitle: z.string().min(1).max(200).optional(),
    requestedBy: z.string().max(120).optional(),
    reason: z.string().max(300).optional(),
});
const TerminalQuerySchema = z.object({
    threadId: z.string().uuid().optional(),
});
fastify.post('/tasks', async (req, reply) => {
    return withSpan('remote-dev.dispatch-task', {
        'http.method': req.method,
        'http.route': '/tasks',
        'dd.remote.thread_id': config.threadId ?? undefined,
    }, async (span) => {
        const parsed = DispatchSchema.safeParse(req.body);
        if (!parsed.success) {
            return reply.code(400).send({ error: parsed.error.format() });
        }
        const { prompt } = parsed.data;
        const taskId = parsed.data.taskId ?? randomUUID();
        span.setAttribute('dd.remote.task_id', taskId);
        setContextField('taskId', taskId);
        const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
        const userId = parsed.data.userId ?? config.userId ?? undefined;
        if (threadId) {
            span.setAttribute('dd.remote.thread_id', threadId);
            setContextField('threadId', threadId);
        }
        if (userId) {
            span.setAttribute('dd.remote.user_id', userId);
            setContextField('userId', userId);
        }
        if (config.threadId && threadId !== config.threadId) {
            return reply.code(409).send({
                error: 'container is bound to a different thread',
                boundThreadId: config.threadId,
            });
        }
        if (config.workerBindMode === 'repo' && !threadId) {
            return reply.code(400).send({
                error: 'threadId is required for repo-scoped warm workers',
            });
        }
        if (config.userId && userId !== config.userId) {
            return reply.code(403).send({
                error: 'container is bound to a different user',
                boundUserId: config.userId,
            });
        }
        const requestedRepo = parsed.data.repo?.trim();
        if (requestedRepo && !repoUrlsMatch(requestedRepo, config.repoUrl)) {
            return reply.code(409).send({
                error: 'container is bound to a different repo',
                boundRepo: config.repoUrl,
                requestedRepo,
            });
        }
        const requestedBaseBranch = parsed.data.baseBranch?.trim();
        if (requestedBaseBranch && requestedBaseBranch !== config.baseBranch) {
            return reply.code(409).send({
                error: 'container is bound to a different baseBranch',
                boundBaseBranch: config.baseBranch,
            });
        }
        const existingTask = tasks.get(taskId);
        if (existingTask) {
            return {
                taskId,
                branch: existingTask.branch,
                duplicate: true,
                status: existingTask.finished ? 'finished' : 'running',
            };
        }
        const receipt = await readTaskReceipt(taskId);
        if (receipt) {
            if (threadId && receipt.threadId && threadId !== receipt.threadId) {
                return reply.code(409).send({
                    error: 'task receipt belongs to a different thread',
                    taskId,
                    receiptThreadId: receipt.threadId,
                });
            }
            return {
                taskId,
                branch: receipt.branch,
                duplicate: true,
                status: 'accepted',
            };
        }
        const session = getOrCreateSession({
            taskId,
            threadId,
            userId,
            branch: parsed.data.branch ?? undefined,
            threadTitle: parsed.data.threadTitle ?? undefined,
            prompt,
        });
        session.taskIds.add(taskId);
        const blockingTaskIds = sessionBlockingTaskIds(session);
        const queuedBehind = blockingTaskIds.length > 0;
        const queuePosition = blockingTaskIds.length + 1;
        const state = {
            taskId,
            prompt,
            userId,
            threadId,
            provider: resolveAgentProvider(parsed.data.provider ?? undefined),
            contextMode: parsed.data.contextMode ?? undefined,
            contextIds: parsed.data.contextIds ?? undefined,
            contextBlobs: parsed.data.contextBlobs ?? undefined,
            containerPool: parsed.data.containerPool
                ? {
                    pool: parsed.data.containerPool.pool,
                    request: {
                        requestId: parsed.data.containerPool.requestId,
                        poolId: parsed.data.containerPool.poolId,
                        poolSlug: parsed.data.containerPool.poolSlug ?? parsed.data.containerPool.pool,
                        path: parsed.data.containerPool.path,
                        headers: parsed.data.containerPool.headers,
                        payload: parsed.data.containerPool.payload,
                        body: parsed.data.containerPool.body,
                    },
                }
                : undefined,
            session,
            abortController: new AbortController(),
            events: [],
            event$: new ReplaySubject(),
            finished: false,
            cancelled: false,
            worktreePath: session.workspacePath,
            branch: session.branch,
        };
        span.setAttribute('dd.remote.provider', state.provider);
        span.setAttribute('dd.remote.branch', state.branch);
        span.setAttribute('dd.remote.queue_position', queuePosition);
        span.setAttribute('dd.remote.queued_behind', queuedBehind);
        setContextField('provider', state.provider);
        tasks.set(taskId, state);
        session.queuedTaskIds.push(taskId);
        emit(state, {
            kind: 'status',
            status: 'queued',
            queueDepth: blockingTaskIds.length + 1,
            queuePosition,
            sessionId: session.sessionId,
        });
        if (queuedBehind) {
            incCounter('dd_runtime_tasks_queued_behind_total', {
                service: 'dd-dev-server-api',
                provider: state.provider,
            });
            emit(state, {
                kind: 'status',
                status: 'queued-behind-active-task',
                message: `Task is queued behind ${blockingTaskIds.length} active task(s) for thread ${session.sessionId}. ` +
                    `It will start after the workspace queue drains.`,
                queueDepth: blockingTaskIds.length + 1,
                queuePosition,
                blockedByTaskId: blockingTaskIds[0],
                blockedByTaskIds: blockingTaskIds.slice(0, 20),
                sessionId: session.sessionId,
            });
        }
        const queuedRun = session.queue
            .catch(() => undefined)
            .then(async () => {
            session.queuedTaskIds = session.queuedTaskIds.filter((id) => id !== taskId);
            session.runningTaskId = taskId;
            emit(state, {
                kind: 'status',
                status: 'dequeued-starting',
                message: `Task has acquired the thread workspace queue for ${session.sessionId}.`,
                queueDepth: session.queuedTaskIds.length + 1,
                queuePosition: 1,
                sessionId: session.sessionId,
            });
            try {
                await runTask(state);
            }
            finally {
                if (session.runningTaskId === taskId) {
                    session.runningTaskId = undefined;
                }
                pruneSessionQueueState(session);
            }
        });
        session.queue = queuedRun.catch(() => undefined);
        queuedRun.catch((err) => {
            const message = err instanceof Error ? err.message : String(err);
            emit(state, { kind: 'error', message });
            if (!state.finished) {
                emit(state, {
                    kind: 'done',
                    branch: state.branch,
                    exitReason: 'failed',
                });
            }
        });
        try {
            await writeTaskReceipt({
                taskId,
                threadId,
                branch: state.branch,
                provider: state.provider,
                acceptedAt: new Date().toISOString(),
            });
        }
        catch (err) {
            emit(state, {
                kind: 'stderr',
                text: `task receipt write failed: ${err instanceof Error ? err.message : String(err)}`,
            });
        }
        return { taskId, branch: state.branch, queuedBehind, queuePosition };
    });
});
fastify.post('/thread/merge-upstream', async (req, reply) => {
    return withSpan('remote-dev.merge-upstream', {
        'http.method': req.method,
        'http.route': '/thread/merge-upstream',
        'dd.remote.thread_id': config.threadId ?? undefined,
    }, async (span) => {
        const parsed = MergeUpstreamSchema.safeParse(req.body ?? {});
        if (!parsed.success) {
            return reply.code(400).send({ error: parsed.error.format() });
        }
        const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
        if (threadId) {
            span.setAttribute('dd.remote.thread_id', threadId);
            setContextField('threadId', threadId);
        }
        if (parsed.data.taskId) {
            setContextField('taskId', parsed.data.taskId);
        }
        try {
            const result = await mergeUpstreamForThread(parsed.data);
            span.setAttribute('dd.remote.branch', result.branch);
            span.setAttribute('dd.remote.base_branch', result.baseBranch);
            return result;
        }
        catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            const status = message.includes('pinned to') ? 409 : 500;
            return reply.code(status).send({ error: message });
        }
    });
});
fastify.post('/thread/make-commit', async (req, reply) => {
    return withSpan('remote-dev.make-commit', {
        'http.method': req.method,
        'http.route': '/thread/make-commit',
        'dd.remote.thread_id': config.threadId ?? undefined,
    }, async (span) => {
        const parsed = MakeCommitSchema.safeParse(req.body ?? {});
        if (!parsed.success) {
            return reply.code(400).send({ error: parsed.error.format() });
        }
        const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
        if (threadId) {
            span.setAttribute('dd.remote.thread_id', threadId);
            setContextField('threadId', threadId);
        }
        if (parsed.data.taskId) {
            setContextField('taskId', parsed.data.taskId);
        }
        try {
            const result = await makeCommitForThread(parsed.data);
            span.setAttribute('dd.remote.branch', result.branch);
            span.setAttribute('dd.remote.committed', result.committed);
            span.setAttribute('dd.remote.pushed', result.pushed);
            return result;
        }
        catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            const status = message.includes('pinned to') ? 409 : 500;
            return reply.code(status).send({ error: message });
        }
    });
});
fastify.post('/thread/open-pr', async (req, reply) => {
    return withSpan('remote-dev.open-pr', {
        'http.method': req.method,
        'http.route': '/thread/open-pr',
        'dd.remote.thread_id': config.threadId ?? undefined,
    }, async (span) => {
        const parsed = OpenPullRequestSchema.safeParse(req.body ?? {});
        if (!parsed.success) {
            return reply.code(400).send({ error: parsed.error.format() });
        }
        const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
        if (threadId) {
            span.setAttribute('dd.remote.thread_id', threadId);
            setContextField('threadId', threadId);
        }
        if (parsed.data.taskId) {
            setContextField('taskId', parsed.data.taskId);
        }
        try {
            const result = await openPullRequestForThread(parsed.data);
            span.setAttribute('dd.remote.branch', result.branch);
            span.setAttribute('dd.remote.base_branch', result.baseBranch);
            span.setAttribute('dd.remote.pr_url', result.prUrl);
            return result;
        }
        catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            const status = message.includes('pinned to') ? 409 : 500;
            return reply.code(status).send({ error: message });
        }
    });
});
fastify.get('/stream/:taskId', (req, reply) => {
    const { taskId } = req.params;
    // Auth: either X-Server-Auth (server-to-server, e.g. Vercel proxy) or
    // a short-lived HMAC token in ?token= for direct browser connections.
    // For direct-browser tokens we ALSO require the token's userId to match
    // the task's owner — otherwise a valid token for task A could be
    // weaponised against task B if its taskId leaked.
    const tokenParam = req.query.token;
    let tokenAuthed = false;
    if (typeof tokenParam === 'string' && tokenParam.length > 0) {
        const payload = verifyDirectStreamToken(tokenParam);
        const candidate = tasks.get(taskId);
        if (!payload ||
            payload.taskId !== taskId ||
            !candidate ||
            candidate.userId !== payload.userId) {
            reply.code(401).send({ error: 'unauthorized' });
            return;
        }
        tokenAuthed = true;
    }
    if (!tokenAuthed &&
        (!config.serverAuthSecret || req.headers['x-server-auth'] !== config.serverAuthSecret)) {
        reply.code(401).send({ error: 'unauthorized' });
        return;
    }
    const state = tasks.get(taskId);
    if (!state) {
        reply.code(404).send({ error: 'not found' });
        return;
    }
    reply.hijack();
    reply.raw.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache, no-transform',
        Connection: 'keep-alive',
        'X-Accel-Buffering': 'no',
    });
    const lastEventIdHeader = req.headers['last-event-id'];
    const resumeFromIdParam = req.query.resumeFromId;
    const lastEventIdRaw = typeof lastEventIdHeader === 'string' ? lastEventIdHeader : resumeFromIdParam;
    const lastEventIdNumber = lastEventIdRaw ? Number(lastEventIdRaw) : -1;
    const lastEventId = Number.isFinite(lastEventIdNumber)
        ? Math.max(-1, Math.trunc(lastEventIdNumber))
        : -1;
    const send = (s) => {
        reply.raw.write(`id: ${s.seq}\nevent: ${s.event.kind}\ndata: ${JSON.stringify(s.event)}\n\n`);
    };
    const disconnected$ = new Subject();
    const subscription = state.event$
        .pipe(filter((s) => s.seq > lastEventId), takeUntil(disconnected$))
        .subscribe({
        next: (s) => {
            send(s);
            if (s.event.kind === 'done') {
                disconnected$.next();
                reply.raw.end();
            }
        },
        complete: () => {
            if (!reply.raw.destroyed) {
                reply.raw.end();
            }
        },
    });
    // Heartbeat keeps proxies (Cloudflare, nginx) from timing the
    // connection out during quiet stretches.
    const heartbeat = setInterval(() => {
        reply.raw.write(`: ping\n\n`);
    }, 25_000);
    req.raw.on('close', () => {
        clearInterval(heartbeat);
        disconnected$.next();
        disconnected$.complete();
        subscription.unsubscribe();
    });
});
fastify.post('/tasks/:taskId/cancel', async (req, reply) => {
    const { taskId } = req.params;
    setContextField('taskId', taskId);
    const state = tasks.get(taskId);
    if (!state) {
        return reply.code(404).send({ error: 'not found' });
    }
    if (state.threadId)
        setContextField('threadId', state.threadId);
    if (state.userId)
        setContextField('userId', state.userId);
    if (state.provider)
        setContextField('provider', state.provider);
    if (state.finished) {
        return reply.code(409).send({ error: 'already finished' });
    }
    // Cancel hits two paths: AbortController for SDK / async runners,
    // SIGTERM for CLI runners that actually have a child process. Both
    // run for safety — child.kill on a non-CLI runner is a no-op and
    // abort on a CLI runner is harmless extra signal.
    state.cancelled = true;
    try {
        state.abortController.abort();
    }
    catch {
        /* already aborted */
    }
    if (state.child && !state.child.killed) {
        state.child.kill('SIGTERM');
    }
    emit(state, {
        kind: 'done',
        branch: state.branch,
        exitReason: 'cancelled',
    });
    return { ok: true };
});
// ---------- GC ----------
setInterval(() => {
    const now = Date.now();
    for (const [id, state] of tasks) {
        if (state.finished &&
            state.finishedAt !== undefined &&
            now - state.finishedAt > config.taskGcAfterMs) {
            tasks.delete(id);
            state.session.taskIds.delete(id);
            eventBus.gcTask(id);
        }
    }
    for (const [sessionId, session] of sessions) {
        if (session.taskIds.size === 0 && now - session.lastActiveAt > config.sessionIdleGcAfterMs) {
            sessions.delete(sessionId);
        }
    }
}, config.taskGcIntervalMs);
// ---------- Heartbeat sender ----------
// Periodically POSTs a snapshot of in-flight tasks to Vercel. The page
// uses this to mark "docker alive" without holding open a connection,
// and to reconcile NeonDB if any per-event POSTs were dropped (since
// /events ingestion is at-least-once but at-least-once means duplicates
// are deduped on `(task_id, seq)` while drops would leave gaps).
//
// In thread-container mode, the first heartbeat also self-registers the
// K8s pod in the Vercel-side routing registry.
let cachedOwnIp = null;
async function sendHeartbeat() {
    if (!config.heartbeatUrl || !config.heartbeatSecret) {
        return;
    }
    if (!cachedOwnIp) {
        cachedOwnIp = await discoverOwnIp();
    }
    const inFlight = Array.from(tasks.values()).map((t) => {
        const queue = taskQueueSnapshot(t);
        return {
            taskId: t.taskId,
            threadId: t.threadId,
            userId: t.userId,
            branch: t.branch,
            running: queue.running,
            queued: queue.queued,
            queuePosition: queue.queuePosition,
            finished: t.finished,
            finishedAt: t.finishedAt,
            eventCount: t.events.length,
            lastSeq: t.events.length > 0 ? t.events[t.events.length - 1].seq : -1,
        };
    });
    const containerInfo = config.threadId
        ? {
            threadId: config.threadId,
            ip: cachedOwnIp,
            port: config.port,
            status: 'ready',
            podName: process.env.POD_NAME ?? process.env.HOSTNAME ?? serverInstanceId,
            namespace: process.env.POD_NAMESPACE ?? process.env.K8S_NAMESPACE ?? '',
            orchestrator: process.env.K8S_API_SERVER
                ? 'k8s'
                : process.env.ECS_CONTAINER_METADATA_URI_V4
                    ? 'ecs'
                    : 'docker-compose',
        }
        : undefined;
    try {
        await contextFetch(config.heartbeatUrl, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-Heartbeat-Auth': config.heartbeatSecret,
            },
            body: JSON.stringify({
                serverInstanceId,
                serverStartedAt,
                sentAt: new Date().toISOString(),
                inFlight,
                queuedTaskCount: totalQueuedTaskCount(),
                container: containerInfo,
            }),
        });
    }
    catch {
        /* best-effort — Vercel will eventually mark docker as down */
    }
}
if (config.heartbeatUrl) {
    void sendHeartbeat();
    interval(Math.max(config.heartbeatIntervalMs, 5_000)).subscribe(() => {
        void sendHeartbeat();
    });
}
// ---------- Boot ----------
async function discoverOwnIp() {
    if (process.env.POD_IP) {
        return process.env.POD_IP;
    }
    const ecsMetaUri = process.env.ECS_CONTAINER_METADATA_URI_V4;
    if (ecsMetaUri) {
        try {
            const res = await contextFetch(`${ecsMetaUri}/task`);
            if (res.ok) {
                const meta = (await res.json());
                const ip = meta.Containers?.[0]?.Networks?.[0]?.IPv4Addresses?.[0];
                if (ip) {
                    return ip;
                }
            }
        }
        catch {
            /* fall through */
        }
    }
    return '0.0.0.0';
}
async function main() {
    initTelemetry();
    if (!config.threadId && config.workerBindMode !== 'repo') {
        throw new Error('REMOTE_DEV_THREAD_ID or THREAD_ID is required unless WORKER_BIND_MODE=repo.');
    }
    if (!config.repoUrl) {
        throw new Error('DD_REPO_URL is required — the container must be pinned to one git repo.');
    }
    assertSafeGitBranchName(config.baseBranch, 'base branch');
    if (!config.serverAuthSecret) {
        fastify.log.warn('SERVER_AUTH_SECRET is not set — all non-healthz requests will 401');
    }
    if (!config.ghPat) {
        fastify.log.warn('GH_PAT is not set — /thread/open-pr and PR-requesting tasks will fail closed');
    }
    await ensureDeployKey();
    await mkdir(config.outputsDir, { recursive: true });
    // ---- Wire RxJS EventBus pipelines ----
    // 1. Vercel ingest pipeline — retries with exponential backoff.
    if (config.eventIngestUrl && config.eventIngestSecret) {
        eventBus.startVercelIngest(config.eventIngestUrl, config.eventIngestSecret);
        fastify.log.info('EventBus: Vercel ingest pipeline active');
    }
    // 2. Supabase broadcast pipeline — per-user fan-out with retry.
    if (isRealtimeEnabled()) {
        eventBus.startSupabaseBroadcast((userId, payload) => publishUserEvent(userId, payload));
        fastify.log.info('EventBus: Supabase broadcast pipeline active');
    }
    // 3. Log sink — tee all events to /tmp/convos/thread.log.
    eventBus.startLogSink(config.logDir);
    fastify.log.info(`EventBus: log sink active at ${config.logDir}/thread.log`);
    if (config.natsUrl) {
        fastify.log.info(`EventBus: NATS websocket fanout active on ${config.natsEventSubject}`);
    }
    if (config.workerFanoutWsUrl) {
        fastify.log.info('EventBus: outbound worker websocket fanout active');
    }
    if (config.threadId && config.idleTimeoutMs > 0) {
        eventBus.startIdleWatchdog(config.idleTimeoutMs, () => {
            fastify.log.info(`Idle timeout (${config.idleTimeoutMs / 1000}s) - shutting down`);
            process.kill(process.pid, 'SIGTERM');
        });
        fastify.log.info(`EventBus: idle watchdog active (${config.idleTimeoutMs / 1000}s) for thread ${config.threadId}`);
    }
    // Probe agent providers up front so the UI never picks one that fails
    // ENOENT mid-task. Fire-and-forget — we don't block boot on it; if it
    // hasn't finished by the time /agents is hit, the route falls back to
    // running the probe inline.
    void probeAllProviders().then((list) => {
        const installed = list
            .filter((p) => p.available)
            .map((p) => p.provider)
            .join(', ');
        const missing = list
            .filter((p) => !p.available)
            .map((p) => `${p.provider}(${p.reason ?? '?'})`)
            .join(', ');
        fastify.log.info(`agent providers — available: [${installed || 'none'}]` +
            (missing ? ` · unavailable: [${missing}]` : ''));
    });
    // Per-thread pods pre-warm their single session. Repo-scoped pool workers
    // create sessions lazily per task/thread.
    if (config.threadId) {
        const bootSession = getOrCreateSession({
            taskId: config.threadId,
            threadId: config.threadId,
            threadTitle: config.threadTitle ?? undefined,
        });
        await bootSession.ready;
    }
    registerWorkerWebSocketUpgrade();
    await fastify.listen({ host: config.host, port: config.port });
}
function shutdown(signal) {
    fastify.log.info(`${signal} received — tearing down EventBus + channels`);
    workerFanout.destroy();
    natsPublisher.destroy();
    eventBus.destroy();
    destroyChannelPool();
    fastify.close().then(() => shutdownTelemetry().finally(() => process.exit(0)), () => process.exit(1));
    setTimeout(() => process.exit(1), 10_000).unref();
}
process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));
// Pin runtime crashes to the request that caused them. wrapped-fetch
// already tags network errors via annotateError(); other throw sites
// fall back to whatever request context is still active on the async
// stack at crash time. Outside any request (e.g. setInterval heartbeat)
// requestContext is null and the log still captures the bare error.
function logCrash(kind, err) {
    const e = err instanceof Error ? err : new Error(String(err));
    const tagged = readErrorRequestContext(e) ?? snapshotRequestContext();
    fastify.log.error({
        err: e,
        crashKind: kind,
        requestContext: tagged,
    }, `${kind} pinned to request ${tagged?.requestId ?? '<none>'}`);
}
process.on('uncaughtException', (err) => {
    logCrash('uncaughtException', err);
});
process.on('unhandledRejection', (reason) => {
    logCrash('unhandledRejection', reason);
});
main().catch((err) => {
    logCrash('uncaughtException', err);
    workerFanout.destroy();
    natsPublisher.destroy();
    eventBus.destroy();
    shutdownTelemetry().finally(() => process.exit(1));
});
/* eslint-enable security/detect-non-literal-fs-filename */
//# sourceMappingURL=server.js.map