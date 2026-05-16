/* eslint-disable security/detect-non-literal-fs-filename -- remote-dev manages configured workspace, log, and artifact paths. */
// dd-dev-server — HTTP server that runs Claude/OpenAI coding agents inside
// a warm dd-next-1 git workspace, then streams events.
//
// Endpoints (all auth'd via X-Server-Auth header except /healthz):
//   POST /tasks                  — { taskId?, threadId?, prompt } → queues task
//   GET  /stream/:taskId         — Server-Sent Events of agent activity
//   POST /tasks/:taskId/cancel   — abort an in-flight task
//   GET  /healthz                — liveness probe
//
// Per thread, the server prepares/reuses branch agent/thread/<threadId>.
// Per task, it runs the selected provider, streams sequenced events,
// appends tmp/convos/thread.log, pushes the branch, and creates/reuses a PR.
//
// Worktrees + finished tasks are GC'd one hour after completion.
import Fastify from "fastify";
import { spawn } from "node:child_process";
import { access, appendFile, mkdir, readdir, stat, writeFile, } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { randomUUID } from "node:crypto";
import { ReplaySubject, Subject, interval } from "rxjs";
import { filter, takeUntil } from "rxjs/operators";
import { z } from "zod";
import { EventBus } from "./event-bus.js";
import { buildAgentEnv, getCachedAvailability, getRunner, probeAllProviders, resolveAgentProvider, } from "./agents/index.js";
import { publishArtifact } from "./storage/index.js";
import { acquireUserChannel, destroyChannelPool, isRealtimeEnabled, publishUserEvent, releaseUserChannel, } from "./realtime.js";
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
        "local"),
    // Periodic heartbeat to Vercel — lets the UI poll an "is the docker
    // alive?" endpoint backed by our most recent ping. Disabled if
    // HEARTBEAT_URL is unset.
    heartbeatUrl: process.env.HEARTBEAT_URL ?? null,
    heartbeatSecret: process.env.HEARTBEAT_SECRET ?? null,
    heartbeatIntervalMs: Number(process.env.HEARTBEAT_INTERVAL_MS ?? 20_000),
    idleTimeoutMs: Number(process.env.IDLE_TIMEOUT_MS ?? 30 * 60 * 1000),
    agentRunTimeoutMs: Number(process.env.AGENT_RUN_TIMEOUT_MS ?? 2 * 60 * 60_000),
    ghDeployKeyPath: process.env.GH_DEPLOY_KEY_PATH ?? "/home/agent/.ssh/id_ed25519",
    ghDeployKey: process.env.GH_DEPLOY_KEY ?? null,
    ghPat: process.env.GH_PAT ?? null,
    anthropicApiKey: process.env.ANTHROPIC_API_KEY ?? null,
    serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
    eventIngestUrl: process.env.EVENT_INGEST_URL ?? null,
    eventIngestSecret: process.env.EVENT_INGEST_SECRET ?? null,
    baseBranch: process.env.BASE_BRANCH ?? "dev",
    threadLogRelativePath: process.env.THREAD_LOG_RELATIVE_PATH ?? "tmp/convos/thread.log",
    sessionIdleGcAfterMs: Number(process.env.SESSION_IDLE_GC_AFTER_MS ?? 6 * 60 * 60 * 1000),
    prAuthor: {
        name: process.env.GIT_AUTHOR_NAME ?? "DD Agent",
        email: process.env.GIT_AUTHOR_EMAIL ?? "agent@dancingdragons.dev",
    },
    logDir: process.env.LOG_DIR ?? "/tmp/convos",
    taskGcAfterMs: 60 * 60 * 1000, // 1 hour
    taskGcIntervalMs: 5 * 60 * 1000, // 5 min sweep
};
const tasks = new Map();
const sessions = new Map();
const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();
// ---- RxJS EventBus — reactive pipeline for events ----
const eventBus = new EventBus();
// ---------- Helpers ----------
function escapeHtml(value) {
    return value
        .replaceAll("&", "&amp;")
        .replaceAll("<", "&lt;")
        .replaceAll(">", "&gt;")
        .replaceAll('"', "&quot;");
}
function renderHomePage() {
    const uptimeMs = Math.max(0, Date.now() - Date.parse(serverStartedAt));
    const uptimeMinutes = Math.floor(uptimeMs / 60_000);
    const uptimeSeconds = Math.floor((uptimeMs % 60_000) / 1000);
    const liveHealth = {
        serverInstanceId,
        serverStartedAt,
        uptime: `${uptimeMinutes}m ${uptimeSeconds}s`,
        threadId: config.threadId ?? "unset",
        userId: config.userId ?? "unset",
        workspaceRepo: config.workspaceRepo,
        baseBranch: config.baseBranch,
        serverAuth: config.serverAuthSecret ? "configured" : "open",
        ingest: config.eventIngestUrl ? "configured" : "off",
        heartbeat: config.heartbeatUrl ? "configured" : "off",
        realtime: isRealtimeEnabled() ? "on" : "off",
    };
    const rows = Object.entries(liveHealth)
        .map(([label, value]) => `
        <div class="row">
          <span class="label">${escapeHtml(label)}</span>
          <span class="value">${escapeHtml(String(value))}</span>
        </div>`)
        .join("");
    return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta http-equiv="x-ua-compatible" content="ie=edge" />
    <title>dd-dev-server</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #081018;
        --panel: #101827;
        --panel-2: #152033;
        --text: #e7edf7;
        --muted: #94a3b8;
        --line: rgba(148, 163, 184, 0.18);
        --accent: #60a5fa;
        --accent-2: #34d399;
      }
      * { box-sizing: border-box; }
      html, body {
        margin: 0;
        min-height: 100%;
        background: linear-gradient(180deg, #081018 0%, #0b1220 100%);
        color: var(--text);
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      }
      body { padding: 32px; }
      .shell { max-width: 1100px; margin: 0 auto; }
      .topline {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        margin-bottom: 20px;
      }
      h1 {
        margin: 0;
        font-size: 28px;
        line-height: 1.1;
        letter-spacing: 0;
      }
      .sub {
        margin-top: 8px;
        color: var(--muted);
        max-width: 72ch;
        line-height: 1.5;
      }
      .pillbar {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
      }
      .pill,
      .button {
        border: 1px solid var(--line);
        background: rgba(16, 24, 39, 0.9);
        color: var(--text);
        border-radius: 8px;
        padding: 10px 12px;
        text-decoration: none;
        font-size: 14px;
        line-height: 1;
      }
      .button {
        display: inline-flex;
        align-items: center;
        gap: 8px;
      }
      .button:hover,
      .pill:hover {
        border-color: rgba(96, 165, 250, 0.6);
        color: #fff;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 16px;
        margin-top: 24px;
      }
      .panel {
        border: 1px solid var(--line);
        background: rgba(16, 24, 39, 0.92);
        border-radius: 8px;
        padding: 16px;
      }
      .panel h2 {
        margin: 0 0 12px;
        font-size: 16px;
      }
      .rows {
        display: grid;
        gap: 10px;
      }
      .row {
        display: flex;
        justify-content: space-between;
        gap: 16px;
        padding-bottom: 10px;
        border-bottom: 1px solid rgba(148, 163, 184, 0.12);
      }
      .row:last-child {
        border-bottom: 0;
        padding-bottom: 0;
      }
      .label {
        color: var(--muted);
        text-transform: uppercase;
        font-size: 11px;
        letter-spacing: 0.08em;
      }
      .value {
        text-align: right;
        word-break: break-word;
      }
      .controls {
        display: flex;
        flex-wrap: wrap;
        gap: 10px;
      }
      .note {
        margin-top: 12px;
        color: var(--muted);
        line-height: 1.45;
      }
      @media (max-width: 860px) {
        body { padding: 16px; }
        .topline { display: block; }
        .grid { grid-template-columns: 1fr; }
        .row { flex-direction: column; }
        .value { text-align: left; }
      }
    </style>
  </head>
  <body>
    <main class="shell">
      <div class="topline">
        <div>
          <h1>dd-dev-server</h1>
          <div class="sub">
            Node.js + TypeScript task runner for remote dev work. The public
            landing page lives here; task dispatch, streaming, and agent runs
            stay behind the authenticated API.
          </div>
        </div>
        <div class="pillbar">
          <span class="pill">online</span>
          <span class="pill">port ${escapeHtml(String(config.port))}</span>
          <span class="pill">instance ${escapeHtml(serverInstanceId.slice(0, 8))}</span>
        </div>
      </div>

      <div class="panel">
        <h2>Controls</h2>
        <div class="controls">
          <a class="button" href="/healthz">Health</a>
          <a class="button" href="/status">Status</a>
          <a class="button" href="/agents">Agents</a>
          <a class="button" href="/tasks">Tasks</a>
        </div>
        <div class="note">
          GET / redirects here. /healthz is public; the task and snapshot
          routes still require the configured server auth header.
        </div>
      </div>

      <div class="grid">
        <section class="panel">
          <h2>Runtime</h2>
          <div class="rows">${rows}</div>
        </section>
        <section class="panel">
          <h2>What this node exposes</h2>
          <div class="rows">
            <div class="row">
              <span class="label">public</span>
              <span class="value">/, /home, /healthz</span>
            </div>
            <div class="row">
              <span class="label">authenticated</span>
              <span class="value">/status, /agents, /tasks, /stream/:taskId</span>
            </div>
            <div class="row">
              <span class="label">workspace</span>
              <span class="value">${escapeHtml(config.workspaceRepo)}</span>
            </div>
            <div class="row">
              <span class="label">branch</span>
              <span class="value">${escapeHtml(config.baseBranch)}</span>
            </div>
          </div>
        </section>
      </div>
    </main>
    <script>
      const livePills = Array.from(document.querySelectorAll(".pill"));
      async function refreshHealth() {
        try {
          const response = await fetch("/healthz", { cache: "no-store" });
          const payload = await response.json();
          const isOk = Boolean(payload && payload.ok);
          livePills[0].textContent = isOk ? "online" : "degraded";
          livePills[2].textContent = "instance " + String(payload.serverInstanceId || "").slice(0, 8);
        } catch {
          livePills[0].textContent = "offline";
        }
      }
      refreshHealth();
      setInterval(refreshHealth, 15000);
    </script>
  </body>
</html>`;
}
function shCapture(cmd, args, cwd, optsOrExtraEnv) {
    // Backwards-compat: callers passing a plain extraEnv object still work.
    const opts = optsOrExtraEnv && typeof optsOrExtraEnv === "object" &&
        ("timeoutMs" in optsOrExtraEnv ||
            "extraEnv" in optsOrExtraEnv ||
            "isolatedEnv" in optsOrExtraEnv)
        ? optsOrExtraEnv
        : { extraEnv: optsOrExtraEnv };
    const env = opts.isolatedEnv
        ? opts.isolatedEnv
        : { ...process.env, ...(opts.extraEnv ?? {}) };
    return new Promise((resolve, reject) => {
        const child = spawn(cmd, args, { cwd, env });
        let stdout = "";
        let stderr = "";
        let timedOut = false;
        let killTimer = null;
        if (opts.timeoutMs && opts.timeoutMs > 0) {
            killTimer = setTimeout(() => {
                timedOut = true;
                try {
                    child.kill("SIGKILL");
                }
                catch {
                    /* ignore */
                }
            }, opts.timeoutMs);
        }
        child.stdout.on("data", (d) => {
            stdout += d.toString("utf8");
        });
        child.stderr.on("data", (d) => {
            stderr += d.toString("utf8");
        });
        child.on("close", (code) => {
            if (killTimer) {
                clearTimeout(killTimer);
            }
            if (timedOut) {
                reject(new Error(`${cmd} ${args.join(" ")} timed out after ${opts.timeoutMs}ms`));
                return;
            }
            if (code === 0) {
                resolve(stdout);
            }
            else {
                reject(new Error(`${cmd} ${args.join(" ")} exited ${code}: ${stderr.slice(0, 1000)}`));
            }
        });
        child.on("error", (err) => {
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
function getSessionBranch(sessionId) {
    return `agent/thread/${sessionId}`;
}
function getSessionLogPath(workspacePath) {
    return join(workspacePath, config.threadLogRelativePath);
}
async function remoteBranchExists(branch) {
    const out = await shCapture("git", ["ls-remote", "--heads", "origin", branch], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_NETWORK });
    return out.trim().length > 0;
}
async function prepareSessionWorkspace(session) {
    if (config.threadId && session.sessionId !== config.threadId) {
        throw new Error(`container is pinned to thread ${config.threadId}, got ${session.sessionId}`);
    }
    await shCapture("git", ["fetch", "--quiet", "origin", config.baseBranch], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_NETWORK });
    const hasRemoteBranch = await remoteBranchExists(session.branch);
    if (hasRemoteBranch) {
        await shCapture("git", ["fetch", "--quiet", "origin", session.branch], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_NETWORK });
    }
    await shCapture("git", [
        "switch",
        "--discard-changes",
        "-C",
        session.branch,
        hasRemoteBranch
            ? `origin/${session.branch}`
            : `origin/${config.baseBranch}`,
    ], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_QUICK });
    await shCapture("pnpm", ["install", "--frozen-lockfile"], session.workspacePath, { timeoutMs: TIMEOUT_PNPM_INSTALL });
    await configureGitIdentity(session.workspacePath);
    await mkdir(dirname(session.logPath), { recursive: true });
    await appendFile(session.logPath, JSON.stringify({
        ts: new Date().toISOString(),
        kind: "session-ready",
        sessionId: session.sessionId,
        branch: session.branch,
        workspacePath: session.workspacePath,
        baseBranch: config.baseBranch,
    }) + "\n");
}
function getOrCreateSession(input) {
    const sessionId = getSessionId(input.threadId, input.taskId);
    const existing = sessions.get(sessionId);
    if (existing) {
        existing.lastActiveAt = Date.now();
        if (!existing.userId && input.userId) {
            existing.userId = input.userId;
        }
        return existing;
    }
    const workspacePath = getSessionWorkspacePath(sessionId);
    const session = {
        sessionId,
        userId: input.userId,
        workspacePath,
        branch: getSessionBranch(sessionId),
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
async function appendThreadLog(state, payload) {
    try {
        await access(state.worktreePath);
        await mkdir(dirname(state.logPath), { recursive: true });
        await appendFile(state.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            taskId: state.taskId,
            threadId: state.threadId,
            provider: state.provider,
            ...payload,
        }) + "\n");
    }
    catch (err) {
        if (err &&
            typeof err === "object" &&
            "code" in err &&
            err.code === "ENOENT") {
            return;
        }
        process.stderr.write(`[remote-dev thread-log] append failed: ${err instanceof Error ? err.message : String(err)}\n`);
    }
}
function emit(state, event) {
    // Once a task has emitted `done`, it's terminal — late events from a
    // race (e.g. cancel firing while claude was already exiting cleanly)
    // would corrupt the seq stream and confuse downstream consumers.
    if (state.finished && event.kind !== "done") {
        return;
    }
    if (state.finished && event.kind === "done") {
        return; // dedupe duplicate done emits (cancel + natural close race)
    }
    const stored = { seq: state.events.length, event };
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
        event: event,
    });
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
async function configureGitIdentity(cwd) {
    await shCapture("git", ["config", "user.name", config.prAuthor.name], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture("git", ["config", "user.email", config.prAuthor.email], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
}
// ---------- Per-task workflow ----------
async function runTask(state) {
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
            await new Promise((resolve) => {
                const check = () => {
                    try {
                        process.kill(Number(gitReadyPid), 0); // signal 0 = existence check
                        setTimeout(check, 500);
                    }
                    catch {
                        resolve(); // process gone — git is done
                    }
                };
                check();
            });
            // Clear so subsequent tasks don't re-wait.
            delete process.env.GIT_READY_PID;
        }
        catch {
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
        emit: (ev) => emit(state, ev),
        setChild: (child) => {
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
    emit(state, { kind: "status", status: "pushing" });
    const status = await shCapture("git", ["status", "--porcelain"], state.worktreePath, { timeoutMs: TIMEOUT_GIT_QUICK });
    if (status.trim()) {
        await shCapture("git", ["add", "-A"], state.worktreePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
        await shCapture("git", ["commit", "-m", `agent(${state.session.sessionId}): ${state.taskId}`], state.worktreePath, { timeoutMs: TIMEOUT_GIT_QUICK });
    }
    await shCapture("git", ["push", "--set-upstream", "origin", state.branch], state.worktreePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
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
async function ensurePullRequest(state) {
    const ghEnv = config.ghPat ? { GH_TOKEN: config.ghPat } : undefined;
    try {
        const existing = await shCapture("gh", ["pr", "view", state.branch, "--json", "url", "--jq", ".url"], state.worktreePath, { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv });
        const url = existing.trim();
        if (url) {
            return url;
        }
    }
    catch {
        /* no existing PR */
    }
    const titleSnippet = state.prompt
        .slice(0, 60)
        .replace(/\s+/g, " ")
        .trim();
    try {
        const out = await shCapture("gh", [
            "pr",
            "create",
            "--base",
            config.baseBranch,
            "--head",
            state.branch,
            "--title",
            `agent/thread/${state.session.sessionId.slice(0, 8)}: ${titleSnippet}`,
            "--body",
            `**Thread**\n\n${state.session.sessionId}\n\n**Prompt**\n\n${state.prompt}\n\n_Opened by dd-dev-server._`,
        ], state.worktreePath, { timeoutMs: TIMEOUT_GH_PR, extraEnv: ghEnv });
        return out
            .trim()
            .split("\n")
            .map((l) => l.trim())
            .filter(Boolean)
            .pop();
    }
    catch (err) {
        emit(state, {
            kind: "error",
            message: `gh pr create/view failed: ${err.message}`,
        });
        return undefined;
    }
}
/**
 * Walk the per-task outputs/ directory, publish every regular file via
 * the configured storage adapter, and emit an `artifact` event for each.
 */
async function publishOutputs(state, taskOutputsDir) {
    let dirents;
    try {
        dirents = (await readdir(taskOutputsDir, {
            withFileTypes: true,
        }));
    }
    catch {
        return; // dir absent / unreadable → nothing to publish, that's fine
    }
    if (dirents.length === 0) {
        return;
    }
    emit(state, { kind: "status", status: "publishing-artifacts" });
    // Recurse one level so flat-or-nested layouts both work.
    const filesToPublish = [];
    for (const e of dirents) {
        if (e.isFile()) {
            filesToPublish.push(join(taskOutputsDir, e.name));
        }
        else if (e.isDirectory()) {
            try {
                const sub = (await readdir(join(taskOutputsDir, e.name), {
                    withFileTypes: true,
                }));
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
            emit(state, { kind: "artifact", artifact: published });
        }
        catch (err) {
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
    const requestPath = req.url.split("?")[0] ?? req.url;
    if (requestPath === "/healthz" ||
        requestPath === "/" ||
        requestPath === "/home" ||
        requestPath === "/home/" ||
        requestPath === "/favicon.ico") {
        return;
    }
    // GET /stream/:taskId may auth via short-lived HMAC token (?token=)
    // for direct browser → docker SSE connections that bypass Vercel's
    // 800s function cap. Defer that check to the route handler.
    if (req.method === "GET" && requestPath.startsWith("/stream/")) {
        return;
    }
    if (!config.serverAuthSecret ||
        req.headers["x-server-auth"] !== config.serverAuthSecret) {
        return reply.code(401).send({ error: "unauthorized" });
    }
});
fastify.get("/", async (_req, reply) => {
    return reply.redirect("/home", 302);
});
fastify.get("/home", async (_req, reply) => {
    reply.type("text/html; charset=utf-8");
    return renderHomePage();
});
fastify.get("/home/", async (_req, reply) => {
    return reply.redirect("/home", 302);
});
fastify.get("/favicon.ico", async (_req, reply) => {
    return reply.code(204).send();
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
        lastSeq: t.events.length > 0 ? t.events[t.events.length - 1].seq : -1,
    }));
    return { tasks: snapshot, serverStartedAt };
});
const DispatchSchema = z.object({
    taskId: z.string().uuid().optional(),
    prompt: z.string().min(1).max(64_000),
    /** dd-user UUID. When present, events also fan out via Supabase Realtime. */
    userId: z.string().uuid().optional(),
    /** Vercel-side thread id, included in published events for client routing. */
    threadId: z.string().uuid().optional(),
    /**
     * Which agent runner to drive the task. Falls back to AGENT_PROVIDER env
     * then "claude-cli". Validated by the selector — unknown values fall
     * back to default rather than 400ing.
     */
    provider: z
        .enum(["claude-cli", "claude-sdk", "openai-codex-cli", "openai-sdk"])
        .optional(),
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
    });
    session.taskIds.add(taskId);
    const state = {
        taskId,
        prompt,
        userId,
        threadId,
        provider: resolveAgentProvider(parsed.data.provider),
        session,
        abortController: new AbortController(),
        events: [],
        event$: new ReplaySubject(),
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
    queuedRun.catch((err) => {
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
    return { taskId };
});
fastify.get("/stream/:taskId", (req, reply) => {
    const { taskId } = req.params;
    // Auth: either X-Server-Auth (server-to-server, e.g. Vercel proxy) or
    // a short-lived HMAC token in ?token= for direct browser connections.
    // For direct-browser tokens we ALSO require the token's userId to match
    // the task's owner — otherwise a valid token for task A could be
    // weaponised against task B if its taskId leaked.
    const tokenParam = req.query.token;
    let tokenAuthed = false;
    if (typeof tokenParam === "string" && tokenParam.length > 0) {
        const payload = verifyDirectStreamToken(tokenParam);
        const candidate = tasks.get(taskId);
        if (!payload ||
            payload.taskId !== taskId ||
            !candidate ||
            candidate.userId !== payload.userId) {
            reply.code(401).send({ error: "unauthorized" });
            return;
        }
        tokenAuthed = true;
    }
    if (!tokenAuthed &&
        (!config.serverAuthSecret ||
            req.headers["x-server-auth"] !== config.serverAuthSecret)) {
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
    const resumeFromIdParam = req.query.resumeFromId;
    const lastEventIdRaw = typeof lastEventIdHeader === "string"
        ? lastEventIdHeader
        : resumeFromIdParam;
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
    const { taskId } = req.params;
    const state = tasks.get(taskId);
    if (!state) {
        return reply.code(404).send({ error: "not found" });
    }
    if (state.finished) {
        return reply.code(409).send({ error: "already finished" });
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
        state.child.kill("SIGTERM");
    }
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
        if (state.finished &&
            state.finishedAt !== undefined &&
            now - state.finishedAt > config.taskGcAfterMs) {
            tasks.delete(id);
            state.session.taskIds.delete(id);
            eventBus.gcTask(id);
        }
    }
    for (const [sessionId, session] of sessions) {
        if (session.taskIds.size === 0 &&
            now - session.lastActiveAt > config.sessionIdleGcAfterMs) {
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
    const inFlight = Array.from(tasks.values()).map((t) => ({
        taskId: t.taskId,
        threadId: t.threadId,
        userId: t.userId,
        branch: t.branch,
        finished: t.finished,
        finishedAt: t.finishedAt,
        eventCount: t.events.length,
        lastSeq: t.events.length > 0 ? t.events[t.events.length - 1].seq : -1,
    }));
    const containerInfo = config.threadId
        ? {
            threadId: config.threadId,
            ip: cachedOwnIp,
            port: config.port,
            status: "ready",
            podName: process.env.POD_NAME ?? process.env.HOSTNAME ?? serverInstanceId,
            namespace: process.env.POD_NAMESPACE ?? process.env.K8S_NAMESPACE ?? "",
            orchestrator: process.env.K8S_API_SERVER
                ? "k8s"
                : process.env.ECS_CONTAINER_METADATA_URI_V4
                    ? "ecs"
                    : "docker-compose",
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
            const res = await fetch(`${ecsMetaUri}/task`);
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
    return "0.0.0.0";
}
async function main() {
    if (!config.threadId) {
        throw new Error("REMOTE_DEV_THREAD_ID or THREAD_ID is required — the container is pinned to one thread.");
    }
    if (!config.serverAuthSecret) {
        fastify.log.warn("SERVER_AUTH_SECRET is not set — all non-healthz requests will 401");
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
        eventBus.startSupabaseBroadcast((userId, payload) => publishUserEvent(userId, payload));
        fastify.log.info("EventBus: Supabase broadcast pipeline active");
    }
    // 3. Log sink — tee all events to /tmp/convos/thread.log.
    eventBus.startLogSink(config.logDir);
    fastify.log.info(`EventBus: log sink active at ${config.logDir}/thread.log`);
    if (config.threadId && config.idleTimeoutMs > 0) {
        eventBus.startIdleWatchdog(config.idleTimeoutMs, () => {
            fastify.log.info(`Idle timeout (${config.idleTimeoutMs / 1000}s) - shutting down`);
            process.kill(process.pid, "SIGTERM");
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
            .join(", ");
        const missing = list
            .filter((p) => !p.available)
            .map((p) => `${p.provider}(${p.reason ?? "?"})`)
            .join(", ");
        fastify.log.info(`agent providers — available: [${installed || "none"}]` +
            (missing ? ` · unavailable: [${missing}]` : ""));
    });
    // Pre-warm the thread session so the first task lands on a ready workspace.
    const bootSession = getOrCreateSession({
        taskId: config.threadId,
        threadId: config.threadId,
    });
    await bootSession.ready;
    await fastify.listen({ host: config.host, port: config.port });
}
function shutdown(signal) {
    fastify.log.info(`${signal} received — tearing down EventBus + channels`);
    eventBus.destroy();
    destroyChannelPool();
    fastify.close().then(() => process.exit(0), () => process.exit(1));
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
//# sourceMappingURL=server.js.map