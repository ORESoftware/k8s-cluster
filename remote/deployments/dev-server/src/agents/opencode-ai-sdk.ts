// OpenCode Zen runner - uses Vercel AI SDK with the OpenAI-compatible adapter.
//
// Uses a bounded workspace toolset so OpenCode-compatible models can inspect
// and edit repo files without receiving broad shell or process env access.

import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { generateText, stepCountIs } from 'ai';

import type { AgentRunOpts, AgentRunner } from './types.js';
import { createWorkspaceTools } from './workspace-tools.js';

const DEFAULT_OPENCODE_BASE_URL = 'https://opencode.ai/zen/v1';

export const DEFAULT_OPENCODE_MODELS = [
  'big-pickle',
  'deepseek-v4-flash-free',
  'minimax-m2.5-free',
  'nemotron-3-super-free',
  'qwen3.6-plus-free',
] as const;

function parseModelList(value: string | undefined): string[] {
  if (!value?.trim()) {
    return [...DEFAULT_OPENCODE_MODELS];
  }
  try {
    const parsed = JSON.parse(value) as unknown;
    if (Array.isArray(parsed)) {
      return uniqueModels(parsed.filter((item): item is string => typeof item === 'string'));
    }
  } catch {
    /* fall through to delimited list parsing */
  }
  return uniqueModels(
    value
      .split(/[,\n;]/)
      .map((item) => item.trim())
      .filter(Boolean),
  );
}

function uniqueModels(values: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values) {
    const model = value.trim();
    if (model && !seen.has(model)) {
      seen.add(model);
      out.push(model);
    }
  }
  return out.length > 0 ? out : [...DEFAULT_OPENCODE_MODELS];
}

function errorText(error: unknown): string {
  if (error instanceof Error) {
    return `${error.name}: ${error.message}`;
  }
  if (typeof error === 'string') {
    return error;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export const opencodeAiSdkRunner: AgentRunner = {
  id: 'opencode-ai-sdk',
  displayName: 'OpenCode AI SDK',

  async run(opts: AgentRunOpts): Promise<void> {
    if (!opts.env.OPENCODE_API_KEY) {
      throw new Error('opencode-ai-sdk requires OPENCODE_API_KEY in the env allowlist');
    }

    const baseURL = opts.env.OPENCODE_BASE_URL ?? DEFAULT_OPENCODE_BASE_URL;
    const source = opts.env.OPENCODE_SOURCE ?? 'opencode';
    const provider = createOpenAICompatible({
      name: source,
      apiKey: opts.env.OPENCODE_API_KEY,
      baseURL,
      includeUsage: true,
    });
    const models = parseModelList(opts.env.OPENCODE_MODELS ?? opts.env.OPENCODE_MODEL);
    const failures: string[] = [];

    for (const modelId of models) {
      if (opts.signal?.aborted) {
        opts.emit({ kind: 'stderr', text: 'opencode-ai-sdk: aborted by signal' });
        return;
      }
      try {
        const result = await generateText({
          model: provider(modelId),
          system:
            'You are editing a git workspace. Use the provided workspace tools for repo inspection and file edits. ' +
            'Keep changes focused on the user request. Do not claim files were edited unless you used a write tool. ' +
            'When done, call workspace_status and summarize the changed files.',
          prompt: opts.prompt,
          tools: createWorkspaceTools(opts.cwd, opts.emit),
          stopWhen: stepCountIs(8),
          abortSignal: opts.signal,
        });
        if (!result.text.trim() && result.toolResults.length === 0) {
          throw new Error(`${modelId} produced no text output`);
        }
        opts.emit({
          kind: 'claude',
          raw: {
            provider: 'opencode-ai-sdk',
            model: modelId,
            text: result.text,
            toolCalls: result.toolCalls.length,
            toolResults: result.toolResults.length,
            finishReason: result.finishReason,
            usage: result.usage,
          },
        });
        return;
      } catch (error) {
        if (opts.signal?.aborted) {
          throw error;
        }
        failures.push(`${modelId}: ${errorText(error)}`);
      }
    }

    throw new Error(`OpenCode AI SDK failed all configured models: ${failures.join(' | ')}`);
  },
};
