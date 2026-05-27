/* eslint-disable security/detect-non-literal-fs-filename -- remote-dev manages configured workspace, log, and artifact paths. */
// dd-dev-server — HTTP server that runs Claude/OpenAI coding agents inside
// a warm configured git workspace, then streams events.
//
// Endpoints (all auth'd via X-Server-Auth header except /healthz):
//   POST /tasks                  — { taskId?, threadId?, prompt } → queues task
//   GET  /stream/:taskId         — Server-Sent Events of agent activity
//   POST /tasks/:taskId/cancel   — abort an in-flight task
//   GET  /healthz                — liveness probe
//
// Per thread, the server prepares/reuses a stable branch
// dev/<threadId>/<slugified-title>.
// Per task, it runs the selected provider, streams sequenced events,
// appends tmp/convos/thread.log, pushes the branch, and creates/reuses a PR.
//
// Worktrees + finished tasks are GC'd one hour after completion.

import Fastify from "fastify";
import { spawn, type ChildProcess } from "node:child_process";
import {
  access,
  appendFile,
  mkdir,
  readdir,
  stat,
  writeFile,
} from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { randomUUID } from "node:crypto";
import { ReplaySubject, Subject, interval } from "rxjs";
import { filter, takeUntil } from "rxjs/operators";
import { z } from "zod";

import { EventBus, type BusEvent } from "./event-bus.js";
import {
  containerPoolConfigFromEnv,
  containerPoolConfigured,
  dispatchContainerPool,
  type ContainerPoolDispatchRequest,
  type ContainerPoolDispatchResponse,
} from "./container-pool.js";

import {
  buildAgentEnv,
  getCachedAvailability,
  getRunner,
  probeAllProviders,
  resolveAgentProvider,
  type AgentProvider,
  type AgentRunnerEvent,
} from "./agents/index.js";
import { publishArtifact } from "./storage/index.js";
import type {
  PublishedArtifact,
  StorageProvider,
} from "./storage/types.js";
import {
  acquireUserChannel,
  destroyChannelPool,
  isRealtimeEnabled,
  publishUserEvent,
  releaseUserChannel,
} from "./realtime.js";
import { verifyDirectStreamToken } from "./token.js";

// ---------- Config ----------

const config = {
  port: Number(process.env.PORT ?? 8080),
  host: process.env.HOST ?? "0.0.0.0",
  workspaceRepo: process.env.WORKSPACE_REPO ?? "/home/agent/workspace/repo",
  // The container is pinned to a single threadId originating from
  // /u/admin/remote-dev. Set via REMOTE_DEV_THREAD_ID (preferred) or
  // THREAD_ID (fallback). The server refuses to start without one — see
  // main().
  threadId: process.env.REMOTE_DEV_THREAD_ID ?? process.env.THREAD_ID ?? null,
  userId: process.env.USER_ID ?? null,
  // Each agent run writes publishable files into ${OUTPUTS_DIR}/<taskId>/.
  // After claude exits, runTask scans that dir and uploads each file via
  // the configured storage adapter, emitting an `artifact` event per file.
  outputsDir: process.env.OUTPUTS_DIR ?? "/home/agent/workspace/outputs",
  defaultStorageProvider: (process.env.DEFAULT_STORAGE_PROVIDER ??
    "local") as StorageProvider,
  // Periodic heartbeat to Vercel — lets the UI poll an "is the docker
  // alive?" endpoint backed by our most recent ping. Disabled if
  // HEARTBEAT_URL is unset.
  heartbeatUrl: process.env.HEARTBEAT_URL ?? null,
  heartbeatSecret: process.env.HEARTBEAT_SECRET ?? null,
  heartbeatIntervalMs: Number(process.env.HEARTBEAT_INTERVAL_MS ?? 20_000),
  idleTimeoutMs: Number(process.env.IDLE_TIMEOUT_MS ?? 30 * 60 * 1000),
  agentRunTimeoutMs: Number(
    process.env.AGENT_RUN_TIMEOUT_MS ?? 2 * 60 * 60_000,
  ),
  ghDeployKeyPath:
    process.env.GH_DEPLOY_KEY_PATH ?? "/home/agent/.ssh/id_ed25519",
  ghDeployKey: process.env.GH_DEPLOY_KEY ?? null,
  ghPat: process.env.GH_PAT ?? null,
  anthropicApiKey: process.env.ANTHROPIC_API_KEY ?? null,
  serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
  eventIngestUrl: process.env.EVENT_INGEST_URL ?? null,
  eventIngestSecret: process.env.EVENT_INGEST_SECRET ?? null,
  baseBranch: process.env.BASE_BRANCH ?? "dev",
  threadLogRelativePath:
    process.env.THREAD_LOG_RELATIVE_PATH ?? "tmp/convos/thread.log",
  skipBootGitSync: process.env.SKIP_BOOT_GIT_SYNC === "true",
  sessionIdleGcAfterMs: Number(
    process.env.SESSION_IDLE_GC_AFTER_MS ?? 6 * 60 * 60 * 1000,
  ),
  prAuthor: {
    name: process.env.GIT_AUTHOR_NAME ?? "DD Agent",
    email: process.env.GIT_AUTHOR_EMAIL ?? "agent@dancingdragons.dev",
  },
  logDir: process.env.LOG_DIR ?? "/tmp/convos",
  containerPool: containerPoolConfigFromEnv(process.env),
  taskGcAfterMs: 60 * 60 * 1000, // 1 hour
  taskGcIntervalMs: 5 * 60 * 1000, // 5 min sweep
};

