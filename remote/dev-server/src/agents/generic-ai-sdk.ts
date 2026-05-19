// Generic OpenAI-compatible runner using Vercel AI SDK.
//
// This is model-only for now. It can answer from thread context through
// OpenCode, DeepSeek, Qwen/DashScope, xAI, or another compatible endpoint,
// but it does not expose local shell/edit tools.

import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { generateText } from 'ai';

import type { AgentRunOpts, AgentRunner } from './types.js';

export const DEFAULT_GENERIC_AI_SDK_SOURCES = [
  {
    id: 'opencode',
    displayName: 'OpenCode Zen',
    baseURL: 'https://opencode.ai/zen/v1',
    models: [
      'big-pickle',
      'deepseek-v4-flash-free',
      'minimax-m2.5-free',
      'nemotron-3-super-free',
      'qwen3.6-plus-free',
    ],
  },
  {
    id: 'deepseek',
    displayName: 'DeepSeek',
    baseURL: 'https://api.deepseek.com',
    models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
  },
  {
    id: 'qwen',
    displayName: 'Qwen DashScope',
    baseURL: 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
    models: ['qwen3.6-max-preview', 'qwen3.6-plus', 'qwen3.6-flash'],
  },
  {
    id: 'xai',
    displayName: 'xAI Grok',
    baseURL: 'https://api.x.ai/v1',
    models: ['grok-4.3', 'grok-code-fast-1', 'grok-4-fast'],
  },
] as const;

export type GenericAiSdkSourceId = (typeof DEFAULT_GENERIC_AI_SDK_SOURCES)[number]['id'];

export function defaultGenericAiSdkModels(sourceId: string | undefined): string[] {
  const source = DEFAULT_GENERIC_AI_SDK_SOURCES.find((item) => item.id === sourceId);
  return [...(source?.models ?? DEFAULT_GENERIC_AI_SDK_SOURCES[0].models)];
}

function parseModelList(value: string | undefined, fallback: string[]): string[] {
  if (!value?.trim()) {
    return [...fallback];
  }
  try {
    const parsed = JSON.parse(value) as unknown;
    if (Array.isArray(parsed)) {
      return uniqueModels(parsed.filter((item): item is string => typeof item === 'string'), fallback);
    }
  } catch {
    /* fall through to delimited list parsing */
  }
  return uniqueModels(
    value
      .split(/[,\n;]/)
      .map((item) => item.trim())
      .filter(Boolean),
    fallback,
  );
}

function uniqueModels(values: string[], fallback: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values) {
    const model = value.trim();
    if (model && !seen.has(model)) {
      seen.add(model);
      out.push(model);
    }
  }
  return out.length > 0 ? out : [...fallback];
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

export const genericAiSdkRunner: AgentRunner = {
  id: 'generic-ai-sdk',
  displayName: 'Generic AI SDK',

  async run(opts: AgentRunOpts): Promise<void> {
    const sourceId = opts.env.GENERIC_AI_SDK_SOURCE ?? 'generic';
    const apiKey = opts.env.GENERIC_AI_SDK_API_KEY;
    const baseURL = opts.env.GENERIC_AI_SDK_BASE_URL;
    if (!apiKey) {
      throw new Error('generic-ai-sdk requires GENERIC_AI_SDK_API_KEY in the env allowlist');
    }
    if (!baseURL) {
      throw new Error('generic-ai-sdk requires GENERIC_AI_SDK_BASE_URL in the env allowlist');
    }

    const provider = createOpenAICompatible({
      name: sourceId,
      apiKey,
      baseURL,
      includeUsage: true,
    });
    const fallbackModels = defaultGenericAiSdkModels(sourceId);
    const models = parseModelList(opts.env.GENERIC_AI_SDK_MODELS, fallbackModels);
    const failures: string[] = [];

    for (const modelId of models) {
      if (opts.signal?.aborted) {
        opts.emit({ kind: 'stderr', text: 'generic-ai-sdk: aborted by signal' });
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
            provider: 'generic-ai-sdk',
            source: sourceId,
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

    throw new Error(`Generic AI SDK failed ${sourceId} models: ${failures.join(' | ')}`);
  },
};
