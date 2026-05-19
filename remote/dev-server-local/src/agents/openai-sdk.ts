// OpenAI Agents SDK runner - drives an OpenAI-compatible coding agent
// in process via @openai/agents.
//
// The SDK path streams structured events and exposes local shell/apply-patch
// tools scoped to the per-thread worktree. The Codex CLI runner remains as
// the CLI fallback when the SDK surface changes.

import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";

import type { AgentRunOpts, AgentRunner } from "./types.js";

type OpenAiAgentsModule = {
  Agent: new (_input: Record<string, unknown>) => unknown;
  applyDiff: (
    _input: string,
    _diff: string,
    _mode?: "create" | "default",
  ) => string;
  applyPatchTool: (_input: Record<string, unknown>) => unknown;
  run: (
    _agent: unknown,
    _input: string,
    _options: Record<string, unknown>,
  ) => Promise<
    AsyncIterable<unknown> & {
      completed?: Promise<unknown>;
      toTextStream?: (_options?: Record<string, unknown>) => unknown;
    }
  >;
  shellTool: (_input: Record<string, unknown>) => unknown;
};

type ShellAction = {
  commands?: string[];
  maxOutputLength?: number;
  timeoutMs?: number;
};

type ApplyPatchOperation = {
  path?: string;
  diff?: string;
};