// Paths the dev-server owns by contract and must never treat as user
// repo content (commits, dirty-state guards, install-artifact cleans):
//   - dependency caches that should not pollute commits or dirty checks
//   - tmp/convos: per-thread breadcrumb log written by this server even
//     when the underlying repo does not gitignore tmp/. Mirrors the
//     k8s dd-dev-server contract; see remote/deployments/dev-server/src/server.ts
//     and AGENTS.md for the breadcrumb contract.
const GENERATED_GIT_EXCLUDE_PATHS = [
  ".pnpm-store",
  "node_modules",
  ".next",
  ".turbo",
  "tmp/convos",
];
const GENERATED_GIT_STATUS_EXCLUDES = GENERATED_GIT_EXCLUDE_PATHS.map(
  (path) => `:(exclude)${path}`,
);

// ---------- Event types ----------

type WrappedEvent =
  | { kind: "status"; status: string }
  | { kind: "claude"; raw: unknown }
  | { kind: "stderr"; text: string }
  | { kind: "error"; message: string }
  | { kind: "artifact"; artifact: PublishedArtifact }
  | {
      kind: "container_pool";
      pool: string;
      response: ContainerPoolDispatchResponse;
    }
  | {
      kind: "done";
      branch: string;
      prUrl?: string;
      exitReason: "completed" | "cancelled" | "failed";
    };

type StoredEvent = { seq: number; event: WrappedEvent };

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

