# Agent Context Memory

Remote coding agents should treat the remote-dev thread UUID as the durable conversation id.
Provider-side chat/session memory is not reliable across OpenAI, Anthropic, Gemini, DeepSeek,
Grok, and OpenCode-compatible calls, so the platform stores and re-injects context itself.

## Durable Context Layers

- `AGENTS.md` is the repo entrypoint. It points agents to `docs/*.md`, `agents/*.md`, and nested
  `AGENTS.md` files for persistent repo-local context.
- Postgres `agent_context_blobs` stores curated long-lived memory for a repo/project. The REST API
  ranks matching blobs and the thread UI lets the operator keep or uncheck each row before dispatch.
- Postgres `agent_remote_dev_threads`, `agent_remote_dev_tasks`, `agent_remote_dev_events`, and
  `agent_remote_dev_breadcrumbs` store per-thread conversation, task, event, and breadcrumb history
  keyed by thread UUID. Previous-task rows and individual breadcrumbs can be selected into the same
  context review payload as durable blobs.

## Runtime Prompt Contract

Every SDK/CLI runner receives one shared prompt assembled by `remote/deployments/dev-server`.
That prompt includes the thread UUID, current task UUID, optimistic operating mode, repo context
files, selected Postgres context rows, runtime MCP hints, and finally the current user prompt.
In automatic mode the worker can receive previous thread summaries from the REST API. In selected
mode it receives only the checked durable blobs, previous tasks, and breadcrumbs. In zero-context
mode it receives no previous task, breadcrumb, or selected blob context.

Optimistic mode means agents should make safe assumptions and proceed without pausing for human
input. If a question would normally block the work, the agent should document the question and its
assumption in the final summary so the human can answer in a later task prompt.