const BLOCKED_SHELL_TOKEN_PATTERN =
  /(?:^|[\s;&|()'"`])(?:rm|sed|mv)(?=$|[\s;&|()'"`])/;
const BLOCKED_GIT_PATTERN =
  /(?:^|[\s;&|()'"`])git\s+(?:stash|reset|checkout)(?=$|[\s;&|()'"`])/;

function resolveInsideWorkspace(cwd: string, maybePath: string): string {
  const absolute = resolve(cwd, maybePath);
  const rel = relative(cwd, absolute);
  if (rel === "" || rel.startsWith("..") || resolve(rel) === rel) {
    throw new Error(`path escapes workspace: ${maybePath}`);
  }
  if (rel === ".git" || rel.startsWith(".git/")) {
    throw new Error(`refusing to edit git internals: ${maybePath}`);
  }
  return absolute;
}

function commandIsBlocked(command: string): boolean {
  return (
    BLOCKED_SHELL_TOKEN_PATTERN.test(command) ||
    BLOCKED_GIT_PATTERN.test(command)
  );
}

async function runOneShellCommand(
  command: string,
  opts: AgentRunOpts,
  timeoutMs: number,
  maxOutputLength: number,
): Promise<{
  stdout: string;
  stderr: string;
  outcome: { type: "exit"; exitCode: number };
}> {
  if (commandIsBlocked(command)) {
    return {
      stdout: "",
      stderr:
        "Blocked by remote-dev shell policy. Use safe file tools or git-safe alternatives.",
      outcome: { type: "exit", exitCode: 126 },
    };
  }

  return new Promise((resolveShell, reject) => {
    const child = spawn("/bin/bash", ["-lc", command], {
      cwd: opts.cwd,
      env: opts.env,
    });
    opts.setChild?.(child);

    let stdout = "";
    let stderr = "";
    const trim = (value: string) => value.slice(0, maxOutputLength);

    child.stdout.on("data", (chunk: Buffer) => {
      stdout = trim(stdout + chunk.toString("utf8"));
    });
    child.stderr.on("data", (chunk: Buffer) => {
      stderr = trim(stderr + chunk.toString("utf8"));
    });

    const timer = setTimeout(() => {
      try {
        if (!child.killed) {
          child.kill("SIGKILL");
        }
      } catch {
        /* ignore */
      }
      stderr = trim(`${stderr}\nTimed out after ${timeoutMs}ms`);
    }, timeoutMs);

    const onAbort = () => {
      try {
        if (!child.killed) {
          child.kill("SIGTERM");
        }
      } catch {
        /* ignore */
      }
    };
    opts.signal?.addEventListener("abort", onAbort);

    child.on("close", (code) => {
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onAbort);
      resolveShell({
        stdout,
        stderr,
        outcome: { type: "exit", exitCode: code ?? 1 },
      });
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onAbort);
      reject(err);
    });
  });
}

export const openaiSdkRunner: AgentRunner = {
  id: "openai-sdk",
  displayName: "OpenAI Agents SDK",

  async run(opts: AgentRunOpts): Promise<void> {
    if (!opts.env.OPENAI_API_KEY) {
      throw new Error(
        "openai-sdk requires OPENAI_API_KEY in the env allowlist",
      );
    }

    const agents = (await import(
      "@openai/agents"
    )) as unknown as OpenAiAgentsModule;

    const shell = {
      run: async (action: ShellAction) => {
        const commands = action.commands ?? [];
        const maxOutputLength = Math.min(
          action.maxOutputLength ?? 20_000,
          80_000,
        );
        const timeoutMs = Math.min(
          action.timeoutMs ?? 120_000,
          10 * 60_000,
        );
        const output = [];
        for (const command of commands) {
          output.push(
            await runOneShellCommand(command, opts, timeoutMs, maxOutputLength),
          );
        }
        return { output };
      },
    };

    const editor = {
      createFile: async (operation: ApplyPatchOperation) => {
        if (!operation.path || !operation.diff) {
          return {
            status: "failed",
            output: "create_file requires path + diff",
          };
        }
        const filePath = resolveInsideWorkspace(opts.cwd, operation.path);
        await mkdir(dirname(filePath), { recursive: true });
        const next = agents.applyDiff("", operation.diff, "create");
        await writeFile(filePath, next);
        return { status: "completed", output: `created ${operation.path}` };
      },
      updateFile: async (operation: ApplyPatchOperation) => {
        if (!operation.path || !operation.diff) {
          return {
            status: "failed",
            output: "update_file requires path + diff",
          };
        }
        const filePath = resolveInsideWorkspace(opts.cwd, operation.path);
        const current = await readFile(filePath, "utf8");
        const next = agents.applyDiff(current, operation.diff);
        await writeFile(filePath, next);
        return { status: "completed", output: `updated ${operation.path}` };
      },
      deleteFile: async (operation: ApplyPatchOperation) => {
        const path = operation.path ?? "<missing>";
        return {
          status: "failed",
          output: `delete_file refused for ${path}; use shell git rm for tracked files`,
        };
      },
    };

    const agent = new agents.Agent({
      name: "DD Remote Dev Agent",
      model: opts.env.OPENAI_MODEL ?? "gpt-5.5",
      instructions:
        "You are a coding agent working inside a per-thread git workspace. " +
        "Use apply_patch for file edits and shell for inspection/tests. " +
        "Do not use rm, sed, mv, git reset, git checkout, or git stash. " +
        "Keep changes scoped and leave a concise final summary.",
      tools: [
        agents.applyPatchTool({ editor, needsApproval: false }),
        agents.shellTool({ shell, needsApproval: false }),
      ],
    });

    const abortController = new AbortController();
    const onAbort = () => abortController.abort();
    opts.signal?.addEventListener("abort", onAbort);

    let killTimer: ReturnType<typeof setTimeout> | null = null;
    if (opts.timeoutMs && opts.timeoutMs > 0) {
      killTimer = setTimeout(() => {
        abortController.abort();
        opts.emit({
          kind: "error",
          message: `openai-sdk timed out after ${opts.timeoutMs}ms`,
        });
      }, opts.timeoutMs);
    }

    try {
      const stream = await agents.run(agent, opts.prompt, {
        stream: true,
        maxTurns: Number(process.env.OPENAI_AGENT_MAX_TURNS ?? 50),
        signal: abortController.signal,
      });

      for await (const event of stream) {
        if (abortController.signal.aborted) {
          break;
        }
        opts.emit({ kind: "claude", raw: event });
      }
      await stream.completed;
    } catch (err) {
      if (
        err instanceof Error &&
        (err.name === "AbortError" || abortController.signal.aborted)
      ) {
        return;
      }
      throw err;
    } finally {
      if (killTimer) {
        clearTimeout(killTimer);
      }
      opts.signal?.removeEventListener("abort", onAbort);
    }
  },
};
