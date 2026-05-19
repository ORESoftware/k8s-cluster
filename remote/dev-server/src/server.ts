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
// appends tmp/convos/thread.log, and pushes the branch. PR creation is an
// explicit UI/API action.
//
// Worktrees + finished tasks are GC'd one hour after completion.

import Fastify from 'fastify';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import type { Dirent } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import type { IncomingMessage } from 'node:http';
import type { Socket } from 'node:net';
import { access, appendFile, mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { ReplaySubject, Subject, interval, type Subscription } from 'rxjs';
import { filter, takeUntil } from 'rxjs/operators';
import { z } from 'zod';

import { EventBus, type BusEvent } from './event-bus.js';
import {
  containerPoolConfigFromEnv,
  containerPoolConfigured,
  dispatchContainerPool,
  type ContainerPoolDispatchRequest,
  type ContainerPoolDispatchResponse,
} from './container-pool.js';

import {
  buildAgentEnvCandidates,
  getCachedAvailability,
  getRunner,
  probeAllProviders,
  resolveAgentProvider,
  type AgentEnvCandidate,
  type AgentProvider,
  type AgentRunnerEvent,
} from './agents/index.js';
import { publishArtifact } from './storage/index.js';
import type { PublishedArtifact, StorageProvider } from './storage/types.js';
import {
  acquireUserChannel,
  destroyChannelPool,
  isRealtimeEnabled,
  publishUserEvent,
  releaseUserChannel,
} from './realtime.js';
import { initTelemetry, shutdownTelemetry, withSpan } from './telemetry.js';
import { verifyDirectStreamToken } from './token.js';
import { NatsPublisher } from './nats-publisher.js';
import { WorkerFanoutWebSocket, workerFanoutWsUrlFromEnv } from './ws-fanout.js';

// ---------- Config ----------

const AGENT_FALLBACK_PROVIDER: AgentProvider = 'openai-sdk';
const AGENT_SECONDARY_FALLBACK_PROVIDER: AgentProvider = 'claude-sdk';
const CONFIG_AGENT_PROVIDERS = new Set<AgentProvider>([
  'claude-cli',
  'claude-sdk',
  'gemini-sdk',
  'openai-codex-cli',
  'openai-sdk',
]);
const GENERATED_GIT_EXCLUDES = [':!.pnpm-store', ':!node_modules', ':!.next', ':!.turbo'];

function configAgentProvider(value: string | undefined, fallback: AgentProvider): AgentProvider {
  return value && CONFIG_AGENT_PROVIDERS.has(value as AgentProvider) ? (value as AgentProvider) : fallback;
}

function configAgentProviderList(value: string | undefined, fallback: AgentProvider[]): AgentProvider[] {
  const requested = value
    ? value
        .split(/[,\s]+/)
        .map((item) => item.trim())
        .filter(Boolean)
    : fallback;
  const seen = new Set<AgentProvider>();
  const providers: AgentProvider[] = [];
  for (const item of requested) {
    if (CONFIG_AGENT_PROVIDERS.has(item as AgentProvider) && !seen.has(item as AgentProvider)) {
      seen.add(item as AgentProvider);
      providers.push(item as AgentProvider);
    }
  }
  return providers.length > 0 ? providers : fallback;
}

const configuredAgentFallbackProvider = configAgentProvider(
  process.env.AGENT_FALLBACK_PROVIDER,
  AGENT_FALLBACK_PROVIDER,
);
const configuredAgentSecondaryFallbackProvider = configAgentProvider(
  process.env.AGENT_SECONDARY_FALLBACK_PROVIDER,
  AGENT_SECONDARY_FALLBACK_PROVIDER,
);

const config = {
  port: Number(process.env.PORT ?? 8080),
  host: process.env.HOST ?? '0.0.0.0',
  workspaceRepo: process.env.WORKSPACE_REPO ?? '/home/node/workspace/repo',
  repoUrl: process.env.DD_REPO_URL ?? null,
  // Per-thread pods set REMOTE_DEV_THREAD_ID. Repo-scoped warm pool workers
  // leave it unset and accept tasks for any thread in the configured repo.
  threadId: process.env.REMOTE_DEV_THREAD_ID ?? process.env.THREAD_ID ?? null,
  workerBindMode:
    process.env.WORKER_BIND_MODE ?? (process.env.REMOTE_DEV_THREAD_ID || process.env.THREAD_ID ? 'thread' : 'repo'),
  userId: process.env.USER_ID ?? null,
  // Each agent run writes publishable files into ${OUTPUTS_DIR}/<taskId>/.
  // After claude exits, runTask scans that dir and uploads each file via
  // the configured storage adapter, emitting an `artifact` event per file.
  outputsDir: process.env.OUTPUTS_DIR ?? '/home/node/workspace/outputs',
  defaultStorageProvider: (process.env.DEFAULT_STORAGE_PROVIDER ?? 'local') as StorageProvider,
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
  threadContextBaseUrl:
    process.env.THREAD_CONTEXT_BASE_URL ??
    process.env.REMOTE_REST_API_BASE_URL ??
    'http://dd-remote-rest-api.default.svc.cluster.local:8082',
  threadContextLimit: Number(process.env.THREAD_CONTEXT_LIMIT ?? 20),
  threadContextMaxChars: Number(process.env.THREAD_CONTEXT_MAX_CHARS ?? 48_000),
  agentFallbackProvider: configuredAgentFallbackProvider,
  agentSecondaryFallbackProvider: configuredAgentSecondaryFallbackProvider,
  agentProviderRotation: configAgentProviderList(
    process.env.AGENT_PROVIDER_ROTATION,
    [configuredAgentFallbackProvider, configuredAgentSecondaryFallbackProvider, 'gemini-sdk'],
  ),
  agentBranchPrefix: process.env.AGENT_BRANCH_PREFIX ?? 'agent/k8s/openai-5.5',
  baseBranch: process.env.BASE_BRANCH ?? 'dev',
  threadLogRelativePath: process.env.THREAD_LOG_RELATIVE_PATH ?? 'tmp/convos/thread.log',
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

// ---------- Event types ----------

type ClaudeWrappedEvent = BusEvent['event'] & Extract<AgentRunnerEvent, { kind: 'claude' }>;

type WrappedEvent =
  | (BusEvent['event'] & { kind: 'status'; status: string; message?: string })
  | ClaudeWrappedEvent
  | (BusEvent['event'] & { kind: 'stderr'; text: string })
  | (BusEvent['event'] & { kind: 'error'; message: string })
  | (BusEvent['event'] & { kind: 'artifact'; artifact: PublishedArtifact })
  | (BusEvent['event'] & {
      kind: 'container_pool';
      pool: string;
      response: ContainerPoolDispatchResponse;
    })
  | (BusEvent['event'] & {
      kind: 'done';
      branch: string;
      prUrl?: string;
      exitReason: 'completed' | 'cancelled' | 'failed';
    })
  | (BusEvent['event'] & {
      kind: 'pr_open';
      branch: string;
      prUrl: string;
      draft: boolean;
    });

type StoredEvent = { seq: number; event: WrappedEvent };
type WebSocketJsonPrimitive = string | number | boolean | null;
type WebSocketJsonValue =
  | WebSocketJsonPrimitive
  | WebSocketJsonValue[]
  | { [key: string]: WebSocketJsonValue | undefined };
type WebSocketJsonObject = { [key: string]: WebSocketJsonValue | undefined };

type ThreadSession = {
  sessionId: string;
  userId?: string;
  workspacePath: string;
  branch: string;
  logPath: string;
  ready: Promise<void>;
  queue: Promise<void>;
  taskIds: Set<string>;
  createdAt: number;
  lastActiveAt: number;
};

type MergeUpstreamResult = {
  ok: true;
  threadId: string;
  branch: string;
  baseBranch: string;
  before: string;
  after: string;
  fastForward: boolean;
};

type MakeCommitResult = {
  ok: true;
  threadId: string;
  branch: string;
  before: string;
  after: string;
  committed: boolean;
  pushed: true;
  status: string;
};

type OpenPullRequestResult = {
  ok: true;
  threadId: string;
  branch: string;
  baseBranch: string;
  prUrl: string;
  title: string;
  draft: true;
  reused: boolean;
};

type TaskState = {
  taskId: string;
  prompt: string;
  /** dd-user UUID — used as the Supabase Realtime channel scope. */
  userId?: string;
  /** Optional Vercel-side thread id, for cross-referencing in events. */
  threadId?: string;
  /** Which runner is driving this task (Claude, Gemini, OpenAI, etc.). */
  provider: AgentProvider;
  containerPool?: { pool: string; request: ContainerPoolDispatchRequest };
  session: ThreadSession;
  child?: ChildProcess;
  /**
   * Per-task abort controller. Cancel triggers `abort()` so SDK runners
   * (which have no child process) can bail out via their own abort
   * mechanisms. CLI runners use it AND the child SIGTERM as belt+braces.
   */
  abortController: AbortController;
  events: StoredEvent[];
  event$: ReplaySubject<StoredEvent>;
  finished: boolean;
  cancelled: boolean;
  finishedAt?: number;
  worktreePath: string;
  branch: string;
  logPath: string;
};

type TaskReceipt = {
  taskId: string;
  threadId?: string;
  branch: string;
  provider?: AgentProvider;
  acceptedAt: string;
};

type ThreadContextTask = {
  id?: string;
  prompt?: string;
  status?: string;
  branch?: string | null;
  exitReason?: string | null;
  errorMessage?: string | null;
  latestEventKind?: string | null;
  latestPayload?: string | null;
  createdAt?: string | null;
  finishedAt?: string | null;
};

const tasks = new Map<string, TaskState>();
const sessions = new Map<string, ThreadSession>();

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

interface ShCaptureOptions {
  /** Hard timeout (ms) after which the child is SIGKILLed. */
  timeoutMs?: number;
  /** Extra env vars merged on top of process.env. */
  extraEnv?: Record<string, string>;
  /**
   * If set, replaces process.env entirely with this object for the child.
   * Use this to spawn `claude` without leaking GH_PAT / SUPABASE_* etc.
   */
  isolatedEnv?: Record<string, string>;
}

function shCapture(
  cmd: string,
  args: string[],
  cwd: string,
  optsOrExtraEnv?: ShCaptureOptions | Record<string, string>,
): Promise<string> {
  // Backwards-compat: callers passing a plain extraEnv object still work.
  const opts: ShCaptureOptions =
    optsOrExtraEnv &&
    typeof optsOrExtraEnv === 'object' &&
    ('timeoutMs' in optsOrExtraEnv ||
      'extraEnv' in optsOrExtraEnv ||
      'isolatedEnv' in optsOrExtraEnv)
      ? (optsOrExtraEnv as ShCaptureOptions)
      : { extraEnv: optsOrExtraEnv as Record<string, string> | undefined };

  const env = opts.isolatedEnv
    ? opts.isolatedEnv
    : { ...(process.env as Record<string, string>), ...(opts.extraEnv ?? {}) };

  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd, env });
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    let killTimer: ReturnType<typeof setTimeout> | null = null;
    if (opts.timeoutMs && opts.timeoutMs > 0) {
      killTimer = setTimeout(() => {
        timedOut = true;
        try {
          child.kill('SIGKILL');
        } catch {
          /* ignore */
        }
      }, opts.timeoutMs);
    }
    child.stdout.on('data', (d: Buffer) => {
      stdout += d.toString('utf8');
    });
    child.stderr.on('data', (d: Buffer) => {
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
      } else {
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

function getSessionId(threadId: string | undefined, taskId: string): string {
  return threadId ?? config.threadId ?? taskId;
}

function getSessionWorkspacePath(_sessionId: string): string {
  // The container is pinned to one thread; every task on this thread
  // shares the same workspace.
  return config.workspaceRepo;
}

function slugifyBranchFragment(value: string): string {
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

function repoDisplayName(): string {
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

function gitBranchTarget(branch: string): string {
  return `${repoDisplayName()} branch ${branch}`;
}

function providerCanEditWorkspace(provider: AgentProvider): boolean {
  return provider !== 'gemini-sdk';
}

function providerCanAccessWorkspace(provider: AgentProvider): boolean {
  return providerCanEditWorkspace(provider);
}

function stripNegatedWorkspaceChangePhrases(prompt: string): string {
  return prompt
    .replace(
      /\b(?:do\s+not|don't|dont|never)\s+(?:make\s+)?(?:any\s+)?(?:file|code|workspace|repo(?:sitory)?|source)?\s*(?:changes?|edits?|modifications?)\b/gi,
      ' ',
    )
    .replace(
      /\b(?:do\s+not|don't|dont|never)\s+(?:edit|change|modify|write|update|create|delete|remove|patch|fix|implement)(?:\s+(?:the|a|any|files?|code|workspace|repo(?:sitory)?|source|project|readme(?:\.md)?|package(?:\.json)?)){0,4}\b/gi,
      ' ',
    )
    .replace(
      /\b(?:without|no)\s+(?:making\s+)?(?:any\s+)?(?:file|code|workspace|repo(?:sitory)?|source)?\s*(?:changes?|edits?|modifications?)\b/gi,
      ' ',
    );
}

function promptLikelyRequiresWorkspaceChange(prompt: string): boolean {
  const editablePrompt = stripNegatedWorkspaceChangePhrases(prompt);
  return /\b(add|change|create|delete|edit|fix|implement|modify|move|patch|refactor|remove|rename|replace|update|write)\b/i.test(
    editablePrompt,
  );
}

function promptLikelyRequiresWorkspaceAccess(prompt: string): boolean {
  const workspacePrompt = stripNegatedWorkspaceChangePhrases(prompt);
  if (promptLikelyRequiresWorkspaceChange(prompt)) {
    return true;
  }
  if (/\b(readme(?:\.md)?|package(?:\.json)?|pnpm-lock|dockerfile|makefile|tsconfig|cargo\.toml|go\.mod)\b/i.test(workspacePrompt)) {
    return true;
  }
  const hasWorkspaceNoun =
    /\b(repo(?:sitory)?|codebase|workspace|working tree|source tree|folders?|directories|dirs?|files?|top[- ]level|root)\b/i.test(
      workspacePrompt,
    );
  const hasInspectionVerb =
    /\b(count|find|grep|how many|inspect|list|look|open|read|search|show|tree|what|where|which)\b/i.test(
      workspacePrompt,
    );
  return hasWorkspaceNoun && hasInspectionVerb;
}

async function gitWorkspaceStatus(workspacePath: string): Promise<string> {
  return shCapture(
    'git',
    ['status', '--porcelain', '--untracked-files=all', '--', '.', ...GENERATED_GIT_EXCLUDES],
    workspacePath,
    { timeoutMs: TIMEOUT_GIT_QUICK },
  );
}

async function gitAddWorkspaceChanges(workspacePath: string): Promise<void> {
  await shCapture('git', ['add', '-A', '--', '.', ...GENERATED_GIT_EXCLUDES], workspacePath, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
}

async function resetDependencyInstallArtifacts(workspacePath: string): Promise<void> {
  await shCapture('git', ['restore', '--staged', '--worktree', '--', '.'], workspacePath, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
  await shCapture(
    'git',
    ['clean', '-fdx', '--exclude=node_modules', '--exclude=.pnpm-store', '--exclude=.next', '--exclude=.turbo'],
    workspacePath,
    { timeoutMs: TIMEOUT_GIT_QUICK },
  );
}

function getSessionBranch(
  sessionId: string,
  branchHint?: string | null,
  titleHint?: string | null,
  promptHint?: string | null,
): string {
  const hinted = branchHint?.trim();
  if (hinted) {
    return hinted;
  }
  const titleSlug = slugifyBranchFragment(titleHint?.trim() || promptHint?.trim() || sessionId);
  return `${config.agentBranchPrefix}/${sessionId}/${titleSlug}`;
}

function isPlaceholderSessionBranch(sessionId: string, branch: string): boolean {
  return branch === getSessionBranch(sessionId);
}

function getSessionLogPath(workspacePath: string): string {
  return join(workspacePath, config.threadLogRelativePath);
}

async function remoteBranchExists(branch: string): Promise<boolean> {
  const out = await shCapture(
    'git',
    ['ls-remote', '--heads', 'origin', branch],
    config.workspaceRepo,
    { timeoutMs: TIMEOUT_GIT_NETWORK },
  );
  return out.trim().length > 0;
}

async function installWorkspaceDependencies(workspacePath: string): Promise<{
  ok: boolean;
  error?: string;
}> {
  try {
    await access(join(workspacePath, 'package.json'));
  } catch {
    return { ok: true };
  }
  try {
    await shCapture('pnpm', ['install', '--frozen-lockfile'], workspacePath, {
      timeoutMs: TIMEOUT_PNPM_INSTALL,
    });
    return { ok: true };
  } catch (err) {
    const frozenMessage = err instanceof Error ? err.message : String(err);
    process.stderr.write(`[remote-dev] frozen pnpm install failed: ${frozenMessage}\n`);
    try {
      await shCapture('pnpm', ['install', '--no-frozen-lockfile'], workspacePath, {
        timeoutMs: TIMEOUT_PNPM_INSTALL,
      });
      return { ok: true, error: frozenMessage };
    } catch (fallbackErr) {
      const fallbackMessage =
        fallbackErr instanceof Error ? fallbackErr.message : String(fallbackErr);
      process.stderr.write(`[remote-dev] fallback pnpm install failed: ${fallbackMessage}\n`);
      return { ok: false, error: `${frozenMessage}; fallback: ${fallbackMessage}` };
    }
  }
}

async function prepareSessionWorkspace(session: ThreadSession): Promise<void> {
  if (config.threadId && session.sessionId !== config.threadId) {
    throw new Error(`container is pinned to thread ${config.threadId}, got ${session.sessionId}`);
  }

  if (config.skipBootGitSync) {
    await configureGitIdentity(session.workspacePath);
    await mkdir(dirname(session.logPath), { recursive: true });
    await appendFile(
      session.logPath,
      JSON.stringify({
        ts: new Date().toISOString(),
        kind: 'session-ready',
        sessionId: session.sessionId,
        branch: session.branch,
        workspacePath: session.workspacePath,
        repo: config.repoUrl,
        baseBranch: config.baseBranch,
        skippedBootGitSync: true,
      }) + '\n',
    );
    return;
  }

  await shCapture('git', ['fetch', '--quiet', 'origin', config.baseBranch], config.workspaceRepo, {
    timeoutMs: TIMEOUT_GIT_NETWORK,
  });

  const hasRemoteBranch = await remoteBranchExists(session.branch);
  let switchSource = `origin/${config.baseBranch}`;
  if (hasRemoteBranch) {
    await shCapture('git', ['fetch', '--quiet', 'origin', session.branch], config.workspaceRepo, {
      timeoutMs: TIMEOUT_GIT_NETWORK,
    });
    switchSource = 'FETCH_HEAD';
  }

  await shCapture(
    'git',
    [
      'switch',
      '--discard-changes',
      '-C',
      session.branch,
      switchSource,
    ],
    config.workspaceRepo,
    { timeoutMs: TIMEOUT_GIT_QUICK },
  );

  const installResult = await installWorkspaceDependencies(session.workspacePath);
  await resetDependencyInstallArtifacts(session.workspacePath);

  await configureGitIdentity(session.workspacePath);
  await mkdir(dirname(session.logPath), { recursive: true });
  await appendFile(
    session.logPath,
    JSON.stringify({
      ts: new Date().toISOString(),
      kind: 'session-ready',
      sessionId: session.sessionId,
      branch: session.branch,
      workspacePath: session.workspacePath,
      repo: config.repoUrl,
      baseBranch: config.baseBranch,
      dependencyInstallOk: installResult.ok,
      dependencyInstallError: installResult.error,
    }) + '\n',
  );
}

function getOrCreateSession(input: {
  taskId: string;
  threadId?: string;
  userId?: string;
  branch?: string;
  threadTitle?: string;
  prompt?: string;
}): ThreadSession {
  const sessionId = getSessionId(input.threadId, input.taskId);
  const desiredBranch = getSessionBranch(sessionId, input.branch, input.threadTitle, input.prompt);
  const existing = sessions.get(sessionId);
  if (existing) {
    existing.lastActiveAt = Date.now();
    if (!existing.userId && input.userId) {
      existing.userId = input.userId;
    }
    if (
      existing.taskIds.size === 0 &&
      existing.branch !== desiredBranch &&
      !input.branch?.trim() &&
      isPlaceholderSessionBranch(sessionId, existing.branch)
    ) {
      existing.branch = desiredBranch;
      existing.ready = existing.ready.catch(() => undefined).then(() => prepareSessionWorkspace(existing));
    }
    return existing;
  }

  const workspacePath = getSessionWorkspacePath(sessionId);
  const session: ThreadSession = {
    sessionId,
    userId: input.userId,
    workspacePath,
    branch: desiredBranch,
    logPath: getSessionLogPath(workspacePath),
    ready: Promise.resolve(),
    queue: Promise.resolve(),
    taskIds: new Set(),
    createdAt: Date.now(),
    lastActiveAt: Date.now(),
  };
  session.ready = prepareSessionWorkspace(session);
  sessions.set(sessionId, session);
  return session;
}

async function appendThreadLog(state: TaskState, payload: Record<string, unknown>): Promise<void> {
  try {
    await access(state.worktreePath);
    await mkdir(dirname(state.logPath), { recursive: true });
    await appendFile(
      state.logPath,
      JSON.stringify({
        ts: new Date().toISOString(),
        taskId: state.taskId,
        threadId: state.threadId,
        provider: state.provider,
        ...payload,
      }) + '\n',
    );
  } catch (err) {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'ENOENT') {
      return;
    }
    process.stderr.write(
      `[remote-dev thread-log] append failed: ${
        err instanceof Error ? err.message : String(err)
      }\n`,
    );
  }
}

function taskReceiptPath(taskId: string): string {
  return join(config.processedTasksDir, `${basename(taskId)}.json`);
}

async function readTaskReceipt(taskId: string): Promise<TaskReceipt | undefined> {
  try {
    return JSON.parse(await readFile(taskReceiptPath(taskId), 'utf8')) as TaskReceipt;
  } catch {
    return undefined;
  }
}

async function writeTaskReceipt(receipt: TaskReceipt): Promise<void> {
  await mkdir(config.processedTasksDir, { recursive: true });
  await writeFile(taskReceiptPath(receipt.taskId), `${JSON.stringify(receipt, null, 2)}\n`);
}

function sanitizeEventText(value: string): string {
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
    .replace(/\b(?:ghp|github_pat)_[A-Za-z0-9_*.-]{8,}\b/g, '[redacted-github-token]');
}

function compactAgentErrorMessage(provider: AgentProvider, err: unknown): string {
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

function formatAgentFailureSummary(
  provider: AgentProvider,
  failures: Map<string, number>,
  attempted: number,
  total: number,
): string {
  const reasons = Array.from(failures.entries())
    .sort((a, b) => b[1] - a[1])
    .map(([reason, count]) => `${reason} (${count})`)
    .slice(0, 4)
    .join('; ');
  return `${provider} failed ${attempted}/${total} configured key(s): ${reasons || 'unknown error'}`;
}

function isWebSocketJsonObject(value: WebSocketJsonValue): value is WebSocketJsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function sanitizeEventValue(value: unknown): unknown {
  if (typeof value === 'string') {
    return sanitizeEventText(value);
  }
  if (Array.isArray(value)) {
    return value.map((item) => sanitizeEventValue(item));
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, item]) => [
        key,
        sanitizeEventValue(item),
      ]),
    );
  }
  return value;
}

function sanitizeEvent(event: WrappedEvent): WrappedEvent {
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

function recordValue(value: unknown): Record<string, unknown> | undefined {
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return undefined;
}

function nestedRecord(value: unknown, key: string): Record<string, unknown> | undefined {
  return recordValue(recordValue(value)?.[key]);
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function agentRawType(raw: unknown): string {
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
    .filter((value): value is string => typeof value === 'string' && value.length > 0)
    .join(' ');
}

function agentEventVisibleText(raw: unknown): string {
  const obj = recordValue(raw);
  if (!obj) return '';
  const directText = stringValue(obj.text).trim();
  if (directText) return directText;
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

function agentEventHasProviderError(raw: unknown): boolean {
  const obj = recordValue(raw);
  const message = nestedRecord(raw, 'message');
  return Boolean(
    obj?.error ||
      obj?.is_error === true ||
      message?.error ||
      /billing_error|api_error|permission_denied|quota|rate limit/i.test(
        [obj?.error, obj?.result, obj?.terminal_reason].filter(Boolean).join(' '),
      ),
  );
}

function shouldForwardAgentRunnerEvent(event: AgentRunnerEvent): boolean {
  if (event.kind !== 'claude') return true;
  const rawType = agentRawType(event.raw);
  const visibleText = agentEventVisibleText(event.raw);
  if (agentEventHasProviderError(event.raw)) return false;
  if (
    /raw_model_stream_event|response\.created|response\.in_progress|response_started|response\.completed|system|tool/i.test(
      rawType,
    ) &&
    !visibleText
  ) {
    return false;
  }
  return true;
}

function emit(state: TaskState, event: WrappedEvent): void {
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
  const stored: StoredEvent = { seq: state.events.length, event };
  state.events.push(stored);
  state.event$.next(stored);
  incCounter('dd_runtime_events_total', {
    service: 'dd-dev-server-api',
    kind: event.kind,
    provider: state.provider,
  });
  void appendThreadLog(state, { kind: 'event', seq: stored.seq, event });
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
    branch: state.branch,
    seq: stored.seq,
    emittedAt: new Date().toISOString(),
    event,
  };
  natsPublisher.publish(config.natsEventSubject, fanoutPayload);
  workerFanout.publish(fanoutPayload);
}

async function ensureDeployKey(): Promise<void> {
  if (!config.ghDeployKey) {
    return;
  }
  await mkdir(dirname(config.ghDeployKeyPath), { recursive: true });
  try {
    await access(config.ghDeployKeyPath);
    return; // already on disk
  } catch {
    /* missing — write it */
  }
  await writeFile(config.ghDeployKeyPath, config.ghDeployKey, { mode: 0o600 });
}

async function configureGitIdentity(cwd: string): Promise<void> {
  await shCapture('git', ['config', 'user.name', config.prAuthor.name], cwd, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
  await shCapture('git', ['config', 'user.email', config.prAuthor.email], cwd, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
}

async function waitForBootGitReady(): Promise<void> {
  // If the container started via entrypoint.sh, the git fetch + switch
  // runs as a background process. Wait for it to finish before we proceed.
  const gitReadyPid = process.env.GIT_READY_PID;
  if (!gitReadyPid) {
    return;
  }

  try {
    // waitpid via polling — Node doesn't expose waitpid() natively.
    // Once the PID is gone from /proc, the background git is done.
    await new Promise<void>((resolve) => {
      const check = (): void => {
        try {
          process.kill(Number(gitReadyPid), 0); // signal 0 = existence check
          setTimeout(check, 500);
        } catch {
          resolve();
        }
      };
      check();
    });
    delete process.env.GIT_READY_PID;
  } catch {
    /* if the PID was already gone, that's fine */
  }
}

async function mergeUpstreamForThread(input: {
  threadId?: string;
  userId?: string;
  branch?: string;
  threadTitle?: string;
}): Promise<MergeUpstreamResult> {
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
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'merge-upstream-start',
          sessionId: session.sessionId,
          branch: session.branch,
          baseBranch: config.baseBranch,
        }) + '\n',
      );
      const before = (
        await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
          timeoutMs: TIMEOUT_GIT_QUICK,
        })
      ).trim();
      await shCapture(
        'git',
        ['fetch', '--quiet', 'origin', config.baseBranch],
        session.workspacePath,
        { timeoutMs: TIMEOUT_GIT_NETWORK },
      );
      await shCapture(
        'git',
        ['merge', '--no-edit', `origin/${config.baseBranch}`],
        session.workspacePath,
        { timeoutMs: TIMEOUT_GIT_QUICK },
      );
      const after = (
        await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
          timeoutMs: TIMEOUT_GIT_QUICK,
        })
      ).trim();
      await shCapture(
        'git',
        ['push', '--no-verify', '--set-upstream', 'origin', session.branch],
        session.workspacePath,
        { timeoutMs: TIMEOUT_GIT_NETWORK },
      );
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'merge-upstream-done',
          sessionId: session.sessionId,
          branch: session.branch,
          baseBranch: config.baseBranch,
          before,
          after,
        }) + '\n',
      );
      return {
        ok: true as const,
        threadId,
        branch: session.branch,
        baseBranch: config.baseBranch,
        before,
        after,
        fastForward: before !== after,
      };
    });

  session.queue = queuedMerge.then(
    () => undefined,
    () => undefined,
  );
  return queuedMerge;
}

