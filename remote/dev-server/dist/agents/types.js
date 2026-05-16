// Pluggable agent runner — lets us swap Claude / OpenAI / etc. behind one
// interface so the orchestration in server.ts (worktree, git, push, PR,
// outputs publishing) is identical regardless of who's "doing the work".
//
// Each runner is responsible for:
//   * spawning the underlying agent (CLI binary or SDK call)
//   * forwarding model output as `claude`/`stderr`/`error` events
//   * exposing its child process handle (if any) so cancel can SIGTERM it
//   * resolving when the agent exits cleanly, rejecting otherwise
export {};
//# sourceMappingURL=types.js.map