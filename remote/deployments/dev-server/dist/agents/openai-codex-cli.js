// OpenAI Codex CLI runner — spawns the `codex` binary, OpenAI's official
// agentic coding tool (analogous to Claude Code on the Anthropic side).
//
// Install in the Dockerfile:
//   npm i -g @openai/codex
// (or whatever the install instruction is for the version you target)
//
// CLI invocation pattern (verify against your installed version — this
// CLI has shipped breaking changes between releases):
//   codex exec "<prompt>" --json
//
// `--json` is treated here as the equivalent of claude's
// `--output-format stream-json` — NDJSON on stdout, one JSON object per
// line. If your version uses a different flag name, adjust the args
// array below. The structure of each event differs from claude's, but
// our wrapper passes them through opaquely as `kind: "claude"` events
// (the kind name is historical; payload shape is provider-specific and
// the UI renders best-effort).
import { spawn } from "node:child_process";
import { attachNdjsonLineParser } from "./cli-stream.js";
export const openaiCodexCliRunner = {
    id: "openai-codex-cli",
    displayName: "OpenAI Codex (CLI)",
    async run(opts) {
        if (!opts.env.OPENAI_API_KEY) {
            throw new Error("openai-codex-cli requires OPENAI_API_KEY in the env allowlist");
        }
        return new Promise((resolve, reject) => {
            // The default model is whatever the CLI's config picks. To pin per
            // dispatch, accept it via env (e.g. CODEX_MODEL=gpt-5) and pass
            // `--model "$CODEX_MODEL"` here.
            const args = ["exec", opts.prompt, "--json"];
            if (opts.env.CODEX_MODEL) {
                args.push("--model", opts.env.CODEX_MODEL);
            }
            const child = spawn("codex", args, {
                cwd: opts.cwd,
                env: opts.env,
            });
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
                        message: `openai-codex-cli timed out after ${opts.timeoutMs}ms`,
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
                    reject(new Error(`codex exited with code ${code}`));
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
//# sourceMappingURL=openai-codex-cli.js.map