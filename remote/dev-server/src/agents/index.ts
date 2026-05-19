// Agent runner selector. Resolution order, highest precedence first:
//   1. Per-task `provider` field on the dispatch payload (untrusted input,
//      validated against the AgentProvider union).
//   2. AGENT_PROVIDER env var.
//   3. Hard-coded default: "gemini-sdk".
//
// Per-runner secret needs (must be present in the env allowlist passed to
// run(), not just process.env):
//   claude-cli       → ANTHROPIC_API_KEY
//   claude-sdk       → ANTHROPIC_API_KEY  (after SDK install)
//   gemini-sdk       → GEMINI_API_KEY      (after SDK install)
//   openai-codex-cli → OPENAI_API_KEY     (and `codex` binary on PATH)
//   openai-sdk       → OPENAI_API_KEY     (after SDK install)

import { spawn } from 'node:child_process';

import { claudeCliRunner } from './claude-cli.js';
import { claudeSdkRunner, resolveClaudeCodeExecutable } from './claude-sdk.js';
import { geminiSdkRunner } from './gemini-sdk.js';
import { openaiCodexCliRunner } from './openai-codex-cli.js';
import { openaiSdkRunner } from './openai-sdk.js';
import type { AgentProvider, AgentRunner } from './types.js';

export type { AgentProvider, AgentRunner, AgentRunOpts, AgentRunnerEvent } from './types.js';

const RUNNERS: Record<AgentProvider, AgentRunner> = {
  'claude-cli': claudeCliRunner,
  'claude-sdk': claudeSdkRunner,
  'gemini-sdk': geminiSdkRunner,
  'openai-codex-cli': openaiCodexCliRunner,
  'openai-sdk': openaiSdkRunner,
};

const DEFAULT_ANTHROPIC_MODEL = 'claude-opus-4-7';
const DEFAULT_GEMINI_MODEL = 'gemini-3.1-pro';
const DEFAULT_GEMINI_FALLBACK_MODEL = 'gemini-3.1-flash-lite';
const DEFAULT_OPENAI_MODEL = 'gpt-5.5';

const VALID_PROVIDERS = new Set<AgentProvider>([
  'claude-cli',
  'claude-sdk',
  'gemini-sdk',
  'openai-codex-cli',
  'openai-sdk',
]);

function isAgentProvider(value: unknown): value is AgentProvider {
  return typeof value === 'string' && VALID_PROVIDERS.has(value as AgentProvider);
}

function configuredSecret(name: string): string | undefined {
  const value = process.env[name]?.trim();
  if (!value || value === 'REPLACE_ME' || value.startsWith('REPLACE_ME_')) {
    return undefined;
  }
  return value;
}

/**
 * Resolve which agent provider to use. Order of precedence:
 *   1. Explicit per-task override (from dispatch payload)
 *   2. AGENT_PROVIDER env var
 *   3. Availability adjustment: prefer SDK over CLI when both are available
 *   4. Default: "gemini-sdk"
 *
 * The availability logic checks the cached probe when it exists. It can
 * upgrade a CLI choice to an available SDK or fall back from an unavailable
 * SDK default to an available CLI runner.
 */
export function resolveAgentProvider(perTaskOverride?: string | null): AgentProvider {
  let chosen: AgentProvider;
  if (isAgentProvider(perTaskOverride)) {
    chosen = perTaskOverride;
  } else {
    const fromEnv = process.env.AGENT_PROVIDER;
    chosen = isAgentProvider(fromEnv) ? fromEnv : 'gemini-sdk';
  }

  // Prefer SDK runners when available, but fall back to CLI if the cached
  // probe says an SDK default is unavailable.
  // Skip adjustments if the user explicitly picked a per-task provider.
  if (!perTaskOverride && cachedAvailability) {
    const sdkUpgrades: Record<string, AgentProvider> = {
      'claude-cli': 'claude-sdk',
      'openai-codex-cli': 'openai-sdk',
    };
    const cliFallbacks: Record<string, AgentProvider> = {
      'claude-sdk': 'claude-cli',
      'openai-sdk': 'openai-codex-cli',
    };
    const sdkTarget = sdkUpgrades[chosen];
    if (sdkTarget) {
      const sdkEntry = cachedAvailability.find((p) => p.provider === sdkTarget);
      if (sdkEntry?.available) {
        chosen = sdkTarget;
      }
    } else {
      const chosenEntry = cachedAvailability.find((p) => p.provider === chosen);
      const cliFallback = cliFallbacks[chosen];
      const cliEntry = cliFallback
        ? cachedAvailability.find((p) => p.provider === cliFallback)
        : undefined;
      if (cliFallback && chosenEntry && !chosenEntry.available && cliEntry?.available) {
        chosen = cliFallback;
      }
    }
  }

  return chosen;
}

