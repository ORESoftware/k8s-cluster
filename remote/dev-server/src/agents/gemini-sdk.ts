// Gemini SDK runner - uses Google's official @google/genai package.
//
// This is intentionally a model-only runner today. It can read the full
// thread context injected by server.ts, but it does not yet expose local
// shell/edit tools like the Claude/OpenAI coding-agent runners.

import type { AgentRunOpts, AgentRunner } from './types.js';

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue | undefined };
type GeminiCandidate = { [key: string]: JsonValue | undefined };
type GeminiUsageMetadata = { [key: string]: JsonValue | undefined };

type GeminiStreamChunk = {
  text?: string;
  candidates?: GeminiCandidate[];
  usageMetadata?: GeminiUsageMetadata;
};

type GoogleGenAiClient = {
  models: {
    generateContentStream: (_input: {
      model: string;
      contents: string;
    }) => Promise<AsyncIterable<GeminiStreamChunk>>;
  };
};

type GoogleGenAiModule = {
  GoogleGenAI: new (_input: { apiKey: string }) => GoogleGenAiClient;
};

export const geminiSdkRunner: AgentRunner = {
  id: 'gemini-sdk',
  displayName: 'Gemini SDK',

  async run(opts: AgentRunOpts): Promise<void> {
    if (!opts.env.GEMINI_API_KEY) {
      throw new Error('gemini-sdk requires GEMINI_API_KEY in the env allowlist');
    }

    const genai = (await import('@google/genai')) as GoogleGenAiModule;
    const client = new genai.GoogleGenAI({ apiKey: opts.env.GEMINI_API_KEY });
    const stream = await client.models.generateContentStream({
      model: opts.env.GEMINI_MODEL ?? 'gemini-3.1-pro',
      contents: opts.prompt,
    });

    for await (const chunk of stream) {
      if (opts.signal?.aborted) {
        opts.emit({ kind: 'stderr', text: 'gemini-sdk: aborted by signal' });
        return;
      }
      opts.emit({
        kind: 'claude',
        raw: {
          provider: 'gemini-sdk',
          text: chunk.text,
          candidates: chunk.candidates,
          usageMetadata: chunk.usageMetadata,
        },
      });
    }
  },
};