type TaskState = {
  taskId: string;
  prompt: string;
  /** dd-user UUID — used as the Supabase Realtime channel scope. */
  userId?: string;
  /** Optional Vercel-side thread id, for cross-referencing in events. */
  threadId?: string;
  /** Which runner is driving this task (claude-cli / openai-codex-cli / ...). */
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

const tasks = new Map<string, TaskState>();
const sessions = new Map<string, ThreadSession>();

const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();

// ---- RxJS EventBus — reactive pipeline for events ----
const eventBus = new EventBus();

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
    optsOrExtraEnv && typeof optsOrExtraEnv === "object" &&
      ("timeoutMs" in optsOrExtraEnv ||
        "extraEnv" in optsOrExtraEnv ||
        "isolatedEnv" in optsOrExtraEnv)
      ? (optsOrExtraEnv as ShCaptureOptions)
      : { extraEnv: optsOrExtraEnv as Record<string, string> | undefined };

  const env = opts.isolatedEnv
    ? opts.isolatedEnv
    : { ...(process.env as Record<string, string>), ...(opts.extraEnv ?? {}) };

  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd, env });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let killTimer: ReturnType<typeof setTimeout> | null = null;
    if (opts.timeoutMs && opts.timeoutMs > 0) {
      killTimer = setTimeout(() => {
        timedOut = true;
        try {
          child.kill("SIGKILL");
        } catch {
          /* ignore */
        }
      }, opts.timeoutMs);
    }
    child.stdout.on("data", (d: Buffer) => {
      stdout += d.toString("utf8");
    });
    child.stderr.on("data", (d: Buffer) => {
      stderr += d.toString("utf8");
    });
    child.on("close", (code) => {
      if (killTimer) {clearTimeout(killTimer);}
      if (timedOut) {
        reject(
          new Error(
            `${cmd} ${args.join(" ")} timed out after ${opts.timeoutMs}ms`,
          ),
        );
        return;
      }
      if (code === 0) {resolve(stdout);}
      else {
        reject(
          new Error(
            `${cmd} ${args.join(" ")} exited ${code}: ${stderr.slice(0, 1000)}`,
          ),
        );
      }
    });
    child.on("error", (err) => {
      if (killTimer) {clearTimeout(killTimer);}
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
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .replace(/-{2,}/g, "-")
    .slice(0, 80);
  return slug || "thread";
}

function getSessionBranch(
  sessionId: string,
  branchHint?: string | null,
  titleHint?: string | null,
): string {
  const hinted = branchHint?.trim();
  if (hinted) {
    return hinted;
  }
  const titleSlug = slugifyBranchFragment(titleHint?.trim() || sessionId);
  return `dev/${sessionId}/${titleSlug}`;
}

function getSessionLogPath(workspacePath: string): string {
  return join(workspacePath, config.threadLogRelativePath);
}

async function remoteBranchExists(branch: string): Promise<boolean> {
  const out = await shCapture(
    "git",
    ["ls-remote", "--heads", "origin", branch],
    config.workspaceRepo,
    { timeoutMs: TIMEOUT_GIT_NETWORK },
  );
  return out.trim().length > 0;
}

async function prepareSessionWorkspace(session: ThreadSession): Promise<void> {
  if (config.threadId && session.sessionId !== config.threadId) {
    throw new Error(
      `container is pinned to thread ${config.threadId}, got ${session.sessionId}`,
    );
  }

  if (config.skipBootGitSync) {
    await configureGitIdentity(session.workspacePath);
    await mkdir(dirname(session.logPath), { recursive: true });
    await appendFile(
      session.logPath,
      JSON.stringify({
        ts: new Date().toISOString(),
        kind: "session-ready",
        sessionId: session.sessionId,
        branch: session.branch,
        workspacePath: session.workspacePath,
        baseBranch: config.baseBranch,
        skippedBootGitSync: true,
      }) + "\n",
    );
    return;
  }

  await shCapture(
    "git",
    ["fetch", "--quiet", "origin", config.baseBranch],
    config.workspaceRepo,
    { timeoutMs: TIMEOUT_GIT_NETWORK },
  );

  const hasRemoteBranch = await remoteBranchExists(session.branch);
  if (hasRemoteBranch) {
    await shCapture(
      "git",
      ["fetch", "--quiet", "origin", session.branch],
      config.workspaceRepo,
      { timeoutMs: TIMEOUT_GIT_NETWORK },
    );
  }

  await shCapture(
    "git",
    [
      "switch",
      "--discard-changes",
      "-C",
      session.branch,
      hasRemoteBranch
        ? `origin/${session.branch}`
        : `origin/${config.baseBranch}`,
    ],
    config.workspaceRepo,
    { timeoutMs: TIMEOUT_GIT_QUICK },
  );

  // Local-only: entrypoint.sh already ran `pnpm install` synchronously
  // against this workspace before exec'ing the server, so re-running it
  // here just doubles boot time and risks racing the lockfile that the
  // background install may have rewritten with --no-frozen-lockfile.
  // Production keeps both installs because boot is concurrent there.

  await configureGitIdentity(session.workspacePath);
  await mkdir(dirname(session.logPath), { recursive: true });
  await appendFile(
    session.logPath,
    JSON.stringify({
      ts: new Date().toISOString(),
      kind: "session-ready",
      sessionId: session.sessionId,
      branch: session.branch,
      workspacePath: session.workspacePath,
      baseBranch: config.baseBranch,
    }) + "\n",
  );
}

function getOrCreateSession(input: {
  taskId: string;
  threadId?: string;
  userId?: string;
  branch?: string;
  threadTitle?: string;
}): ThreadSession {
  const sessionId = getSessionId(input.threadId, input.taskId);
  const existing = sessions.get(sessionId);
  if (existing) {
    existing.lastActiveAt = Date.now();
    if (!existing.userId && input.userId) {existing.userId = input.userId;}
    return existing;
  }

  const workspacePath = getSessionWorkspacePath(sessionId);
  const session: ThreadSession = {
    sessionId,
    userId: input.userId,
    workspacePath,
    branch: getSessionBranch(sessionId, input.branch, input.threadTitle),
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

async function appendThreadLog(
  state: TaskState,
  payload: Record<string, unknown>,
): Promise<void> {
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
      }) + "\n",
    );
  } catch (err) {
    if (
      err &&
      typeof err === "object" &&
      "code" in err &&
      err.code === "ENOENT"
    ) {
      return;
    }
    process.stderr.write(
      `[remote-dev thread-log] append failed: ${
        err instanceof Error ? err.message : String(err)
      }\n`,
    );
  }
}