export function getRunner(provider: AgentProvider): AgentRunner {
  return RUNNERS[provider];
}

/**
 * Per-runner env allowlist passed to `run()`. We never inherit the full
 * process.env into the agent process — it'd hand it our GitHub deploy
 * key, Supabase service role key, etc. on a silver platter via `printenv`.
 *
 * Build the strict subset here so each provider gets exactly what it
 * needs to authenticate to its model and nothing else.
 */
export function buildAgentEnv(provider: AgentProvider): Record<string, string> {
  const base: Record<string, string> = {
    PATH: process.env.PATH ?? '',
    HOME: process.env.HOME ?? '/home/node',
    USER: process.env.USER ?? 'node',
    LANG: process.env.LANG ?? 'C.UTF-8',
    NODE_ENV: process.env.NODE_ENV ?? 'production',
  };

  if (provider === 'claude-cli' || provider === 'claude-sdk') {
    if (process.env.ANTHROPIC_API_KEY) {
      base.ANTHROPIC_API_KEY = process.env.ANTHROPIC_API_KEY;
    }
    // Pin the model when set (e.g. `claude-opus-4-7`). Without this we
    // get whatever the CLI's config / SDK default picks, which can drift
    // when Anthropic ships a newer flagship.
    base.ANTHROPIC_MODEL = process.env.ANTHROPIC_MODEL ?? DEFAULT_ANTHROPIC_MODEL;
    // Pass through proxy / Bedrock / custom-endpoint config when set.
    if (process.env.ANTHROPIC_BASE_URL) {
      base.ANTHROPIC_BASE_URL = process.env.ANTHROPIC_BASE_URL;
    }
  }
  if (provider === 'gemini-sdk') {
    const apiKey = configuredSecret('GOOGLE_API_KEY') ?? configuredSecret('GEMINI_API_KEY');
    if (apiKey) {
      base.GEMINI_API_KEY = apiKey;
    }
    base.GEMINI_MODEL = process.env.GEMINI_MODEL ?? DEFAULT_GEMINI_MODEL;
    base.GEMINI_FALLBACK_MODEL =
      process.env.GEMINI_FALLBACK_MODEL ?? DEFAULT_GEMINI_FALLBACK_MODEL;
  }
  if (provider === 'openai-codex-cli' || provider === 'openai-sdk') {
    if (process.env.OPENAI_API_KEY) {
      base.OPENAI_API_KEY = process.env.OPENAI_API_KEY;
    }
    base.OPENAI_MODEL = process.env.OPENAI_MODEL ?? DEFAULT_OPENAI_MODEL;
    base.CODEX_MODEL = process.env.CODEX_MODEL ?? base.OPENAI_MODEL;
    // Azure OpenAI / proxy support.
    if (process.env.OPENAI_BASE_URL) {
      base.OPENAI_BASE_URL = process.env.OPENAI_BASE_URL;
    }
    if (process.env.OPENAI_ORG) {
      base.OPENAI_ORG = process.env.OPENAI_ORG;
    }
  }

  return base;
}

// ---------------------------------------------------- availability probe

export interface AgentAvailability {
  provider: AgentProvider;
  displayName: string;
  available: boolean;
  /** Why unavailable, if not. */
  reason?: string;
}

const HAS_BIN_TIMEOUT_MS = 3_000;

/**
 * Spawn `<bin> --version` and return true if it exits 0 within the
 * timeout. Used to detect whether `claude` and `codex` are actually
 * installed in this image. ENOENT → false.
 */
