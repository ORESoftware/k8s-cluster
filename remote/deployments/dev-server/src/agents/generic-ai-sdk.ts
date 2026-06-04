// Generic OpenAI-compatible runner using Vercel AI SDK.
//
// Uses a bounded workspace toolset so OpenAI-compatible models can inspect
// and edit repo files without receiving broad shell or process env access.

import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { generateText, stepCountIs } from 'ai';

import type { AgentRunOpts, AgentRunner } from './types.js';
import { createWorkspaceTools } from './workspace-tools.js';

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
    displayName: 'DeepSeek V4',
    baseURL: 'https://api.deepseek.com',
    // V4 Flash is the value default; Pro remains in rotation for harder coding tasks.
    models: ['deepseek-v4-flash', 'deepseek-v4-pro'],
  },
  {
    id: 'qwen',
    displayName: 'Qwen DashScope',
    baseURL: 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
    models: ['qwen3.6-max-preview', 'qwen3.6-plus', 'qwen3.6-flash'],
  },
  {
    id: 'xai',
    displayName: 'Grok 4.x',
    baseURL: 'https://api.x.ai/v1',
    // Older Grok 4 fast and code-fast slugs retired on 2026-05-15 and redirect
    // to grok-4.3, so prefer the current canonical slug directly.
    models: ['grok-4.3'],
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
          system:
            'You are editing a git workspace. Use the provided workspace tools for repo inspection and file edits. ' +
            'Keep changes focused on the user request. Do not claim files were edited unless you used a write tool. ' +
            // PR-comment / PR-edit requests must hit GitHub, not a workspace
            // file. The pr_comment / pr_update_body / pr_view tools wrap the
            // worker's gh CLI — file appends are NEVER a substitute for a
            // real PR comment, even in autonomous mode.
            'If the user asks to comment on, update, or describe a pull request, use pr_view, pr_comment, or pr_update_body. ' +
            'Never substitute append_file, write_file, or replace_in_file for a PR comment or PR body update. ' +
            'Never call any tool that would merge, close, or mark-ready a PR — those flows are off limits. ' +
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
            provider: 'generic-ai-sdk',
            source: sourceId,
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

    throw new Error(`Generic AI SDK failed ${sourceId} models: ${failures.join(' | ')}`);
  },
};
