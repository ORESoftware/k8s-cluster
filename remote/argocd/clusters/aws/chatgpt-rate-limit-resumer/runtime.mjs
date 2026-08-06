import { randomUUID } from 'node:crypto';
import process from 'node:process';
import { DEFAULT_CONTINUATION_PROMPT, normalizeText } from './policy.mjs';

export const RUN_ID = randomUUID();
export const STATE_DIR = process.env.CHATGPT_RESUMER_STATE_DIR ?? '/state';
export const STORAGE_STATE_PATH = `${STATE_DIR}/storage-state.json`;
export const RUN_STATE_PATH = `${STATE_DIR}/run-state.json`;
export const SEED_HASH_PATH = `${STATE_DIR}/seed.sha256`;
export const SEED_PATH = process.env.CHATGPT_STORAGE_STATE_SEED ?? '/seed/storage-state.json';
export const RUN_LOCK_PATH = `${STATE_DIR}/run.lock`;

function boundedInt(name, fallback, min, max) {
  const parsed = Number.parseInt(process.env[name] ?? '', 10);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.max(min, Math.min(max, parsed));
}

function envList(name, fallback) {
  const value = process.env[name];
  if (!value) return fallback;
  return value
    .split(',')
    .map((entry) => entry.trim().toLowerCase())
    .filter(Boolean);
}

export const config = {
  baseUrl: process.env.CHATGPT_BASE_URL ?? 'https://chatgpt.com/',
  maxChatScan: boundedInt('CHATGPT_RESUMER_MAX_CHAT_SCAN', 60, 1, 250),
  maxResumes: boundedInt('CHATGPT_RESUMER_MAX_RESUMES', 8, 1, 25),
  maxAttempts: boundedInt('CHATGPT_RESUMER_MAX_ATTEMPTS', 7, 1, 30),
  sidebarScrollPasses: boundedInt('CHATGPT_RESUMER_SIDEBAR_SCROLL_PASSES', 10, 1, 30),
  pageTimeoutMs: boundedInt('CHATGPT_RESUMER_PAGE_TIMEOUT_MS', 60_000, 10_000, 180_000),
  responseWaitMs: boundedInt('CHATGPT_RESUMER_RESPONSE_WAIT_MS', 360_000, 30_000, 900_000),
  cooldownMs:
    boundedInt('CHATGPT_RESUMER_RETRY_COOLDOWN_HOURS', 20, 1, 168) * 60 * 60 * 1_000,
  attemptWindowMs:
    boundedInt('CHATGPT_RESUMER_ATTEMPT_WINDOW_DAYS', 7, 1, 30) * 24 * 60 * 60 * 1_000,
  retentionMs:
    boundedInt('CHATGPT_RESUMER_STATE_RETENTION_DAYS', 45, 7, 365) * 24 * 60 * 60 * 1_000,
  lockStaleMs:
    boundedInt('CHATGPT_RESUMER_LOCK_STALE_MINUTES', 90, 70, 240) * 60 * 1_000,
  continuationPrompt:
    process.env.CHATGPT_RESUMER_CONTINUATION_PROMPT ?? DEFAULT_CONTINUATION_PROMPT,
  allowedDomainSuffixes: envList('CHATGPT_RESUMER_ALLOWED_DOMAIN_SUFFIXES', [
    'chatgpt.com',
    'openai.com',
    'oaistatic.com',
    'oaiusercontent.com',
  ]),
};

export function log(event, fields = {}) {
  process.stdout.write(
    `${JSON.stringify({
      timestamp: new Date().toISOString(),
      service: 'dd-chatgpt-rate-limit-resumer',
      run_id: RUN_ID,
      event,
      ...fields,
    })}\n`,
  );
}

export function errorCode(error) {
  const message = normalizeText(error?.message ?? error).toLowerCase();
  if (Number.isInteger(error?.exitCode) && error.exitCode === 78) return 'configuration_or_auth';
  if (Number.isInteger(error?.exitCode) && error.exitCode === 75) return 'concurrent_run';
  if (error?.name === 'TimeoutError' || message.includes('timeout')) return 'timeout';
  if (message.includes('composer')) return 'composer_unavailable';
  if (message.includes('acknowledge')) return 'submission_unconfirmed';
  if (message.includes('conversation links')) return 'sidebar_contract_changed';
  if (message.includes('authentication')) return 'authentication_unavailable';
  if (message.includes('storage state')) return 'storage_state_invalid';
  return 'automation_error';
}

function isAllowedHostname(hostname) {
  const host = hostname.toLowerCase().replace(/\.$/, '');
  return config.allowedDomainSuffixes.some(
    (suffix) => host === suffix || host.endsWith(`.${suffix}`),
  );
}

export async function installRequestBoundary(context) {
  let blockedRequests = 0;
  await context.route('**/*', async (route) => {
    const requestUrl = route.request().url();
    let url;
    try {
      url = new URL(requestUrl);
    } catch {
      blockedRequests += 1;
      await route.abort('blockedbyclient');
      return;
    }

    if (['about:', 'blob:', 'data:'].includes(url.protocol)) {
      await route.continue();
      return;
    }

    if (url.protocol !== 'https:' || !isAllowedHostname(url.hostname)) {
      blockedRequests += 1;
      await route.abort('blockedbyclient');
      return;
    }

    await route.continue();
  });

  await context.routeWebSocket('**/*', async (webSocket) => {
    let url;
    try {
      url = new URL(webSocket.url());
    } catch {
      blockedRequests += 1;
      await webSocket.close({ code: 1008, reason: 'invalid websocket URL' });
      return;
    }

    if (url.protocol !== 'wss:' || !isAllowedHostname(url.hostname)) {
      blockedRequests += 1;
      await webSocket.close({ code: 1008, reason: 'websocket blocked by policy' });
      return;
    }

    webSocket.connectToServer();
  });

  return () => blockedRequests;
}