function probeBinary(bin: string): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const finish = (ok: boolean) => {
      if (settled) {
        return;
      }
      settled = true;
      resolve(ok);
    };
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(bin, ['--version'], { stdio: 'ignore' });
    } catch {
      finish(false);
      return;
    }
    const t = setTimeout(() => {
      try {
        child.kill('SIGKILL');
      } catch {
        /* ignore */
      }
      finish(false);
    }, HAS_BIN_TIMEOUT_MS);
    child.on('error', () => {
      clearTimeout(t);
      finish(false);
    });
    child.on('close', (code) => {
      clearTimeout(t);
      finish(code === 0);
    });
  });
}

/**
 * Try a dynamic import of an SDK package without throwing. Returns true
 * if the package is resolvable in node_modules.
 */
async function probePackage(pkg: string): Promise<boolean> {
  try {
    await import(pkg);
    return true;
  } catch {
    return false;
  }
}

let cachedAvailability: AgentAvailability[] | null = null;

/**
 * Probe each provider once and cache. Called at boot; the /agents
 * endpoint serves the cache. Re-probe on demand isn't worth the cost —
 * if you install a new SDK, restart the container.
 */
export async function probeAllProviders(): Promise<AgentAvailability[]> {
  if (cachedAvailability) {
    return cachedAvailability;
  }

  const [hasClaude, hasCodex, hasClaudeSdkPackage, hasGeminiSdk, hasOpenaiSdk] = await Promise.all([
    probeBinary('claude'),
    probeBinary('codex'),
    probePackage('@anthropic-ai/claude-agent-sdk'),
    probePackage('@google/genai'),
    probePackage('@openai/agents'),
  ]);
  const hasClaudeSdkExecutable = hasClaudeSdkPackage && !!resolveClaudeCodeExecutable();
  const hasGeminiApiKey = !!(configuredSecret('GOOGLE_API_KEY') ?? configuredSecret('GEMINI_API_KEY'));

  const out: AgentAvailability[] = [
    {
      provider: 'claude-cli',
      displayName: claudeCliRunner.displayName,
      available: hasClaude && !!process.env.ANTHROPIC_API_KEY,
      reason: !hasClaude
        ? '`claude` binary not on PATH (npm i -g @anthropic-ai/claude-code)'
        : !process.env.ANTHROPIC_API_KEY
          ? 'ANTHROPIC_API_KEY not set'
          : undefined,
    },
    {
      provider: 'claude-sdk',
      displayName: claudeSdkRunner.displayName,
      available: hasClaudeSdkPackage && hasClaudeSdkExecutable && !!process.env.ANTHROPIC_API_KEY,
      reason: !hasClaudeSdkPackage
        ? '@anthropic-ai/claude-agent-sdk not installed (pnpm add @anthropic-ai/claude-agent-sdk)'
        : !hasClaudeSdkExecutable
          ? 'Claude SDK native executable not found or not executable'
          : !process.env.ANTHROPIC_API_KEY
            ? 'ANTHROPIC_API_KEY not set'
            : undefined,
    },
    {
      provider: 'gemini-sdk',
      displayName: geminiSdkRunner.displayName,
      available: hasGeminiSdk && hasGeminiApiKey,
      reason: !hasGeminiSdk
        ? '@google/genai package not installed'
        : !hasGeminiApiKey
          ? 'GOOGLE_API_KEY or GEMINI_API_KEY not set'
          : undefined,
    },
    {
      provider: 'openai-codex-cli',
      displayName: openaiCodexCliRunner.displayName,
      available: hasCodex && !!process.env.OPENAI_API_KEY,
      reason: !hasCodex
        ? '`codex` binary not on PATH (install OpenAI Codex CLI)'
        : !process.env.OPENAI_API_KEY
          ? 'OPENAI_API_KEY not set'
          : undefined,
    },
    {
      provider: 'openai-sdk',
      displayName: openaiSdkRunner.displayName,
      available: hasOpenaiSdk && !!process.env.OPENAI_API_KEY,
      reason: !hasOpenaiSdk
        ? '@openai/agents package not installed'
        : !process.env.OPENAI_API_KEY
          ? 'OPENAI_API_KEY not set'
          : undefined,
    },
  ];
  cachedAvailability = out;
  return out;
}

export function getCachedAvailability(): AgentAvailability[] | null {
  return cachedAvailability;
}
