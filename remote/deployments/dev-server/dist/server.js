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
// dev-thread/<threadId>/<slugified-title>.
// Per task, it runs the selected provider, streams sequenced events,
// appends tmp/convos/thread.log, and pushes the branch. PR creation is an
// explicit UI/API action.
//
// Worktrees + finished tasks are GC'd one hour after completion.
import Fastify from 'fastify';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { basename, dirname, join } from 'node:path';
import { access, appendFile, mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { ReplaySubject, Subject, interval } from 'rxjs';
import { filter, takeUntil } from 'rxjs/operators';
import { z } from 'zod';
import { EventBus } from './event-bus.js';
import { buildAgentEnv, getCachedAvailability, getRunner, probeAllProviders, resolveAgentProvider, } from './agents/index.js';
import { publishArtifact } from './storage/index.js';
import { acquireUserChannel, destroyChannelPool, isRealtimeEnabled, publishUserEvent, releaseUserChannel, } from './realtime.js';
import { initTelemetry, shutdownTelemetry, withSpan } from './telemetry.js';
import { verifyDirectStreamToken } from './token.js';
import { NatsPublisher } from './nats-publisher.js';
// ---------- Config ----------
const config = {
    port: Number(process.env.PORT ?? 8080),
    host: process.env.HOST ?? '0.0.0.0',
    workspaceRepo: process.env.WORKSPACE_REPO ?? '/home/node/workspace/repo',
    repoUrl: process.env.DD_REPO_URL ?? null,
    // The container is pinned to a single threadId originating from
    // /u/admin/remote-dev. Set via REMOTE_DEV_THREAD_ID (preferred) or
    // THREAD_ID (fallback). The server refuses to start without one — see
    // main().
    threadId: process.env.REMOTE_DEV_THREAD_ID ?? process.env.THREAD_ID ?? null,
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
    threadContextBaseUrl: process.env.THREAD_CONTEXT_BASE_URL ??
        process.env.REMOTE_REST_API_BASE_URL ??
        'http://dd-remote-rest-api.default.svc.cluster.local:8082',
    threadContextLimit: Number(process.env.THREAD_CONTEXT_LIMIT ?? 20),
    threadContextMaxChars: Number(process.env.THREAD_CONTEXT_MAX_CHARS ?? 48_000),
    agentEchoFallback: process.env.AGENT_ECHO_FALLBACK !== 'false',
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
function getSessionBranch(sessionId, branchHint, titleHint) {
    const hinted = branchHint?.trim();
    if (hinted) {
        return hinted;
    }
    const titleSlug = slugifyBranchFragment(titleHint?.trim() || sessionId);
    return `dev-thread/${sessionId}/${titleSlug}`;
}
function getSessionLogPath(workspacePath) {
    return join(workspacePath, config.threadLogRelativePath);
}
async function remoteBranchExists(branch) {
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
        await mkdir(dirname(session.logPath), { recursive: true });
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'session-ready',
            sessionId: session.sessionId,
            branch: session.branch,
            workspacePath: session.workspacePath,
            repo: config.repoUrl,
            baseBranch: config.baseBranch,
            skippedBootGitSync: true,
        }) + '\n');
        return;
    }
    await shCapture('git', ['fetch', '--quiet', 'origin', config.baseBranch], config.workspaceRepo, {
        timeoutMs: TIMEOUT_GIT_NETWORK,
    });
    const hasRemoteBranch = await remoteBranchExists(session.branch);
    if (hasRemoteBranch) {
        await shCapture('git', ['fetch', '--quiet', 'origin', session.branch], config.workspaceRepo, {
            timeoutMs: TIMEOUT_GIT_NETWORK,
        });
    }
    await shCapture('git', [
        'switch',
        '--discard-changes',
        '-C',
        session.branch,
        hasRemoteBranch ? `origin/${session.branch}` : `origin/${config.baseBranch}`,
    ], config.workspaceRepo, { timeoutMs: TIMEOUT_GIT_QUICK });
    const installResult = await installWorkspaceDependencies(session.workspacePath);
    await configureGitIdentity(session.workspacePath);
    await mkdir(dirname(session.logPath), { recursive: true });
    await appendFile(session.logPath, JSON.stringify({
        ts: new Date().toISOString(),
        kind: 'session-ready',
        sessionId: session.sessionId,
        branch: session.branch,
        workspacePath: session.workspacePath,
        repo: config.repoUrl,
        baseBranch: config.baseBranch,
        dependencyInstallOk: installResult.ok,
        dependencyInstallError: installResult.error,
    }) + '\n');
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
        }) + '\n');
    }
    catch (err) {
        if (err && typeof err === 'object' && 'code' in err && err.code === 'ENOENT') {
            return;
        }
        process.stderr.write(`[remote-dev thread-log] append failed: ${err instanceof Error ? err.message : String(err)}\n`);
    }
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
        'ANTHROPIC_API_KEY',
        'GEMINI_API_KEY',
        'GOOGLE_API_KEY',
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
function emit(state, event) {
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
    natsPublisher.publish(config.natsEventSubject, {
        type: 'task-event',
        taskId: state.taskId,
        threadId: state.threadId,
        userId: state.userId,
        provider: state.provider,
        branch: state.branch,
        seq: stored.seq,
        emittedAt: new Date().toISOString(),
        event,
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
    await shCapture('git', ['config', 'user.name', config.prAuthor.name], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
    await shCapture('git', ['config', 'user.email', config.prAuthor.email], cwd, {
        timeoutMs: TIMEOUT_GIT_QUICK,
    });
}
async function waitForBootGitReady() {
    // If the container started via entrypoint.sh, the git fetch + switch
    // runs as a background process. Wait for it to finish before we proceed.
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
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'merge-upstream-start',
            sessionId: session.sessionId,
            branch: session.branch,
            baseBranch: config.baseBranch,
        }) + '\n');
        const before = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await shCapture('git', ['fetch', '--quiet', 'origin', config.baseBranch], session.workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
        await shCapture('git', ['merge', '--no-edit', `origin/${config.baseBranch}`], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        const after = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await shCapture('git', ['push', '--no-verify', '--set-upstream', 'origin', session.branch], session.workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'merge-upstream-done',
            sessionId: session.sessionId,
            branch: session.branch,
            baseBranch: config.baseBranch,
            before,
            after,
        }) + '\n');
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
        emit(taskState, { kind: 'status', status: 'manual-commit-pushing' });
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
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'make-commit-start',
            sessionId: session.sessionId,
            branch: session.branch,
            taskId,
        }) + '\n');
        const status = await shCapture('git', ['status', '--porcelain'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
        const hasChanges = status.trim().length > 0;
        if (hasChanges) {
            await shCapture('git', ['add', '-A'], session.workspacePath, {
                timeoutMs: TIMEOUT_GIT_QUICK,
            });
            await shCapture('git', ['commit', '--no-verify', '-m', manualCommitMessage({ ...input, threadId })], session.workspacePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        }
        const after = (await shCapture('git', ['rev-parse', 'HEAD'], session.workspacePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        })).trim();
        await shCapture('git', ['push', '--no-verify', '--set-upstream', 'origin', session.branch], session.workspacePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'make-commit-done',
            sessionId: session.sessionId,
            branch: session.branch,
            taskId,
            before,
            after,
            committed: hasChanges,
        }) + '\n');
        if (taskState) {
            emit(taskState, { kind: 'status', status: hasChanges ? 'manual-commit-pushed' : 'pushed' });
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
        emit(taskState, { kind: 'status', status: 'opening-draft-pr' });
    }
    await waitForBootGitReady();
    await session.ready;
    const queuedOpen = session.queue
        .catch(() => undefined)
        .then(async () => {
        session.lastActiveAt = Date.now();
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'open-pr-start',
            sessionId: session.sessionId,
            branch: session.branch,
            baseBranch: config.baseBranch,
            taskId,
        }) + '\n');
        const result = await ensurePullRequestForSession({
            session,
            taskId,
            threadTitle: input.threadTitle ?? input.reason,
        });
        await appendFile(session.logPath, JSON.stringify({
            ts: new Date().toISOString(),
            kind: 'open-pr-done',
            sessionId: session.sessionId,
            branch: session.branch,
            baseBranch: config.baseBranch,
            taskId,
            prUrl: result.prUrl,
            draft: result.draft,
            reused: result.reused,
        }) + '\n');
        if (taskState) {
            emit(taskState, {
                kind: 'pr_open',
                branch: result.branch,
                prUrl: result.prUrl,
                draft: result.draft,
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
async function readLocalThreadContext(state) {
    try {
        const text = await readFile(state.logPath, 'utf8');
        return truncateContext(text, Math.min(config.threadContextMaxChars, 24_000));
    }
    catch {
        return '';
    }
}
async function buildPromptWithThreadContext(state) {
    if (!state.threadId) {
        return state.prompt;
    }
    const base = config.threadContextBaseUrl?.replace(/\/+$/, '');
    let contextText = '';
    let contextSource = 'none';
    if (base) {
        try {
            const response = await fetch(`${base}/api/agents/threads/${encodeURIComponent(state.threadId)}/context?limit=${config.threadContextLimit}`, { signal: AbortSignal.timeout(10_000) });
            if (response.ok) {
                const body = (await response.json());
                contextText = formatThreadContextTasks(body.tasks ?? [], state.taskId);
                contextSource = body.source ?? 'rest-api';
            }
        }
        catch (err) {
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
        emit(state, { kind: 'status', status: `agent-running:${state.provider}` });
        // Strict env allowlist owned by the runner module. Inheriting the full
        // process.env into the agent process would leak our GitHub deploy key,
        // Supabase service role key, ingest secret, etc. via any `env` or
        // `printenv` tool call. The runner adds only the API key its model
        // needs.
        const prompt = await buildPromptWithThreadContext(state);
        const runSelectedAgent = async (provider) => {
            const agentEnv = buildAgentEnv(provider);
            const runner = getRunner(provider);
            await runner.run({
                prompt,
                cwd: state.worktreePath,
                env: agentEnv,
                signal: state.abortController.signal,
                timeoutMs: config.agentRunTimeoutMs,
                emit: (ev) => emit(state, ev),
                setChild: (child) => {
                    state.child = child;
                },
            });
        };
        try {
            await runSelectedAgent(state.provider);
        }
        catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            emit(state, {
                kind: 'error',
                message: `${state.provider} failed: ${message}`,
            });
            if (!config.agentEchoFallback ||
                state.provider === 'echo' ||
                state.cancelled ||
                state.abortController.signal.aborted) {
                throw err;
            }
            emit(state, { kind: 'status', status: 'agent-fallback:echo' });
            await runSelectedAgent('echo');
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
        emit(state, { kind: 'status', status: 'pushing' });
        const status = await shCapture('git', ['status', '--porcelain'], state.worktreePath, {
            timeoutMs: TIMEOUT_GIT_QUICK,
        });
        if (status.trim()) {
            await shCapture('git', ['add', '-A'], state.worktreePath, {
                timeoutMs: TIMEOUT_GIT_QUICK,
            });
            await shCapture('git', ['commit', '--no-verify', '-m', `agent(${state.session.sessionId}): ${state.taskId}`], state.worktreePath, { timeoutMs: TIMEOUT_GIT_QUICK });
        }
        await shCapture('git', ['push', '--no-verify', '--set-upstream', 'origin', state.branch], state.worktreePath, { timeoutMs: TIMEOUT_GIT_NETWORK });
        emit(state, { kind: 'status', status: 'pushed' });
        // Publish any files the agent dropped in the per-task outputs dir.
        // Failures uploading individual files are surfaced as `error` events
        // but do not fail the whole task.
        await publishOutputs(state, taskOutputsDir);
        emit(state, {
            kind: 'done',
            branch: state.branch,
            exitReason: 'completed',
        });
    });
}
async function ensurePullRequestForSession(input) {
    const ghEnv = config.ghPat ? { GH_TOKEN: config.ghPat } : undefined;
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
const fastify = Fastify({ logger: true });
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
        ...renderGauge('dd_runtime_inflight_tasks', 'Tasks that are currently not finished.', Array.from(tasks.values()).filter((t) => !t.finished).length),
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
  <style>
    :root { color-scheme: dark; --bg: #0d1117; --panel: #111827; --line: #263244; --text: #e5edf7; --muted: #9aa7b7; --accent: #7dd3fc; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100dvh; background: var(--bg); color: var(--text); font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    main { min-height: 100dvh; display: grid; grid-template-rows: auto minmax(0, 1fr) auto; }
    header { display: flex; justify-content: space-between; gap: 12px; align-items: center; padding: 12px 14px; border-bottom: 1px solid var(--line); background: #0f172a; }
    h1 { margin: 0; font-size: 16px; font-weight: 650; }
    #status { color: var(--muted); font-size: 13px; }
    #output { width: 100%; height: 100%; min-height: 0; resize: none; border: 0; outline: 0; padding: 14px; background: #05080d; color: #d5f5e3; font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; white-space: pre-wrap; }
    form { display: flex; gap: 8px; padding: 10px; border-top: 1px solid var(--line); background: var(--panel); }
    input { flex: 1 1 auto; min-width: 0; border: 1px solid var(--line); border-radius: 6px; padding: 9px 10px; background: #0b1220; color: var(--text); font: 13px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    button { border: 1px solid #31536d; border-radius: 6px; padding: 9px 12px; background: #12324a; color: var(--text); font-weight: 650; cursor: pointer; }
    button:disabled, input:disabled { opacity: 0.55; cursor: not-allowed; }
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Thread terminal</h1>
      <span id="status">connecting</span>
    </header>
    <textarea id="output" spellcheck="false" readonly></textarea>
    <form id="command-form">
      <input id="command" autocomplete="off" spellcheck="false" placeholder="command" disabled>
      <button id="send" type="submit" disabled>Run</button>
    </form>
  </main>
  <script>
    const threadId = ${encodedThreadId};
    const statusNode = document.getElementById("status");
    const output = document.getElementById("output");
    const form = document.getElementById("command-form");
    const command = document.getElementById("command");
    const send = document.getElementById("send");
    let socket;
    function append(value) {
      output.value += value;
      output.scrollTop = output.scrollHeight;
    }
    function connect() {
      const url = new URL("terminal/ws", window.location.href);
      url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      url.searchParams.set("threadId", threadId);
      socket = new WebSocket(url);
      socket.addEventListener("open", () => {
        statusNode.textContent = "connected";
        command.disabled = false;
        send.disabled = false;
        command.focus();
      });
      socket.addEventListener("message", (event) => {
        let message;
        try {
          message = JSON.parse(event.data);
        } catch {
          append(String(event.data));
          return;
        }
        if (message.type === "terminal-output") append(String(message.data || ""));
        if (message.type === "terminal-status") statusNode.textContent = String(message.status || "status");
        if (message.type === "terminal-error") {
          statusNode.textContent = "error";
          append("\\n" + String(message.message || "terminal error") + "\\n");
        }
        if (message.type === "terminal-exit") {
          statusNode.textContent = "closed";
          command.disabled = true;
          send.disabled = true;
        }
      });
      socket.addEventListener("close", () => {
        statusNode.textContent = "closed";
        command.disabled = true;
        send.disabled = true;
      });
      socket.addEventListener("error", () => {
        statusNode.textContent = "connection error";
      });
    }
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      if (!socket || socket.readyState !== WebSocket.OPEN) return;
      const value = command.value;
      command.value = "";
      socket.send(JSON.stringify({ type: "input", data: value + "\\n" }));
    });
    connect();
  </script>
</body>
</html>`;
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
            this.child = spawn(shell, ['-i'], {
                cwd: session.workspacePath,
                env: {
                    ...process.env,
                    SHELL: shell,
                    TERM: process.env.TERM || 'xterm-256color',
                    PS1: '\\w $ ',
                },
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
    repo: z.string().min(1).max(2048).optional(),
    baseBranch: z.string().min(1).max(120).optional(),
    /** Stable remote-dev thread branch, if the dispatcher already knows it. */
    branch: z.string().max(200).optional(),
    /** Human-readable thread title / branch slug fallback. */
    threadTitle: z.string().min(1).max(200).optional(),
    /**
     * Which agent runner to drive the task. Falls back to AGENT_PROVIDER env
     * then "claude-sdk". Validated by the selector — unknown values fall
     * back to default rather than 400ing.
     */
    provider: z
        .enum(['claude-cli', 'claude-sdk', 'echo', 'gemini-sdk', 'openai-codex-cli', 'openai-sdk'])
        .optional(),
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
            branch: parsed.data.branch,
            threadTitle: parsed.data.threadTitle,
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
        return { taskId, branch: state.branch };
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
    return '0.0.0.0';
}
async function main() {
    initTelemetry();
    if (!config.threadId) {
        throw new Error('REMOTE_DEV_THREAD_ID or THREAD_ID is required — the container is pinned to one thread.');
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
    // Pre-warm the thread session so the first task lands on a ready workspace.
    const bootSession = getOrCreateSession({
        taskId: config.threadId,
        threadId: config.threadId,
    });
    await bootSession.ready;
    registerWorkerWebSocketUpgrade();
    await fastify.listen({ host: config.host, port: config.port });
}
function shutdown(signal) {
    fastify.log.info(`${signal} received — tearing down EventBus + channels`);
    natsPublisher.destroy();
    eventBus.destroy();
    destroyChannelPool();
    fastify.close().then(() => shutdownTelemetry().finally(() => process.exit(0)), () => process.exit(1));
    setTimeout(() => process.exit(1), 10_000).unref();
}
process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));
main().catch((err) => {
    fastify.log.error(err);
    natsPublisher.destroy();
    eventBus.destroy();
    shutdownTelemetry().finally(() => process.exit(1));
});
/* eslint-enable security/detect-non-literal-fs-filename */
//# sourceMappingURL=server.js.map