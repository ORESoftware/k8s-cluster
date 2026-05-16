// Shared NDJSON line parser for the CLI runners (claude-cli,
// openai-codex-cli). Both expect "one JSON object per line on stdout"
// and need:
//   * line splitting on "\n"
//   * JSON.parse with a fallback for malformed lines
//   * a hard cap on per-line buffer growth — without it a huge tool
//     result (e.g. agent reads a 50MB file and the CLI dumps it as
//     a single line) accumulates into memory until OOM.

import type { ChildProcess } from "node:child_process";

import type { AgentRunnerEvent } from "./types.js";

/**
 * Hard ceiling for a single line. 4 MiB is generous — claude/codex
 * normally emit tens of KB per line max — and small enough that a runaway
 * tool-result line gets killed instead of starving the container.
 */
export const MAX_LINE_BYTES = 4 * 1024 * 1024;

export interface AttachLineParserOpts {
  child: ChildProcess;
  emit: (_event: AgentRunnerEvent) => void;
  /**
   * Called with the offending line size when MAX_LINE_BYTES is exceeded.
   * Default: SIGKILL the child + emit an error event.
   */
  onOverflow?: (_bytes: number) => void;
}

export function attachNdjsonLineParser(opts: AttachLineParserOpts): void {
  const { child, emit } = opts;
  let stdoutBuf = "";

  child.stdout?.on("data", (chunk: Buffer) => {
    stdoutBuf += chunk.toString("utf8");

    // Cap before splitting — a single megabyte-line never reaches the
    // newline-search loop because we kill the process first.
    if (stdoutBuf.length > MAX_LINE_BYTES) {
      const bytes = stdoutBuf.length;
      // Drop the buffer to free memory immediately.
      stdoutBuf = "";
      if (opts.onOverflow) {
        opts.onOverflow(bytes);
      } else {
        emit({
          kind: "error",
          message: `stdout single-line exceeded ${MAX_LINE_BYTES} bytes (${bytes}); killing agent`,
        });
        try {
          if (!child.killed) {child.kill("SIGKILL");}
        } catch {
          /* ignore */
        }
      }
      return;
    }

    let nl: number;
    while ((nl = stdoutBuf.indexOf("\n")) >= 0) {
      const line = stdoutBuf.slice(0, nl).trim();
      stdoutBuf = stdoutBuf.slice(nl + 1);
      if (!line) {continue;}
      try {
        const raw = JSON.parse(line) as unknown;
        emit({ kind: "claude", raw });
      } catch {
        emit({
          kind: "claude",
          raw: { _unparseable: line.slice(0, 4000) },
        });
      }
    }
  });

  child.stderr?.on("data", (chunk: Buffer) => {
    const text = chunk.toString("utf8");
    if (text.trim()) {emit({ kind: "stderr", text });}
  });
}