function manualCommitMessage(input: { threadId: string; reason?: string; threadTitle?: string }): string {
  const message = (input.reason ?? input.threadTitle ?? '').trim().replace(/\s+/g, ' ');
  if (message) {
    return message.slice(0, 200);
  }
  return `agent(${input.threadId}): manual commit`;
}

async function makeCommitForThread(input: {
  taskId?: string;
  threadId?: string;
  userId?: string;
  branch?: string;
  threadTitle?: string;
  reason?: string;
}): Promise<MakeCommitResult> {
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
      const before = (
        await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
          timeoutMs: TIMEOUT_GIT_QUICK,
        })
      ).trim();
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'make-commit-start',
          sessionId: session.sessionId,
          branch: session.branch,
          taskId,
        }) + '\n',
      );
      const status = await gitWorkspaceStatus(session.workspacePath);
      const hasChanges = status.trim().length > 0;
      if (hasChanges) {
        await gitAddWorkspaceChanges(session.workspacePath);
        await shCapture(
          'git',
          ['commit', '--no-verify', '-m', manualCommitMessage({ ...input, threadId })],
          session.workspacePath,
          { timeoutMs: TIMEOUT_GIT_QUICK },
        );
      }
      const after = (
        await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
          timeoutMs: TIMEOUT_GIT_QUICK,
        })
      ).trim();
      await shCapture(
        'git',
        ['push', '--no-verify', '--set-upstream', 'origin', session.branch],
        session.workspacePath,
        { timeoutMs: TIMEOUT_GIT_NETWORK },
      );
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'make-commit-done',
          sessionId: session.sessionId,
          branch: session.branch,
          taskId,
          before,
          after,
          committed: hasChanges,
        }) + '\n',
      );
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
        ok: true as const,
        threadId,
        branch: session.branch,
        before,
        after,
        committed: hasChanges,
        pushed: true as const,
        status: hasChanges ? 'committed-and-pushed' : 'pushed-without-new-commit',
      };
    });

  session.queue = queuedCommit.then(
    () => undefined,
    () => undefined,
  );
  return queuedCommit;
}

