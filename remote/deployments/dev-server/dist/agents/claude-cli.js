// Claude Code CLI runner — spawns the `claude` binary and parses its
// `--output-format stream-json` NDJSON stdout.
//
// This runner works without any SDK install — just
// `npm i -g @anthropic-ai/claude-code` (already done in the Dockerfile).
import { spawn } from "node:child_process";
import { attachNdjsonLineParser } from "./cli-stream.js";
export const claudeCliRunner = {
    id: "claude-cli",
    displayName: "Claude Code (CLI)",
    async run(opts) {
        return new Promise((resolve, reject) => {
            const args = [
                "-p",
                opts.prompt,
                "--output-format",
                "stream-json",
                "--verbose",
                // Bounded by the worktree (the agent only has filesystem access
                // inside cwd) + by the strict env allowlist. PR is the human
                // review gate. See `Known accepted risks` in remote/readme.md.
                "--dangerously-skip-permissions",
            ];
            // Pin the model when env-set (e.g. `claude-opus-4-7`). Without
            // this the CLI picks whatever it considers default — fine until
            // Anthropic ships a newer flagship and you silently switch off it.
            if (opts.env.ANTHROPIC_MODEL) {
                args.push("--model", opts.env.ANTHROPIC_MODEL);
            }
            const child = spawn("claude", args, { cwd: opts.cwd, env: opts.env });
            opts.setChild?.(child);
            attachNdjsonLineParser({ child, emit: opts.emit });
            const onAbort = () => {
                try {
                    if (!child.killed) {
                        child.kill("SIGTERM");
                    }
                }
                catch {
                    /* ignore */
                }
            };
            opts.signal?.addEventListener("abort", onAbort);
            let killTimer = null;
            if (opts.timeoutMs && opts.timeoutMs > 0) {
                killTimer = setTimeout(() => {
                    try {
                        if (!child.killed) {
                            child.kill("SIGKILL");
                        }
                    }
                    catch {
                        /* ignore */
                    }
                    opts.emit({
                        kind: "error",
                        message: `claude-cli timed out after ${opts.timeoutMs}ms`,
                    });
                }, opts.timeoutMs);
            }
            child.on("close", (code) => {
                if (killTimer) {
                    clearTimeout(killTimer);
                }
                opts.signal?.removeEventListener("abort", onAbort);
                if (code === 0) {
                    resolve();
                }
                else {
                    reject(new Error(`claude exited with code ${code}`));
                }
            });
            child.on("error", (err) => {
                if (killTimer) {
                    clearTimeout(killTimer);
                }
                opts.signal?.removeEventListener("abort", onAbort);
                reject(err);
            });
        });
    },
};
//# sourceMappingURL=claude-cli.js.map