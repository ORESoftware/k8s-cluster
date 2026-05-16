import type { AgentRunOpts, AgentRunner } from './types.js';

export function extractEchoText(prompt: string): string {
  const currentPrompt = extractCurrentUserPrompt(prompt);
  const quoted = currentPrompt.match(/echo\s+back\s+['"]([^'"]+)['"]/i);
  if (quoted?.[1]?.trim()) {
    return quoted[1].trim();
  }

  const unquoted = currentPrompt.match(/echo\s+back\s+(.+)$/i);
  if (unquoted?.[1]?.trim()) {
    return unquoted[1].trim();
  }

  const reverse = currentPrompt.match(/echo\s+(.+?)\s+back$/i);
  if (reverse?.[1]?.trim()) {
    return reverse[1].trim();
  }

  const simple = currentPrompt.match(/echo\s+(.+)$/i);
  if (simple?.[1]?.trim()) {
    return simple[1].trim();
  }

  return `Echo: ${currentPrompt}`;
}

function extractCurrentUserPrompt(prompt: string): string {
  const current = prompt.match(/<current_user_prompt>\s*([\s\S]*?)\s*<\/current_user_prompt>/i);
  return current?.[1]?.trim() || prompt.trim();
}

export const echoRunner: AgentRunner = {
  id: 'echo',
  displayName: 'Echo fallback',

  async run(opts: AgentRunOpts): Promise<void> {
    opts.emit({
      kind: 'claude',
      raw: {
        type: 'assistant_response',
        provider: 'echo',
        text: extractEchoText(opts.prompt),
      },
    });
  },
};
