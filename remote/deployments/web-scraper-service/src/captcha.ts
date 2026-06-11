/**
 * CAPTCHA orchestration: detect a challenge in fetched HTML, hand the sitekey to
 * an external solving provider, and produce the browser-side injection needed to
 * apply the returned token. The solver client speaks the 2captcha in.php/res.php
 * protocol, which CapSolver, CapMonster, and Anti-Captcha all expose a compatible
 * surface for, so the provider is swappable via `providerUrl`.
 */

export type CaptchaType =
  | 'recaptcha-v2'
  | 'recaptcha-v3'
  | 'hcaptcha'
  | 'turnstile'
  | 'cloudflare-challenge';

/** CAPTCHA kinds we can request a token for from the solver. */
export const SOLVABLE_CAPTCHA_TYPES: ReadonlySet<CaptchaType> = new Set([
  'recaptcha-v2',
  'recaptcha-v3',
  'hcaptcha',
  'turnstile',
]);

export type CaptchaDetection = {
  detected: boolean;
  type: CaptchaType | null;
  sitekey: string | null;
  action: string | null;
  signals: string[];
};

const NOT_DETECTED: CaptchaDetection = {
  detected: false,
  type: null,
  sitekey: null,
  action: null,
  signals: [],
};

export function detectCaptcha(html: string): CaptchaDetection {
  if (!html) {
    return NOT_DETECTED;
  }
  // Each branch's regexes contain adjacent unbounded `[^"']*` runs that backtrack
  // quadratically when their marker is absent. Gate every branch behind a cheap
  // lowercase substring check so the regexes only run when the marker is present,
  // keeping detection linear on the common (no-CAPTCHA) path for large documents.
  const hay = html.toLowerCase();
  const signals: string[] = [];

  // Cloudflare Turnstile.
  if (hay.includes('cf-turnstile') || hay.includes('challenges.cloudflare.com/turnstile')) {
    const turnstileKey =
      matchAttr(html, /class=["'][^"']*\bcf-turnstile\b[^"']*["'][^>]*\bdata-sitekey=["']([^"']+)["']/i) ??
      matchAttr(html, /\bdata-sitekey=["']([^"']+)["'][^>]*class=["'][^"']*\bcf-turnstile\b/i);
    if (turnstileKey || hay.includes('challenges.cloudflare.com/turnstile')) {
      signals.push(turnstileKey ? 'cf-turnstile sitekey' : 'challenges.cloudflare.com/turnstile script');
      return { detected: true, type: 'turnstile', sitekey: turnstileKey, action: null, signals };
    }
  }

  // hCaptcha.
  if (hay.includes('h-captcha') || hay.includes('hcaptcha.com/1/api.js')) {
    const hcaptchaKey =
      matchAttr(html, /class=["'][^"']*\bh-captcha\b[^"']*["'][^>]*\bdata-sitekey=["']([^"']+)["']/i) ??
      matchAttr(html, /\bdata-sitekey=["']([^"']+)["'][^>]*class=["'][^"']*\bh-captcha\b/i);
    if (hcaptchaKey || hay.includes('hcaptcha.com/1/api.js')) {
      signals.push(hcaptchaKey ? 'h-captcha sitekey' : 'hcaptcha.com/1/api.js script');
      return { detected: true, type: 'hcaptcha', sitekey: hcaptchaKey, action: null, signals };
    }
  }

  // reCAPTCHA (v2 explicit widget, or v3 via render=<sitekey>).
  if (hay.includes('g-recaptcha') || hay.includes('recaptcha/api.js')) {
    const recaptchaV2Key =
      matchAttr(html, /class=["'][^"']*\bg-recaptcha\b[^"']*["'][^>]*\bdata-sitekey=["']([^"']+)["']/i) ??
      matchAttr(html, /\bdata-sitekey=["']([^"']+)["'][^>]*class=["'][^"']*\bg-recaptcha\b/i);
    if (recaptchaV2Key) {
      signals.push('g-recaptcha sitekey');
      return { detected: true, type: 'recaptcha-v2', sitekey: recaptchaV2Key, action: null, signals };
    }
    const recaptchaV3Key = matchAttr(
      html,
      /recaptcha\/api\.js\?[^"']*\brender=([0-9A-Za-z_-]{20,})/i,
    );
    if (recaptchaV3Key && recaptchaV3Key !== 'explicit' && recaptchaV3Key !== 'onload') {
      signals.push('recaptcha v3 render sitekey');
      return { detected: true, type: 'recaptcha-v3', sitekey: recaptchaV3Key, action: null, signals };
    }
    if (hay.includes('www.google.com/recaptcha/api.js') || hay.includes('g-recaptcha')) {
      signals.push('recaptcha script without inline sitekey');
      return { detected: true, type: 'recaptcha-v2', sitekey: null, action: null, signals };
    }
  }

  // Cloudflare interstitial / managed challenge (no solvable sitekey on its own).
  if (
    /<title>\s*just a moment/i.test(hay) ||
    hay.includes('cf-browser-verification') ||
    hay.includes('cf_chl_opt') ||
    hay.includes('__cf_chl_') ||
    hay.includes('cdn-cgi/challenge-platform')
  ) {
    signals.push('cloudflare interstitial markers');
    return {
      detected: true,
      type: 'cloudflare-challenge',
      sitekey: null,
      action: null,
      signals,
    };
  }

  return NOT_DETECTED;
}

function matchAttr(html: string, pattern: RegExp): string | null {
  const match = pattern.exec(html);
  return match?.[1] ?? null;
}

export type CaptchaSolverConfig = {
  providerUrl: string;
  apiKey: string;
  pollIntervalMs: number;
  timeoutMs: number;
};

export type CaptchaSolveResult = {
  token: string;
  provider: string;
  taskId: string;
  solveMs: number;
};

export class CaptchaSolveError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CaptchaSolveError';
  }
}

/**
 * Request a token from the solver for a detected challenge. Throws
 * `CaptchaSolveError` on provider/timeout failures so the caller can decide
 * whether to surface the original page or fail the scrape.
 */
export async function solveCaptcha(params: {
  config: CaptchaSolverConfig;
  detection: CaptchaDetection;
  pageUrl: string;
  userAgent?: string;
}): Promise<CaptchaSolveResult> {
  const { config, detection, pageUrl, userAgent } = params;
  if (!detection.type || !SOLVABLE_CAPTCHA_TYPES.has(detection.type)) {
    throw new CaptchaSolveError(`captcha type ${detection.type ?? 'unknown'} is not solvable`);
  }
  if (!detection.sitekey) {
    throw new CaptchaSolveError(`no sitekey detected for ${detection.type}`);
  }

  const provider = new URL(config.providerUrl);
  const startedAt = Date.now();
  const taskId = await createTask(provider, config.apiKey, detection, pageUrl, userAgent);
  const token = await pollTask(provider, config.apiKey, taskId, config);
  return {
    token,
    provider: provider.host,
    taskId,
    solveMs: Date.now() - startedAt,
  };
}

async function createTask(
  provider: URL,
  apiKey: string,
  detection: CaptchaDetection,
  pageUrl: string,
  userAgent?: string,
): Promise<string> {
  const submit = new URL('in.php', ensureTrailingSlash(provider));
  submit.searchParams.set('key', apiKey);
  submit.searchParams.set('json', '1');
  submit.searchParams.set('pageurl', pageUrl);
  submit.searchParams.set('method', methodFor(detection.type!));
  submit.searchParams.set('sitekey', detection.sitekey!);
  // 2captcha historically keys reCAPTCHA off `googlekey`; send both for compat.
  if (detection.type === 'recaptcha-v2' || detection.type === 'recaptcha-v3') {
    submit.searchParams.set('googlekey', detection.sitekey!);
  }
  if (detection.type === 'recaptcha-v3') {
    submit.searchParams.set('version', 'v3');
    submit.searchParams.set('action', detection.action ?? 'verify');
    submit.searchParams.set('min_score', '0.3');
  }
  if (userAgent) {
    submit.searchParams.set('userAgent', userAgent);
  }

  const body = await postForJson(submit);
  if (String(body.status) !== '1') {
    throw new CaptchaSolveError(`solver rejected task: ${body.request ?? 'unknown error'}`);
  }
  return String(body.request);
}

async function pollTask(
  provider: URL,
  apiKey: string,
  taskId: string,
  config: CaptchaSolverConfig,
): Promise<string> {
  const result = new URL('res.php', ensureTrailingSlash(provider));
  result.searchParams.set('key', apiKey);
  result.searchParams.set('action', 'get');
  result.searchParams.set('id', taskId);
  result.searchParams.set('json', '1');

  const deadline = Date.now() + config.timeoutMs;
  while (Date.now() < deadline) {
    await delay(config.pollIntervalMs);
    const body = await postForJson(result, 'GET');
    if (String(body.status) === '1') {
      return String(body.request);
    }
    const request = String(body.request ?? '');
    if (request && request !== 'CAPCHA_NOT_READY' && request !== 'CAPTCHA_NOT_READY') {
      throw new CaptchaSolveError(`solver failed task ${taskId}: ${request}`);
    }
  }
  throw new CaptchaSolveError(`solver timed out after ${config.timeoutMs}ms for task ${taskId}`);
}

type SolverResponse = { status?: number | string; request?: string };

async function postForJson(url: URL, method: 'GET' | 'POST' = 'POST'): Promise<SolverResponse> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetch(url, { method, signal: controller.signal });
    if (!response.ok) {
      throw new CaptchaSolveError(`solver HTTP ${response.status}`);
    }
    return (await response.json()) as SolverResponse;
  } catch (error) {
    if (error instanceof CaptchaSolveError) {
      throw error;
    }
    throw new CaptchaSolveError(
      `solver request failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  } finally {
    clearTimeout(timeout);
  }
}

function methodFor(type: CaptchaType): string {
  switch (type) {
    case 'hcaptcha':
      return 'hcaptcha';
    case 'turnstile':
      return 'turnstile';
    case 'recaptcha-v2':
    case 'recaptcha-v3':
      return 'userrecaptcha';
    default:
      return 'userrecaptcha';
  }
}

/**
 * Browser-side script (stringified) that writes the solved token into the
 * standard response field for the challenge and fires the widget callback when
 * one is registered. Best-effort: pages with custom callbacks may still need a
 * manual form submit, which the caller can trigger separately.
 */
export function buildInjectionScript(type: CaptchaType, token: string): string {
  const json = JSON.stringify(token);
  const fields: Record<CaptchaType, string[]> = {
    'recaptcha-v2': ['g-recaptcha-response'],
    'recaptcha-v3': ['g-recaptcha-response'],
    hcaptcha: ['h-captcha-response', 'g-recaptcha-response'],
    turnstile: ['cf-turnstile-response'],
    'cloudflare-challenge': [],
  };
  const names = fields[type] ?? [];
  return `(() => {
  const token = ${json};
  const names = ${JSON.stringify(names)};
  for (const name of names) {
    for (const el of document.querySelectorAll('[name="' + name + '"], #' + name)) {
      try { el.value = token; } catch (_) {}
    }
    if (!document.querySelector('[name="' + name + '"]')) {
      const ta = document.createElement('textarea');
      ta.name = name; ta.style.display = 'none'; ta.value = token;
      (document.forms[0] || document.body).appendChild(ta);
    }
  }
  try {
    const cfg = window.___grecaptcha_cfg;
    if (cfg && cfg.clients) {
      for (const client of Object.values(cfg.clients)) {
        for (const root of Object.values(client || {})) {
          for (const leaf of Object.values(root || {})) {
            if (leaf && typeof leaf.callback === 'function') { try { leaf.callback(token); } catch (_) {} }
          }
        }
      }
    }
  } catch (_) {}
  return true;
})();`;
}

function ensureTrailingSlash(url: URL): URL {
  if (!url.pathname.endsWith('/')) {
    return new URL(`${url.pathname}/${url.search}`, url);
  }
  return url;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
