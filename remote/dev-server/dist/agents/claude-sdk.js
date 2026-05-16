// Claude Agent SDK runner - uses @anthropic-ai/claude-agent-sdk to drive
// Claude Code in process instead of shelling out to the `claude` CLI.
//
// Advantages over claude-cli:
//   * structured events out of the box (no NDJSON parsing)
//   * fine-grained control over allowedTools / permissionMode
//   * easier hooks for tool-call telemetry
//   * programmatic abort via the SDK rather than SIGTERM on a child
//
// The SDK is the preferred Claude path when available: it streams
// structured messages, exposes session/interrupt hooks, and lets us keep
// permissions in config instead of relying on CLI parsing.
import { accessSync, constants } from 'node:fs';
import { createRequire } from 'node:module';
const localRequire = createRequire(import.meta.url);
function isRunnableExecutable(executable) {
    try {
        accessSync(executable, constants.X_OK);
        return true;
    }
    catch {
        return false;
    }
}
function resolveSdkNativeExecutable(packageNames) {
    let sdkRequire;
    try {
        sdkRequire = createRequire(localRequire.resolve('@anthropic-ai/claude-agent-sdk'));
    }
    catch {
        return undefined;
    }
    for (const packageName of packageNames) {
        try {
            const executable = sdkRequire.resolve(`${packageName}/claude`);
            if (isRunnableExecutable(executable)) {
                return executable;
            }
        }
        catch {
            /* try the next platform package */
        }
    }
    return undefined;
}
export function resolveClaudeCodeExecutable() {
    if (process.env.CLAUDE_CODE_EXECUTABLE) {
        return isRunnableExecutable(process.env.CLAUDE_CODE_EXECUTABLE)
            ? process.env.CLAUDE_CODE_EXECUTABLE
            : undefined;
    }
    if (process.platform === 'linux' && process.arch === 'x64') {
        return resolveSdkNativeExecutable([
            '@anthropic-ai/claude-agent-sdk-linux-x64',
            '@anthropic-ai/claude-agent-sdk-linux-x64-musl',
        ]);
    }
    if (process.platform === 'linux' && process.arch === 'arm64') {
        return resolveSdkNativeExecutable([
            '@anthropic-ai/claude-agent-sdk-linux-arm64',
            '@anthropic-ai/claude-agent-sdk-linux-arm64-musl',
        ]);
    }
    return undefined;
}
function resolveClaudePermissionMode() {
    if (process.env.CLAUDE_CODE_PERMISSION_MODE) {
        return process.env.CLAUDE_CODE_PERMISSION_MODE;
    }
    return typeof process.getuid === 'function' && process.getuid() === 0
        ? 'default'
        : 'bypassPermissions';
}
export const claudeSdkRunner = {
    id: 'claude-sdk',
    displayName: 'Claude Agent SDK',
    async run(opts) {
        if (!opts.env.ANTHROPIC_API_KEY) {
            throw new Error('claude-sdk requires ANTHROPIC_API_KEY in the env allowlist');
        }
        let sdk;
        try {
            sdk = (await import('@anthropic-ai/claude-agent-sdk'));
        }
        catch (err) {
            throw new Error('claude-sdk runner: @anthropic-ai/claude-agent-sdk cannot be imported.\n' +
                'Run `pnpm add @anthropic-ai/claude-agent-sdk` and restart the container.\n' +
                `Import error: ${err instanceof Error ? err.message : String(err)}`);
        }
        const abortController = new AbortController();
        const onAbort = () => abortController.abort();
        opts.signal?.addEventListener('abort', onAbort);
        let killTimer = null;
        if (opts.timeoutMs && opts.timeoutMs > 0) {
            killTimer = setTimeout(() => {
                abortController.abort();
                opts.emit({
                    kind: 'error',
                    message: `claude-sdk timed out after ${opts.timeoutMs}ms`,
                });
            }, opts.timeoutMs);
        }
        const claudeExecutable = resolveClaudeCodeExecutable();
        const query = sdk.query({
            prompt: opts.prompt,
            abortController,
            options: {
                cwd: opts.cwd,
                env: opts.env,
                ...(claudeExecutable ? { pathToClaudeCodeExecutable: claudeExecutable } : {}),
                model: opts.env.ANTHROPIC_MODEL,
                stderr: (text) => {
                    opts.emit({ kind: 'stderr', text });
                },
                maxTurns: Number(process.env.CLAUDE_CODE_MAX_TURNS ?? 50),
                permissionMode: resolveClaudePermissionMode(),
                settingSources: ['project'],
                systemPrompt: { type: 'preset', preset: 'claude_code' },
                allowedTools: [
                    'Read',
                    'Write',
                    'Edit',
                    'MultiEdit',
                    'Bash',
                    'Glob',
                    'Grep',
                    'LS',
                    'TodoWrite',
                ],
            },
        });
        try {
            for await (const ev of query) {
                opts.emit({ kind: 'claude', raw: ev });
                if (opts.signal?.aborted || abortController.signal.aborted) {
                    break;
                }
            }
        }
        catch (err) {
            if (err instanceof Error && (err.name === 'AbortError' || err.message.includes('aborted'))) {
                opts.emit({ kind: 'stderr', text: 'claude-sdk: aborted by signal' });
                return;
            }
            throw err;
        }
        finally {
            if (killTimer) {
                clearTimeout(killTimer);
            }
            opts.signal?.removeEventListener('abort', onAbort);
            if (abortController.signal.aborted) {
                await query.interrupt?.().catch(() => undefined);
            }
        }
    },
};
//# sourceMappingURL=claude-sdk.js.map