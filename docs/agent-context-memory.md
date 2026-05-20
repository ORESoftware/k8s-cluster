# Agent Context Memory

Remote coding agents should treat the remote-dev thread UUID as the durable conversation id.
Provider-side chat/session memory is not reliable across OpenAI, Anthropic, Gemini, DeepSeek,
Grok, and OpenCode-compatible calls, so the platform stores and re-injects context itself.

## Durable Context Layers

- `AGENTS.md` is the repo entrypoint. It points agents to `docs/*.md`, `agents/*.md`, and nested
  `AGENTS.md` files for persistent repo-local context.
- Postgres `agent_context_blobs` stores curated long-lived memory for a repo/project. The REST API
  selects matching blobs and passes them to workers as `contextBlobs`.
- Postgres `agent_remote_dev_threads`, `agent_remote_dev_tasks`, and `agent_remote_dev_events`
  store per-thread conversation and task history keyed by thread UUID.
- Each worker also keeps a local `tmp/convos/thread.log` tail inside the thread workspace so a
  warm or recovered worker can continue even when the REST context lookup is unavailable.

## Runtime Prompt Contract

Every SDK/CLI runner receives one shared prompt assembled by `remote/deployments/dev-server`.
That prompt includes the thread UUID, current task UUID, optimistic operating mode, repo context
files, selected Postgres context blobs, previous thread summaries, local thread-log tail, runtime
MCP hints, and finally the current user prompt.

Optimistic mode means agents should make safe assumptions and proceed without pausing for human
input. If a question would normally block the work, the agent should document the question and its
assumption in the final summary so the human can answer in a later task prompt.