async function openPullRequestForThread(input: {
  taskId?: string;
  threadId?: string;
  userId?: string;
  branch?: string;
  threadTitle?: string;
  reason?: string;
}): Promise<OpenPullRequestResult> {
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
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'open-pr-start',
          sessionId: session.sessionId,
          branch: session.branch,
          baseBranch: config.baseBranch,
          taskId,
        }) + '\n',
      );
      const result = await ensurePullRequestForSession({
        session,
        taskId,
        threadTitle: input.threadTitle ?? input.reason,
      });
      await appendFile(
        session.logPath,
        JSON.stringify({
          ts: new Date().toISOString(),
          kind: 'open-pr-done',
          sessionId: session.sessionId,
          branch: session.branch,
          baseBranch: config.baseBranch,
          taskId,
          prUrl: result.prUrl,
          draft: result.draft,
          reused: result.reused,
        }) + '\n',
      );
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

  session.queue = queuedOpen.then(
    () => undefined,
    () => undefined,
  );
  return queuedOpen;
}

function truncateContext(value: string, maxChars: number): string {
  if (value.length <= maxChars) {
    return value;
  }
  return value.slice(value.length - maxChars);
}

function formatThreadContextTasks(
  tasksFromContext: ThreadContextTask[],
  currentTaskId: string,
): string {
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

async function readLocalThreadContext(state: TaskState): Promise<string> {
  try {
    const text = await readFile(state.logPath, 'utf8');
    return truncateContext(text, Math.min(config.threadContextMaxChars, 24_000));
  } catch {
    return '';
  }
}

async function buildPromptWithThreadContext(state: TaskState): Promise<string> {
  if (!state.threadId) {
    return state.prompt;
  }

  const base = config.threadContextBaseUrl?.replace(/\/+$/, '');
  let contextText = '';
  let contextSource = 'none';
  if (base) {
    try {
      const response = await fetch(
        `${base}/api/agents/threads/${encodeURIComponent(state.threadId)}/context?limit=${config.threadContextLimit}`,
        { signal: AbortSignal.timeout(10_000) },
      );
      if (response.ok) {
        const body = (await response.json()) as { source?: string; tasks?: ThreadContextTask[] };
        contextText = formatThreadContextTasks(body.tasks ?? [], state.taskId);
        contextSource = body.source ?? 'rest-api';
      }
    } catch (err) {
      emit(state, {
        kind: 'stderr',
        text: `thread context lookup failed: ${err instanceof Error ? err.message : String(err)}`,
      });
    }
  }

  if (!contextText) {
    contextText = await readLocalThreadContext(state);
    contextSource = contextText ? 'local-thread-log' : 'none';
  }

  if (!contextText) {
    return state.prompt;
  }

  const cappedContext = truncateContext(contextText, config.threadContextMaxChars);
  emit(state, { kind: 'status', status: `thread-context:${contextSource}` });
  return [
    `You are continuing remote development thread ${state.threadId}.`,
    'Use the previous thread context below when deciding what to do next.',
    'Do not repeat completed work unless the current user prompt asks you to.',
    '',
    '<previous_thread_context>',
    cappedContext,
    '</previous_thread_context>',
    '',
    '<current_user_prompt>',
    state.prompt,
    '</current_user_prompt>',
  ].join('\n');
}

// ---------- Per-task workflow ----------

async function runTask(state: TaskState): Promise<void> {
  return withSpan(
    'remote-dev.run-task',
    {
      'dd.remote.task_id': state.taskId,
      'dd.remote.thread_id': state.threadId,
      'dd.remote.provider': state.provider,
      'dd.remote.branch': state.branch,
    },
    async (span) => {
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

      // Per-task outputs dir — the agent writes publishable files here.
      // After claude exits we scan it and upload each file via the storage
      // adapter, emitting an `artifact` event per file.
      const taskOutputsDir = join(config.outputsDir, state.taskId);
      await mkdir(taskOutputsDir, { recursive: true });
      await appendThreadLog(state, {
        kind: 'prompt',
        prompt: state.prompt,
        workspacePath: state.worktreePath,
        branch: state.branch,
      });

      if (state.containerPool) {
        emit(state, {
          kind: 'status',
          status: `container-pool-dispatch:${state.containerPool.pool}`,
        });
        const response = await dispatchContainerPool(
          config.containerPool,
          state.containerPool.pool,
          {
            ...state.containerPool.request,
            requestId: state.containerPool.request.requestId ?? state.taskId,
            poolSlug: state.containerPool.request.poolSlug ?? state.containerPool.pool,
          },
        );
        await appendThreadLog(state, {
          kind: 'container-pool-result',
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
      // Strict env allowlist owned by the runner module. Inheriting the full
      // process.env into the agent process would leak our GitHub deploy key,
      // Supabase service role key, ingest secret, etc. via any `env` or
      // `printenv` tool call. The runner adds only the API key its model
      // needs.
      const prompt = await buildPromptWithThreadContext(state);
      const providerOrder = [...config.agentProviderRotation, state.provider].filter(
        (provider, index, values) => values.indexOf(provider) === index,
      );
      const attemptGroups: { provider: AgentProvider; candidates: AgentEnvCandidate[] }[] = [];
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
        throw new Error(
          `no configured agent API keys for ${repoDisplayName()}; set OPENAI_API_KEYS_JSON, ANTHROPIC_API_KEYS_JSON, or GEMINI_API_KEYS_JSON`,
        );
      }

      const runAgentAttempt = async (attempt: AgentEnvCandidate): Promise<void> => {
        const runner = getRunner(attempt.provider);
        await runner.run({
          prompt,
          cwd: state.worktreePath,
          env: attempt.env,
          signal: state.abortController.signal,
          timeoutMs: config.agentRunTimeoutMs,
          emit: (ev: AgentRunnerEvent) => {
            if (shouldForwardAgentRunnerEvent(ev)) {
              emit(state, ev);
            }
          },
          setChild: (child: ChildProcess) => {
            state.child = child;
          },
        });
      };

      let lastErr: unknown = null;
      let completedAgentRun = false;
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
        emit(state, {
          kind: 'status',
          status: `agent-running:${group.provider}`,
          message:
            `Workspace: ${gitBranchTarget(state.branch)}\n` +
            `Base branch: ${config.baseBranch}\n` +
            `Credentials: ${group.candidates.length} configured key(s)`,
        });
        const failures = new Map<string, number>();
        let attempted = 0;
        for (const attempt of group.candidates) {
          if (state.cancelled || state.abortController.signal.aborted) {
            throw lastErr ?? new Error('agent run cancelled');
          }
          attempted += 1;
          try {
            await runAgentAttempt(attempt);
            completedAgentRun = true;
            lastErr = null;
            break;
          } catch (err) {
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
            message: formatAgentFailureSummary(
              group.provider,
              failures,
              attempted,
              group.candidates.length,
            ),
          });
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
        throw new Error(
          `agent completed without workspace changes for a repo-edit prompt in ${repoDisplayName()}`,
        );
      }
      emit(state, {
        kind: 'status',
        status: `pushing to ${gitBranchTarget(state.branch)}`,
        message: `Base branch: ${config.baseBranch}`,
      });
      if (status.trim()) {
        await gitAddWorkspaceChanges(state.worktreePath);
        await shCapture(
          'git',
          ['commit', '--no-verify', '-m', `agent(${state.session.sessionId}): ${state.taskId}`],
          state.worktreePath,
          { timeoutMs: TIMEOUT_GIT_QUICK },
        );
      }
      await shCapture(
        'git',
        ['push', '--no-verify', '--set-upstream', 'origin', state.branch],
        state.worktreePath,
        { timeoutMs: TIMEOUT_GIT_NETWORK },
      );
      emit(state, {
        kind: 'status',
        status: `pushed to ${gitBranchTarget(state.branch)}`,
        message: status.trim()
          ? `Committed ${status.trim().split('\n').length} changed path(s).`
          : 'No workspace changes were committed; branch push verified.',
      });

      // Publish any files the agent dropped in the per-task outputs dir.
      // Failures uploading individual files are surfaced as `error` events
      // but do not fail the whole task.
      await publishOutputs(state, taskOutputsDir);

      emit(state, {
        kind: 'status',
        status: `completed task on ${gitBranchTarget(state.branch)}`,
        message: 'No PR was opened automatically; use Open draft PR to create one against the base branch.',
      });
      emit(state, {
        kind: 'done',
        branch: state.branch,
        exitReason: 'completed',
      });
    },
  );
}

async function ensurePullRequestForSession(input: {
  session: ThreadSession;
  taskId?: string;
  threadTitle?: string;
  prompt?: string;
}): Promise<OpenPullRequestResult> {
  const ghEnv = config.ghPat ? { GH_TOKEN: config.ghPat } : undefined;
  try {
    const existing = await shCapture(
      'gh',
      [
        'pr',
        'view',
        input.session.branch,
        '--json',
        'url,isDraft,title',
        '--jq',
        '[.url, (.isDraft | tostring), .title] | @tsv',
      ],
      input.session.workspacePath,
      { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv },
    );
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
  } catch {
    /* no existing PR */
  }

  await shCapture('git', ['fetch', 'origin', config.baseBranch], input.session.workspacePath, {
    timeoutMs: TIMEOUT_GIT_NETWORK,
  });
  const [behindCountText = '0', aheadCountText = '0'] = (
    await shCapture(
      'git',
      ['rev-list', '--left-right', '--count', `origin/${config.baseBranch}...HEAD`],
      input.session.workspacePath,
      { timeoutMs: TIMEOUT_GIT_QUICK },
    )
  )
    .trim()
    .split(/\s+/);
  const behindCount = Number.parseInt(behindCountText, 10) || 0;
  const aheadCount = Number.parseInt(aheadCountText, 10) || 0;
  if (aheadCount === 0) {
    const before = (
      await shCapture('git', ['rev-parse', 'HEAD'], input.session.workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
      })
    ).trim();
    if (behindCount > 0) {
      await shCapture(
        'git',
        ['merge', '--ff-only', `origin/${config.baseBranch}`],
        input.session.workspacePath,
        { timeoutMs: TIMEOUT_GIT_NETWORK },
      );
    }
    await shCapture(
      'git',
      [
        'commit',
        '--allow-empty',
        '--no-verify',
        '-m',
        `agent(${input.session.sessionId}): open draft PR`,
      ],
      input.session.workspacePath,
      { timeoutMs: TIMEOUT_GIT_QUICK },
    );
    const after = (
      await shCapture('git', ['rev-parse', 'HEAD'], input.session.workspacePath, {
        timeoutMs: TIMEOUT_GIT_QUICK,
      })
    ).trim();
    await shCapture(
      'git',
      ['push', '--no-verify', '--set-upstream', 'origin', input.session.branch],
      input.session.workspacePath,
      { timeoutMs: TIMEOUT_GIT_NETWORK },
    );
    await appendFile(
      input.session.logPath,
      JSON.stringify({
        ts: new Date().toISOString(),
        kind: 'open-pr-marker-commit',
        sessionId: input.session.sessionId,
        branch: input.session.branch,
        baseBranch: config.baseBranch,
        before,
        after,
        fastForwardedBase: behindCount > 0,
      }) + '\n',
    );
  }

  const commitTitle = (
    await shCapture('git', ['log', '-1', '--pretty=%s'], input.session.workspacePath, {
      timeoutMs: TIMEOUT_GIT_QUICK,
    })
  )
    .trim()
    .replace(/\s+/g, ' ');
  const rawTitle =
    input.threadTitle?.trim() || commitTitle || input.prompt?.trim() || input.session.sessionId;
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

  const out = await shCapture(
    'gh',
    [
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
    ],
    input.session.workspacePath,
    { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv },
  );
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
async function publishOutputs(state: TaskState, taskOutputsDir: string): Promise<void> {
  let dirents: Dirent[];
  try {
    dirents = await readdir(taskOutputsDir, {
      withFileTypes: true,
    });
  } catch {
    return; // dir absent / unreadable → nothing to publish, that's fine
  }

  if (dirents.length === 0) {
    return;
  }

  emit(state, { kind: 'status', status: 'publishing-artifacts' });

  // Recurse one level so flat-or-nested layouts both work.
  const filesToPublish: string[] = [];
  for (const e of dirents) {
    if (e.isFile()) {
      filesToPublish.push(join(taskOutputsDir, e.name));
    } else if (e.isDirectory()) {
      try {
        const sub = await readdir(join(taskOutputsDir, e.name), {
          withFileTypes: true,
        });
        for (const s of sub) {
          if (s.isFile()) {
            filesToPublish.push(join(taskOutputsDir, e.name, s.name));
          }
        }
      } catch {
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
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      emit(state, {
        kind: 'error',
        message: `failed to publish ${basename(filePath)}: ${message}`,
      });
    }
  }
}

// ---------- HTTP server ----------

const fastify = Fastify({ logger: true });

type MetricLabels = Record<string, string | number | boolean | undefined>;

const counters = new Map<string, { labels: MetricLabels; value: number }>();
const activeWorkerSockets = new Set<WorkerWebSocketClient>();
const activeTerminalSockets = new Set<TerminalWebSocketClient>();

function metricKey(name: string, labels: MetricLabels): string {
  return `${name}:${Object.entries(labels)
    .filter((entry) => entry[1] !== undefined)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, value]) => `${key}=${value}`)
    .join(',')}`;
}

function incCounter(name: string, labels: MetricLabels = {}, amount = 1): void {
  const key = metricKey(name, labels);
  const current = counters.get(key) ?? { labels, value: 0 };
  current.value += amount;
  counters.set(key, current);
}

function renderLabels(labels: MetricLabels): string {
  const entries = Object.entries(labels).filter((entry) => entry[1] !== undefined);
  if (entries.length === 0) {
    return '';
  }
  return `{${entries
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, value]) => `${key}="${String(value).replace(/"/g, '\\"')}"`)
    .join(',')}}`;
}

function renderCounter(name: string, help: string): string[] {
  const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} counter`];
  for (const entry of counters.entries()) {
    if (!entry[0].startsWith(`${name}:`)) {
      continue;
    }
    lines.push(`${name}${renderLabels(entry[1].labels)} ${entry[1].value}`);
  }
  return lines;
}

