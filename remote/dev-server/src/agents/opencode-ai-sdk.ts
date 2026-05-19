// OpenCode Zen runner - uses Vercel AI SDK with the OpenAI-compatible adapter.
//
// This is model-only for now. It answers from thread context, but it does not
// expose local shell/edit tools, so server.ts skips it for repo inspection and
// file-edit prompts.

import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { generateText } from 'ai';

import type { AgentRunOpts, AgentRunner } from './types.js';

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
    const provider = createOpenAICompatible({
      name: 'opencode',
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
          prompt: opts.prompt,
          abortSignal: opts.signal,
        });
        if (!result.text.trim()) {
          throw new Error(`${modelId} produced no text output`);
        }
        opts.emit({
          kind: 'claude',
          raw: {
            provider: 'opencode-ai-sdk',
            model: modelId,
            text: result.text,
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