function emit(state: TaskState, event: WrappedEvent): void {
  // Once a task has emitted `done`, it's terminal — late events from a
  // race (e.g. cancel firing while claude was already exiting cleanly)
  // would corrupt the seq stream and confuse downstream consumers.
  if (state.finished && event.kind !== "done") {
    return;
  }
  if (state.finished && event.kind === "done") {
    return; // dedupe duplicate done emits (cancel + natural close race)
  }
  const stored: StoredEvent = { seq: state.events.length, event };
  state.events.push(stored);
  state.event$.next(stored);
  void appendThreadLog(state, { kind: "event", seq: stored.seq, event });
  if (event.kind === "done") {
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
    event: event as unknown as BusEvent["event"],
  });
}

async function ensureDeployKey(): Promise<void> {
  if (!config.ghDeployKey) {return;}
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
  await shCapture("git", ["config", "user.name", config.prAuthor.name], cwd, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
  await shCapture("git", ["config", "user.email", config.prAuthor.email], cwd, {
    timeoutMs: TIMEOUT_GIT_QUICK,
  });
}

// ---------- Per-task workflow ----------

async function runTask(state: TaskState): Promise<void> {
  if (state.userId) {
    acquireUserChannel(state.userId);
  }

  if (state.finished || state.cancelled) {
    if (!state.finished) {
      emit(state, {
        kind: "done",
        branch: state.branch,
        exitReason: "cancelled",
      });
    }
    return;
  }

  // If the container started via entrypoint.sh, the git fetch + switch
  // runs as a background process. Wait for it to finish before we
  // proceed — otherwise the worktree may branch off stale state.
  const gitReadyPid = process.env.GIT_READY_PID;
  if (gitReadyPid) {
    emit(state, { kind: "status", status: "waiting-for-workspace" });
    try {
      // waitpid via polling — Node doesn't expose waitpid() natively.
      // Once the PID is gone from /proc, the background git is done.
      await new Promise<void>((resolve) => {
        const check = (): void => {
          try {
            process.kill(Number(gitReadyPid), 0); // signal 0 = existence check
            setTimeout(check, 500);
          } catch {
            resolve(); // process gone — git is done
          }
        };
        check();
      });
      // Clear so subsequent tasks don't re-wait.
      delete process.env.GIT_READY_PID;
    } catch {
      /* if the PID was already gone, that's fine */
    }
  }

  emit(state, { kind: "status", status: "syncing-thread-workspace" });
  await state.session.ready;
  state.session.lastActiveAt = Date.now();

  // Per-task outputs dir — the agent writes publishable files here.
  // After claude exits we scan it and upload each file via the storage
  // adapter, emitting an `artifact` event per file.
  const taskOutputsDir = join(config.outputsDir, state.taskId);
  await mkdir(taskOutputsDir, { recursive: true });
  await appendThreadLog(state, {
    kind: "prompt",
    prompt: state.prompt,
    workspacePath: state.worktreePath,
    branch: state.branch,
  });

  if (state.containerPool) {
    emit(state, {
      kind: "status",
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
      kind: "container-pool-result",
      pool: state.containerPool.pool,
      response,
    });
    emit(state, {
      kind: "container_pool",
      pool: state.containerPool.pool,
      response,
    });
    emit(state, {
      kind: "done",
      branch: state.branch,
      exitReason: response.ok ? "completed" : "failed",
    });
    return;
  }

  emit(state, { kind: "status", status: `agent-running:${state.provider}` });
  // Strict env allowlist owned by the runner module. Inheriting the full
  // process.env into the agent process would leak our GitHub deploy key,
  // Supabase service role key, ingest secret, etc. via any `env` or
  // `printenv` tool call. The runner adds only the API key its model
  // needs.
  const agentEnv = buildAgentEnv(state.provider);
  const runner = getRunner(state.provider);
  await runner.run({
    prompt: state.prompt,
    cwd: state.worktreePath,
    env: agentEnv,
    signal: state.abortController.signal,
    timeoutMs: config.agentRunTimeoutMs,
    emit: (ev: AgentRunnerEvent) => emit(state, ev),
    setChild: (child: ChildProcess) => {
      state.child = child;
    },
  });

  if (state.cancelled || state.abortController.signal.aborted) {
    emit(state, {
      kind: "done",
      branch: state.branch,
      exitReason: "cancelled",
    });
    return;
  }

  // Stage + commit anything the agent left uncommitted, then push.
  // Path excludes (and the post-`add` reset) keep the dev-server's own
  // breadcrumb log + dependency caches out of the resulting PR even when
  // the underlying repo doesn't gitignore tmp/, node_modules, etc.
  emit(state, { kind: "status", status: "pushing" });
  const status = await shCapture(
    "git",
    [
      "status",
      "--porcelain",
      "--untracked-files=all",
      "--",
      ".",
      ...GENERATED_GIT_STATUS_EXCLUDES,
    ],
    state.worktreePath,
    { timeoutMs: TIMEOUT_GIT_QUICK },
  );
  if (status.trim()) {
    await shCapture("git", ["add", "-A", "--", "."], state.worktreePath, {
      timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture(
      "git",
      ["reset", "-q", "HEAD", "--", ...GENERATED_GIT_EXCLUDE_PATHS],
      state.worktreePath,
      { timeoutMs: TIMEOUT_GIT_QUICK },
    );
    let hasStagedChanges = true;
    try {
      await shCapture(
        "git",
        ["diff", "--cached", "--quiet"],
        state.worktreePath,
        { timeoutMs: TIMEOUT_GIT_QUICK },
      );
      hasStagedChanges = false;
    } catch {
      // diff --cached --quiet exits non-zero when there are staged changes
    }
    if (hasStagedChanges) {
      await shCapture(
        "git",
        ["commit", "-m", `agent(${state.session.sessionId}): ${state.taskId}`],
        state.worktreePath,
        { timeoutMs: TIMEOUT_GIT_QUICK },
      );
    }
  }
  await shCapture(
    "git",
    ["push", "--set-upstream", "origin", state.branch],
    state.worktreePath,
    { timeoutMs: TIMEOUT_GIT_NETWORK },
  );

  emit(state, { kind: "status", status: "opening-pr" });
  const prUrl = await ensurePullRequest(state);

  // Publish any files the agent dropped in the per-task outputs dir.
  // Failures uploading individual files are surfaced as `error` events
  // but do not fail the whole task — the PR still got opened.
  await publishOutputs(state, taskOutputsDir);

  emit(state, {
    kind: "done",
    branch: state.branch,
    prUrl,
    exitReason: "completed",
  });
}

async function ensurePullRequest(state: TaskState): Promise<string | undefined> {
  const ghEnv = config.ghPat ? { GH_TOKEN: config.ghPat } : undefined;
  try {
    const existing = await shCapture(
      "gh",
      ["pr", "view", state.branch, "--json", "url", "--jq", ".url"],
      state.worktreePath,
      { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv },
    );
    const url = existing.trim();
    if (url) {return url;}
  } catch {
    /* no existing PR */
  }

  const commitTitle = (
    await shCapture(
      "git",
      ["log", "-1", "--pretty=%s"],
      state.worktreePath,
      { timeoutMs: TIMEOUT_GIT_QUICK },
    )
  )
    .trim()
    .replace(/\s+/g, " ");

  try {
    const out = await shCapture(
      "gh",
      [
        "pr",
        "create",
        "--base",
        config.baseBranch,
        "--head",
        state.branch,
        "--title",
        commitTitle || `dev/${state.session.sessionId}`,
        "--body",
        `**Thread**\n\n${state.session.sessionId}\n\n**Prompt**\n\n${state.prompt}\n\n_Opened by dd-dev-server._`,
      ],
      state.worktreePath,
      { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv },
    );
    return out
      .trim()
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .pop();
  } catch (err) {
    emit(state, {
      kind: "error",
      message: `gh pr create/view failed: ${(err as Error).message}`,
    });
    return undefined;
  }
}

/**
 * Walk the per-task outputs/ directory, publish every regular file via
 * the configured storage adapter, and emit an `artifact` event for each.
 */
async function publishOutputs(
  state: TaskState,
  taskOutputsDir: string,
): Promise<void> {
  type DirentLike = { name: string; isFile(): boolean; isDirectory(): boolean };

  let dirents: DirentLike[];
  try {
    dirents = (await readdir(taskOutputsDir, {
      withFileTypes: true,
    })) as unknown as DirentLike[];
  } catch {
    return; // dir absent / unreadable → nothing to publish, that's fine
  }

  if (dirents.length === 0) {
    return;
  }

  emit(state, { kind: "status", status: "publishing-artifacts" });

  // Recurse one level so flat-or-nested layouts both work.
  const filesToPublish: string[] = [];
  for (const e of dirents) {
    if (e.isFile()) {
      filesToPublish.push(join(taskOutputsDir, e.name));
    } else if (e.isDirectory()) {
      try {
        const sub = (await readdir(join(taskOutputsDir, e.name), {
          withFileTypes: true,
        })) as unknown as DirentLike[];
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
      emit(state, { kind: "artifact", artifact: published });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      emit(state, {
        kind: "error",
        message: `failed to publish ${basename(filePath)}: ${message}`,
      });
    }
  }
}

// ---------- HTTP server ----------

const fastify = Fastify({ logger: true });

fastify.addHook("preHandler", async (req, reply) => {
  if (req.url === "/healthz") {
    return;
  }
  // GET /stream/:taskId may auth via short-lived HMAC token (?token=)
  // for direct browser → docker SSE connections that bypass Vercel's
  // 800s function cap. Defer that check to the route handler.
  if (req.method === "GET" && req.url.startsWith("/stream/")) {
    return;
  }

  if (
    !config.serverAuthSecret ||
    req.headers["x-server-auth"] !== config.serverAuthSecret
  ) {
    return reply.code(401).send({ error: "unauthorized" });
  }
});

fastify.get("/healthz", async () => ({
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

fastify.get("/status", async () => ({
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

// Provider availability — boot-probed list of which AGENT_PROVIDER
// values can actually be used in this image (binaries on PATH, SDKs
// installed, API keys set). UI uses this to grey out unavailable
// options instead of letting the user pick something that fails with
// ENOENT mid-run.
fastify.get("/agents", async () => {
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
fastify.get("/tasks", async () => {
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

fastify.post("/container-pools/:pool/dispatch", async (req, reply) => {
  const params = z.object({ pool: z.string().min(1).max(120) }).safeParse(req.params);
  const parsed = ContainerPoolRequestSchema.safeParse(req.body);
  if (!params.success || !parsed.success) {
    return reply.code(400).send({
      error: "invalid container pool dispatch",
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
  /** Stable remote-dev thread branch, if the dispatcher already knows it. */
  branch: z.string().max(200).optional(),
  /** Human-readable thread title / branch slug fallback. */
  threadTitle: z.string().min(1).max(200).optional(),
  /**
   * Which agent runner to drive the task. Falls back to AGENT_PROVIDER env
   * then "claude-cli". Validated by the selector — unknown values fall
   * back to default rather than 400ing.
   */
  provider: z
    .enum(["claude-cli", "claude-sdk", "openai-codex-cli", "openai-sdk"])
    .optional(),
  containerPool: ContainerPoolTaskSchema.optional(),
});

fastify.post("/tasks", async (req, reply) => {
  const parsed = DispatchSchema.safeParse(req.body);
  if (!parsed.success) {
    return reply.code(400).send({ error: parsed.error.format() });
  }
  const { prompt } = parsed.data;
  const taskId = parsed.data.taskId ?? randomUUID();
  if (tasks.has(taskId)) {
    return reply.code(409).send({ error: "task exists" });
  }

  const threadId = parsed.data.threadId ?? config.threadId ?? undefined;
  const userId = parsed.data.userId ?? config.userId ?? undefined;
  if (config.threadId && threadId !== config.threadId) {
    return reply.code(409).send({
      error: "container is bound to a different thread",
      boundThreadId: config.threadId,
    });
  }
  if (config.userId && userId !== config.userId) {
    return reply.code(403).send({
      error: "container is bound to a different user",
      boundUserId: config.userId,
    });
  }

  const session = getOrCreateSession({
    taskId,
    threadId,
    userId,
    branch: parsed.data.branch,
    threadTitle: parsed.data.threadTitle,
  });
  session.taskIds.add(taskId);

  const state: TaskState = {
    taskId,
    prompt,
    userId,
    threadId,
    provider: resolveAgentProvider(parsed.data.provider),
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
  tasks.set(taskId, state);
  emit(state, { kind: "status", status: "queued" });

  const queuedRun = session.queue
    .catch(() => undefined)
    .then(() => runTask(state));
  session.queue = queuedRun.catch(() => undefined);

  queuedRun.catch((err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    emit(state, { kind: "error", message });
    if (!state.finished) {
      emit(state, {
        kind: "done",
        branch: state.branch,
        exitReason: "failed",
      });
    }
  });

  return { taskId, branch: state.branch };
});

fastify.get("/stream/:taskId", (req, reply) => {
  const { taskId } = req.params as { taskId: string };

  // Auth: either X-Server-Auth (server-to-server, e.g. Vercel proxy) or
  // a short-lived HMAC token in ?token= for direct browser connections.
  // For direct-browser tokens we ALSO require the token's userId to match
  // the task's owner — otherwise a valid token for task A could be
  // weaponised against task B if its taskId leaked.
  const tokenParam = (req.query as { token?: string }).token;
  let tokenAuthed = false;
  if (typeof tokenParam === "string" && tokenParam.length > 0) {
    const payload = verifyDirectStreamToken(tokenParam);
    const candidate = tasks.get(taskId);
    if (
      !payload ||
      payload.taskId !== taskId ||
      !candidate ||
      candidate.userId !== payload.userId
    ) {
      reply.code(401).send({ error: "unauthorized" });
      return;
    }
    tokenAuthed = true;
  }
  if (
    !tokenAuthed &&
    (!config.serverAuthSecret ||
      req.headers["x-server-auth"] !== config.serverAuthSecret)
  ) {
    reply.code(401).send({ error: "unauthorized" });
    return;
  }

  const state = tasks.get(taskId);
  if (!state) {
    reply.code(404).send({ error: "not found" });
    return;
  }

  reply.hijack();
  reply.raw.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache, no-transform",
    Connection: "keep-alive",
    "X-Accel-Buffering": "no",
  });

  const lastEventIdHeader = req.headers["last-event-id"];
  const resumeFromIdParam = (req.query as { resumeFromId?: string }).resumeFromId;
  const lastEventIdRaw =
    typeof lastEventIdHeader === "string"
      ? lastEventIdHeader
      : resumeFromIdParam;
  const lastEventIdNumber = lastEventIdRaw ? Number(lastEventIdRaw) : -1;
  const lastEventId = Number.isFinite(lastEventIdNumber)
    ? Math.max(-1, Math.trunc(lastEventIdNumber))
    : -1;

  const send = (s: StoredEvent): void => {
    reply.raw.write(
      `id: ${s.seq}\nevent: ${s.event.kind}\ndata: ${JSON.stringify(s.event)}\n\n`,
    );
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
        if (s.event.kind === "done") {
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

  req.raw.on("close", () => {
    clearInterval(heartbeat);
    disconnected$.next();
    disconnected$.complete();
    subscription.unsubscribe();
  });
});

fastify.post("/tasks/:taskId/cancel", async (req, reply) => {
  const { taskId } = req.params as { taskId: string };
  const state = tasks.get(taskId);
  if (!state) {return reply.code(404).send({ error: "not found" });}
  if (state.finished) {return reply.code(409).send({ error: "already finished" });}

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
  if (state.child && !state.child.killed) {state.child.kill("SIGTERM");}
  emit(state, {
    kind: "done",
    branch: state.branch,
    exitReason: "cancelled",
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
    if (
      session.taskIds.size === 0 &&
      now - session.lastActiveAt > config.sessionIdleGcAfterMs
    ) {
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
        status: "ready" as const,
        podName: process.env.POD_NAME ?? process.env.HOSTNAME ?? serverInstanceId,
        namespace: process.env.POD_NAMESPACE ?? process.env.K8S_NAMESPACE ?? "",
        orchestrator: process.env.K8S_API_SERVER
          ? ("k8s" as const)
          : process.env.ECS_CONTAINER_METADATA_URI_V4
            ? ("ecs" as const)
            : ("docker-compose" as const),
      }
    : undefined;
  try {
    await fetch(config.heartbeatUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Heartbeat-Auth": config.heartbeatSecret,
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
  if (process.env.POD_IP) {return process.env.POD_IP;}

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
        if (ip) {return ip;}
      }
    } catch {
      /* fall through */
    }
  }

  return "0.0.0.0";
}

async function main(): Promise<void> {
  if (!config.threadId) {
    throw new Error(
      "REMOTE_DEV_THREAD_ID or THREAD_ID is required — the container is pinned to one thread.",
    );
  }
  if (!config.serverAuthSecret) {
    fastify.log.warn(
      "SERVER_AUTH_SECRET is not set — all non-healthz requests will 401",
    );
  }
  await ensureDeployKey();
  await mkdir(config.outputsDir, { recursive: true });

  // ---- Wire RxJS EventBus pipelines ----

  // 1. Vercel ingest pipeline — retries with exponential backoff.
  if (config.eventIngestUrl && config.eventIngestSecret) {
    eventBus.startVercelIngest(config.eventIngestUrl, config.eventIngestSecret);
    fastify.log.info("EventBus: Vercel ingest pipeline active");
  }

  // 2. Supabase broadcast pipeline — per-user fan-out with retry.
  if (isRealtimeEnabled()) {
    eventBus.startSupabaseBroadcast((userId, payload) =>
      publishUserEvent(userId, payload),
    );
    fastify.log.info("EventBus: Supabase broadcast pipeline active");
  }

  // 3. Log sink — tee all events to /tmp/convos/thread.log.
  eventBus.startLogSink(config.logDir);
  fastify.log.info(`EventBus: log sink active at ${config.logDir}/thread.log`);

  if (config.threadId && config.idleTimeoutMs > 0) {
    eventBus.startIdleWatchdog(config.idleTimeoutMs, () => {
      fastify.log.info(
        `Idle timeout (${config.idleTimeoutMs / 1000}s) - shutting down`,
      );
      process.kill(process.pid, "SIGTERM");
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
      .join(", ");
    const missing = list
      .filter((p) => !p.available)
      .map((p) => `${p.provider}(${p.reason ?? "?"})`)
      .join(", ");
    fastify.log.info(
      `agent providers — available: [${installed || "none"}]` +
        (missing ? ` · unavailable: [${missing}]` : ""),
    );
  });

  // Pre-warm the thread session so the first task lands on a ready workspace.
  const bootSession = getOrCreateSession({
    taskId: config.threadId,
    threadId: config.threadId,
  });
  await bootSession.ready;

  await fastify.listen({ host: config.host, port: config.port });
}

function shutdown(signal: string): void {
  fastify.log.info(`${signal} received — tearing down EventBus + channels`);
  eventBus.destroy();
  destroyChannelPool();
  fastify.close().then(
    () => process.exit(0),
    () => process.exit(1),
  );
  setTimeout(() => process.exit(1), 10_000).unref();
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));

main().catch((err) => {
  fastify.log.error(err);
  eventBus.destroy();
  process.exit(1);
});

/* eslint-enable security/detect-non-literal-fs-filename */