function renderGauge(name: string, help: string, value: number): string[] {
  return [`# HELP ${name} ${help}`, `# TYPE ${name} gauge`, `${name} ${value}`];
}

function renderMetrics(): string {
  const now = Date.now();
  const startedAtMs = Date.parse(serverStartedAt);
  const lines = [
    ...renderCounter(
      'dd_runtime_http_requests_total',
      'HTTP requests observed by the dd remote runtime.',
    ),
    ...renderCounter(
      'dd_runtime_events_total',
      'Task stream events emitted by the dd remote runtime.',
    ),
    ...renderCounter(
      'dd_runtime_tasks_total',
      'Task dispatches accepted by the dd remote runtime.',
    ),
    ...renderGauge(
      'dd_runtime_inflight_tasks',
      'Tasks that are currently not finished.',
      Array.from(tasks.values()).filter((t) => !t.finished).length,
    ),
    ...renderGauge(
      'dd_runtime_tracked_tasks',
      'Tasks retained in memory for stream replay or GC.',
      tasks.size,
    ),
    ...renderGauge(
      'dd_runtime_sessions',
      'Thread sessions retained in this worker.',
      sessions.size,
    ),
    ...renderGauge(
      'dd_runtime_uptime_seconds',
      'Worker process uptime in seconds.',
      Math.max(0, Math.round((now - startedAtMs) / 1000)),
    ),
    ...renderCounter(
      'dd_runtime_worker_ws_connections_total',
      'Worker websocket connections accepted by the dd remote runtime.',
    ),
    ...renderCounter(
      'dd_runtime_worker_ws_messages_total',
      'Worker websocket messages observed by the dd remote runtime.',
    ),
    ...renderGauge(
      'dd_runtime_worker_ws_active_connections',
      'Currently active worker websocket connections.',
      activeWorkerSockets.size,
    ),
    ...renderGauge(
      'dd_runtime_terminal_ws_active_connections',
      'Currently active worker terminal websocket connections.',
      activeTerminalSockets.size,
    ),
  ];
  return `${lines.join('\n')}\n`;
}

