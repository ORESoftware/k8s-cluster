// Pluggable agent runner — lets us swap Claude / OpenAI / etc. behind one
// interface so the orchestration in server.ts (worktree, git, push, PR,
// outputs publishing) is identical regardless of who's "doing the work".
//
// Each runner is responsible for:
//   * spawning the underlying agent (CLI binary or SDK call)
//   * forwarding model output as `claude`/`stderr`/`error` events
//   * exposing its child process handle (if any) so cancel can SIGTERM it
//   * resolving when the agent exits cleanly, rejecting otherwise

import type { ChildProcess } from 'node:child_process';

export type AgentProvider =
  | 'claude-cli'
  | 'claude-sdk'
  | 'gemini-sdk'
  | 'opencode-ai-sdk'
  | 'openai-codex-cli'
  | 'openai-sdk';

/**
 * The subset of WrappedEvent kinds an agent runner can emit. Status / done /
 * artifact events are emitted by server.ts itself, not the runner.
 */
export type AgentRunnerEvent =
  | { kind: 'claude'; raw: unknown }
  | { kind: 'stderr'; text: string }
  | { kind: 'error'; message: string };

export interface AgentRunOpts {
  /** User prompt (passed straight to the underlying model). */
  prompt: string;
  /** Working directory — the per-task git worktree. */
  cwd: string;
  /** Strict env allowlist for the agent process; never inherit full env. */
  env: Record<string, string>;
  /** Per-event callback. Server.ts wraps these into the SSE/Realtime stream. */
  emit: (event: AgentRunnerEvent) => void;
  /** Called once when the agent has a kill-able handle, so cancel works. */
  setChild?: (child: ChildProcess) => void;
  /** Hard wall-clock cap for the run (ms). Runner enforces. */
  timeoutMs?: number;
  /** Aborted by the orchestrator (e.g. cancel) — runner should bail. */
  signal?: AbortSignal;
}

export interface AgentRunner {
  readonly id: AgentProvider;
  readonly displayName: string;
  /**
   * Resolve when the agent has finished cleanly. Reject for non-zero exit
   * or any unrecoverable error. The runner does NOT need to emit a `done`
   * event — server.ts owns that.
   */
  run(opts: AgentRunOpts): Promise<void>;
}