function headerMatches(value: string | string[] | undefined, expected: string): boolean {
  if (Array.isArray(value)) {
    return value.includes(expected);
  }
  return value === expected;
}

function rejectUpgrade(socket: Socket, status: number, message: string): void {
  const body = JSON.stringify({ error: 'unauthorized', errMessage: message });
  socket.write(
    `HTTP/1.1 ${status} ${status === 401 ? 'Unauthorized' : 'Bad Request'}\r\n` +
      'Content-Type: application/json\r\n' +
      `Content-Length: ${Buffer.byteLength(body)}\r\n` +
      'Connection: close\r\n' +
      '\r\n' +
      body,
  );
  socket.destroy();
}

function createWebSocketAcceptKey(key: string): string {
  return createHash('sha1').update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');
}

function writeWebSocketFrame(
  socket: Socket,
  opcode: number,
  payload: string | Buffer = Buffer.alloc(0),
): void {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(payload, 'utf8');
  const length = body.length;
  let header: Buffer;
  if (length < 126) {
    header = Buffer.from([0x80 | opcode, length]);
  } else if (length <= 0xffff) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header.writeUInt16BE(length, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(length), 2);
  }
  socket.write(Buffer.concat([header, body]));
}

function taskEventPayload(ev: {
  taskId: string;
  threadId?: string;
  userId?: string;
  seq: number;
  event: BusEvent['event'] | WrappedEvent;
}): Record<string, unknown> {
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
  private buffer = Buffer.alloc(0);
  private closed = false;
  private readonly subscription: Subscription;
  private readonly heartbeat: ReturnType<typeof setInterval>;

  constructor(
    private readonly socket: Socket,
    private readonly threadId: string,
    private readonly taskId?: string,
  ) {
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
      totalTracked: tasks.size,
      sessionCount: sessions.size,
      atMs: Date.now(),
    });
    this.replayExistingEvents();
  }

  receive(chunk: Buffer): void {
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
      } else if (length === 127) {
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
        payload = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]!));
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

  handleText(text: string): void {
    let parsed: WebSocketJsonValue;
    try {
      parsed = JSON.parse(text);
    } catch {
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

  replayExistingEvents(): void {
    const candidates = this.taskId
      ? [tasks.get(this.taskId)].filter((task): task is TaskState => Boolean(task))
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
        this.sendJson(
          taskEventPayload({
            taskId: task.taskId,
            threadId: task.threadId,
            userId: task.userId,
            seq: stored.seq,
            event: stored.event,
          }),
        );
      }
    }
  }

  shouldForward(ev: BusEvent): boolean {
    if (this.taskId && ev.taskId === this.taskId) {
      return true;
    }
    return Boolean(this.threadId && ev.threadId === this.threadId);
  }

  sendJson(payload: Record<string, unknown>): void {
    if (this.closed || this.socket.destroyed) {
      return;
    }
    writeWebSocketFrame(this.socket, 0x1, JSON.stringify(payload));
  }

  close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    clearInterval(this.heartbeat);
    this.subscription.unsubscribe();
    activeWorkerSockets.delete(this);
    try {
      writeWebSocketFrame(this.socket, 0x8);
    } catch {
      /* socket may already be gone */
    }
    this.socket.destroy();
  }
}

function terminalPageHtml(threadId: string): string {
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

async function executableAvailable(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

function terminalShellCommand(shell: string): string {
  const normalized = shell.trim();
  if (normalized === 'bash' || normalized.endsWith('/bash')) return 'bash -i';
  if (normalized === 'sh' || normalized.endsWith('/sh')) return 'sh -i';
  return 'bash -i';
}

class TerminalWebSocketClient {
  private buffer = Buffer.alloc(0);
  private closed = false;
  private child?: ChildProcess;

  constructor(
    private readonly socket: Socket,
    private readonly threadId: string,
  ) {
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

  async start(): Promise<void> {
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
        ...(process.env as Record<string, string>),
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
      this.child.stdout?.on('data', (chunk: Buffer) => {
        this.sendOutput(chunk.toString('utf8'));
      });
      this.child.stderr?.on('data', (chunk: Buffer) => {
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
    } catch (error) {
      this.sendJson({
        type: 'terminal-error',
        source: 'node-worker-terminal',
        message: error instanceof Error ? error.message : String(error),
        atMs: Date.now(),
      });
      this.close();
    }
  }

  receive(chunk: Buffer): void {
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
      } else if (length === 127) {
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
        payload = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]!));
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

  handleText(text: string): void {
    let parsed: WebSocketJsonValue;
    try {
      parsed = JSON.parse(text);
    } catch {
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

  sendOutput(data: string): void {
    this.sendJson({
      type: 'terminal-output',
      source: 'node-worker-terminal',
      data,
      atMs: Date.now(),
    });
  }

  sendJson(payload: Record<string, unknown>): void {
    if (this.closed || this.socket.destroyed) {
      return;
    }
    writeWebSocketFrame(this.socket, 0x1, JSON.stringify(payload));
  }

  close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    activeTerminalSockets.delete(this);
    if (this.child && !this.child.killed) {
      try {
        this.child.kill('SIGHUP');
      } catch {
        /* shell may already be gone */
      }
    }
    try {
      writeWebSocketFrame(this.socket, 0x8);
    } catch {
      /* socket may already be gone */
    }
    this.socket.destroy();
  }
}

function registerWorkerWebSocketUpgrade(): void {
  fastify.server.on('upgrade', (request: IncomingMessage, socket: Socket, head: Buffer) => {
    const requestUrl = new URL(
      request.url ?? '/',
      `http://${request.headers.host ?? 'localhost'}`,
    );
    if (requestUrl.pathname !== '/ws' && requestUrl.pathname !== '/terminal/ws') {
      rejectUpgrade(socket, 404, 'websocket path not found');
      return;
    }
    if (
      !config.serverAuthSecret ||
      !headerMatches(request.headers['x-server-auth'], config.serverAuthSecret)
    ) {
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
    socket.write(
      'HTTP/1.1 101 Switching Protocols\r\n' +
        'Upgrade: websocket\r\n' +
        'Connection: Upgrade\r\n' +
        `Sec-WebSocket-Accept: ${createWebSocketAcceptKey(key)}\r\n` +
        '\r\n',
    );
    const client =
      requestUrl.pathname === '/terminal/ws'
        ? new TerminalWebSocketClient(socket, requestedThreadId)
        : new WorkerWebSocketClient(socket, requestedThreadId, taskId);
    if (head.length > 0) {
      client.receive(head);
    }
  });
}

fastify.addHook('preHandler', async (req, reply) => {
  const requestPath = req.url.split('?')[0] ?? req.url;
  if (requestPath === '/healthz' || requestPath === '/metrics' || requestPath === '/favicon.ico') {
    return;
  }
  // GET /stream/:taskId may auth via short-lived HMAC token (?token=)
  // for direct browser → docker SSE connections that bypass Vercel's
  // 800s function cap. Defer that check to the route handler.
  if (req.method === 'GET' && requestPath.startsWith('/stream/')) {
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
  totalTracked: tasks.size,
  sessionCount: sessions.size,
  containerPoolConfigured: containerPoolConfigured(config.containerPool),
}));

fastify.get('/metrics', async (_req, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});

fastify.get('/status', async () => ({
  ok: true,
  serverInstanceId,
  startedAt: serverStartedAt,
  pinnedThreadId: config.threadId,
  pinnedUserId: config.userId,
  inFlightCount: Array.from(tasks.values()).filter((t) => !t.finished).length,
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
  const snapshot = Array.from(tasks.values()).map((t) => ({
    taskId: t.taskId,
    threadId: t.threadId,
    userId: t.userId,
    branch: t.branch,
    sessionId: t.session.sessionId,
    finished: t.finished,
    finishedAt: t.finishedAt,
    eventCount: t.events.length,
    lastSeq: t.events.length > 0 ? t.events[t.events.length - 1]!.seq : -1,
  }));
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
  try {
    return await dispatchContainerPool(config.containerPool, params.data.pool, {
      ...parsed.data,
      poolSlug: parsed.data.poolSlug ?? params.data.pool,
    });
  } catch (error) {
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
   * then "gemini-sdk". Validated by the selector — unknown values fall
   * back to default rather than 400ing.
   */
  provider: z
    .enum(['claude-cli', 'claude-sdk', 'gemini-sdk', 'openai-codex-cli', 'openai-sdk'])
    .nullish(),
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
  return withSpan(
    'remote-dev.dispatch-task',
    {
      'http.method': req.method,
      'http.route': '/tasks',
      'dd.remote.thread_id': config.threadId ?? undefined,
    },
    async (span) => {
      const parsed = DispatchSchema.safeParse(req.body);
      if (!parsed.success) {
        return reply.code(400).send({ error: parsed.error.format() });
      }
      const { prompt } = parsed.data;
      const taskId = parsed.data.taskId ?? randomUUID();
      span.setAttribute('dd.remote.task_id', taskId);

      const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
      const userId = parsed.data.userId ?? config.userId ?? undefined;
      if (threadId) {
        span.setAttribute('dd.remote.thread_id', threadId);
      }
      if (userId) {
        span.setAttribute('dd.remote.user_id', userId);
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
      if (requestedRepo && requestedRepo !== config.repoUrl) {
        return reply.code(409).send({
          error: 'container is bound to a different repo',
          boundRepo: config.repoUrl,
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

      const state: TaskState = {
    taskId,
    prompt,
    userId,
    threadId,
    provider: resolveAgentProvider(parsed.data.provider ?? undefined),
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
        event$: new ReplaySubject<StoredEvent>(),
        finished: false,
        cancelled: false,
        worktreePath: session.workspacePath,
        branch: session.branch,
        logPath: session.logPath,
      };
      span.setAttribute('dd.remote.provider', state.provider);
      span.setAttribute('dd.remote.branch', state.branch);
      tasks.set(taskId, state);
      await writeTaskReceipt({
        taskId,
        threadId,
        branch: state.branch,
        provider: state.provider,
        acceptedAt: new Date().toISOString(),
      });
      emit(state, { kind: 'status', status: 'queued' });

      const queuedRun = session.queue.catch(() => undefined).then(() => runTask(state));
      session.queue = queuedRun.catch(() => undefined);

      queuedRun.catch((err: unknown) => {
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

      return { taskId, branch: state.branch };
    },
  );
});

fastify.post('/thread/merge-upstream', async (req, reply) => {
  return withSpan(
    'remote-dev.merge-upstream',
    {
      'http.method': req.method,
      'http.route': '/thread/merge-upstream',
      'dd.remote.thread_id': config.threadId ?? undefined,
    },
    async (span) => {
      const parsed = MergeUpstreamSchema.safeParse(req.body ?? {});
      if (!parsed.success) {
        return reply.code(400).send({ error: parsed.error.format() });
      }
      const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
      if (threadId) {
        span.setAttribute('dd.remote.thread_id', threadId);
      }
      try {
        const result = await mergeUpstreamForThread(parsed.data);
        span.setAttribute('dd.remote.branch', result.branch);
        span.setAttribute('dd.remote.base_branch', result.baseBranch);
        return result;
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        const status = message.includes('pinned to') ? 409 : 500;
        return reply.code(status).send({ error: message });
      }
    },
  );
});

fastify.post('/thread/make-commit', async (req, reply) => {
  return withSpan(
    'remote-dev.make-commit',
    {
      'http.method': req.method,
      'http.route': '/thread/make-commit',
      'dd.remote.thread_id': config.threadId ?? undefined,
    },
    async (span) => {
      const parsed = MakeCommitSchema.safeParse(req.body ?? {});
      if (!parsed.success) {
        return reply.code(400).send({ error: parsed.error.format() });
      }
      const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
      if (threadId) {
        span.setAttribute('dd.remote.thread_id', threadId);
      }
      try {
        const result = await makeCommitForThread(parsed.data);
        span.setAttribute('dd.remote.branch', result.branch);
        span.setAttribute('dd.remote.committed', result.committed);
        span.setAttribute('dd.remote.pushed', result.pushed);
        return result;
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        const status = message.includes('pinned to') ? 409 : 500;
        return reply.code(status).send({ error: message });
      }
    },
  );
});

fastify.post('/thread/open-pr', async (req, reply) => {
  return withSpan(
    'remote-dev.open-pr',
    {
      'http.method': req.method,
      'http.route': '/thread/open-pr',
      'dd.remote.thread_id': config.threadId ?? undefined,
    },
    async (span) => {
      const parsed = OpenPullRequestSchema.safeParse(req.body ?? {});
      if (!parsed.success) {
        return reply.code(400).send({ error: parsed.error.format() });
      }
      const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
      if (threadId) {
        span.setAttribute('dd.remote.thread_id', threadId);
      }
      try {
        const result = await openPullRequestForThread(parsed.data);
        span.setAttribute('dd.remote.branch', result.branch);
        span.setAttribute('dd.remote.base_branch', result.baseBranch);
        span.setAttribute('dd.remote.pr_url', result.prUrl);
        return result;
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        const status = message.includes('pinned to') ? 409 : 500;
        return reply.code(status).send({ error: message });
      }
    },
  );
});

fastify.get('/stream/:taskId', (req, reply) => {
  const { taskId } = req.params as { taskId: string };

  // Auth: either X-Server-Auth (server-to-server, e.g. Vercel proxy) or
  // a short-lived HMAC token in ?token= for direct browser connections.
  // For direct-browser tokens we ALSO require the token's userId to match
  // the task's owner — otherwise a valid token for task A could be
  // weaponised against task B if its taskId leaked.
  const tokenParam = (req.query as { token?: string }).token;
  let tokenAuthed = false;
  if (typeof tokenParam === 'string' && tokenParam.length > 0) {
    const payload = verifyDirectStreamToken(tokenParam);
    const candidate = tasks.get(taskId);
    if (
      !payload ||
      payload.taskId !== taskId ||
      !candidate ||
      candidate.userId !== payload.userId
    ) {
      reply.code(401).send({ error: 'unauthorized' });
      return;
    }
    tokenAuthed = true;
  }
  if (
    !tokenAuthed &&
    (!config.serverAuthSecret || req.headers['x-server-auth'] !== config.serverAuthSecret)
  ) {
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
  const resumeFromIdParam = (req.query as { resumeFromId?: string }).resumeFromId;
  const lastEventIdRaw =
    typeof lastEventIdHeader === 'string' ? lastEventIdHeader : resumeFromIdParam;
  const lastEventIdNumber = lastEventIdRaw ? Number(lastEventIdRaw) : -1;
  const lastEventId = Number.isFinite(lastEventIdNumber)
    ? Math.max(-1, Math.trunc(lastEventIdNumber))
    : -1;

  const send = (s: StoredEvent): void => {
    reply.raw.write(`id: ${s.seq}\nevent: ${s.event.kind}\ndata: ${JSON.stringify(s.event)}\n\n`);
  };

  const disconnected$ = new Subject<void>();
  const subscription = state.event$
    .pipe(
      filter((s) => s.seq > lastEventId),
      takeUntil(disconnected$),
    )
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
  const { taskId } = req.params as { taskId: string };
  const state = tasks.get(taskId);
  if (!state) {
    return reply.code(404).send({ error: 'not found' });
  }
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
  } catch {
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
    if (
      state.finished &&
      state.finishedAt !== undefined &&
      now - state.finishedAt > config.taskGcAfterMs
    ) {
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

let cachedOwnIp: string | null = null;

async function sendHeartbeat(): Promise<void> {
  if (!config.heartbeatUrl || !config.heartbeatSecret) {
    return;
  }
  if (!cachedOwnIp) {
    cachedOwnIp = await discoverOwnIp();
  }
  const inFlight = Array.from(tasks.values()).map((t) => ({
    taskId: t.taskId,
    threadId: t.threadId,
    userId: t.userId,
    branch: t.branch,
    finished: t.finished,
    finishedAt: t.finishedAt,
    eventCount: t.events.length,
    lastSeq: t.events.length > 0 ? t.events[t.events.length - 1]!.seq : -1,
  }));
  const containerInfo = config.threadId
    ? {
        threadId: config.threadId,
        ip: cachedOwnIp,
        port: config.port,
        status: 'ready' as const,
        podName: process.env.POD_NAME ?? process.env.HOSTNAME ?? serverInstanceId,
        namespace: process.env.POD_NAMESPACE ?? process.env.K8S_NAMESPACE ?? '',
        orchestrator: process.env.K8S_API_SERVER
          ? ('k8s' as const)
          : process.env.ECS_CONTAINER_METADATA_URI_V4
            ? ('ecs' as const)
            : ('docker-compose' as const),
      }
    : undefined;
  try {
    await fetch(config.heartbeatUrl, {
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
        container: containerInfo,
      }),
    });
  } catch {
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

async function discoverOwnIp(): Promise<string> {
  if (process.env.POD_IP) {
    return process.env.POD_IP;
  }

  const ecsMetaUri = process.env.ECS_CONTAINER_METADATA_URI_V4;
  if (ecsMetaUri) {
    try {
      const res = await fetch(`${ecsMetaUri}/task`);
      if (res.ok) {
        const meta = (await res.json()) as {
          Containers?: Array<{
            Networks?: Array<{ IPv4Addresses?: string[] }>;
          }>;
        };
        const ip = meta.Containers?.[0]?.Networks?.[0]?.IPv4Addresses?.[0];
        if (ip) {
          return ip;
        }
      }
    } catch {
      /* fall through */
    }
  }

  return '0.0.0.0';
}

async function main(): Promise<void> {
  initTelemetry();

  if (!config.threadId && config.workerBindMode !== 'repo') {
    throw new Error(
      'REMOTE_DEV_THREAD_ID or THREAD_ID is required unless WORKER_BIND_MODE=repo.',
    );
  }
  if (!config.repoUrl) {
    throw new Error('DD_REPO_URL is required — the container must be pinned to one git repo.');
  }
  if (!config.serverAuthSecret) {
    fastify.log.warn('SERVER_AUTH_SECRET is not set — all non-healthz requests will 401');
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
    fastify.log.info(
      `EventBus: idle watchdog active (${config.idleTimeoutMs / 1000}s) for thread ${config.threadId}`,
    );
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
    fastify.log.info(
      `agent providers — available: [${installed || 'none'}]` +
        (missing ? ` · unavailable: [${missing}]` : ''),
    );
  });

  // Per-thread pods pre-warm their single session. Repo-scoped pool workers
  // create sessions lazily per task/thread.
  if (config.threadId) {
    const bootSession = getOrCreateSession({
      taskId: config.threadId,
      threadId: config.threadId,
    });
    await bootSession.ready;
  }

  registerWorkerWebSocketUpgrade();
  await fastify.listen({ host: config.host, port: config.port });
}

function shutdown(signal: string): void {
  fastify.log.info(`${signal} received — tearing down EventBus + channels`);
  workerFanout.destroy();
  natsPublisher.destroy();
  eventBus.destroy();
  destroyChannelPool();
  fastify.close().then(
    () => shutdownTelemetry().finally(() => process.exit(0)),
    () => process.exit(1),
  );
  setTimeout(() => process.exit(1), 10_000).unref();
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));

main().catch((err) => {
  fastify.log.error(err);
  workerFanout.destroy();
  natsPublisher.destroy();
  eventBus.destroy();
  shutdownTelemetry().finally(() => process.exit(1));
});

/* eslint-enable security/detect-non-literal-fs-filename */
