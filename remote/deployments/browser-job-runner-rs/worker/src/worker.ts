import { randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { connect, JSONCodec, type NatsConnection } from 'nats';
import { z } from 'zod';

import {
  chromium as playwrightChromium,
  type Browser as PlaywrightBrowser,
} from 'playwright';
import puppeteer, { type Browser as PuppeteerBrowser } from 'puppeteer';

// dd-browser-job-worker
//
// Sibling of dd-browser-test-server that runs ONE bounded Playwright/Puppeteer
// scenario per container. It has two mutually exclusive run modes, chosen at
// startup by whether JOB_SPEC_B64 is set:
//
//   serve mode (default, used by the dd-container-pool warm pool):
//     A tiny HTTP server exposes GET /healthz + POST /run. The pool keeps the
//     container warm, dispatches the scenario to /run, and reads the JSON
//     RunResult from the HTTP response (the pool bridges that back to NATS).
//     After one job the worker reports unhealthy and exits, so the pool retires
//     it and reconciles a fresh replacement — one clean browser per job.
//
//   one-shot mode (fallback, used when dd-browser-job-runner spawns us directly
//   via nerdctl because the pool is down):
//     We decode the scenario from JOB_SPEC_B64, run it once, publish the JSON
//     RunResult to NATS (per-job subject + shared fanout), then exit.
//
// In both modes a hard watchdog bounds a running job by BROWSER_JOB_MAX_MS,
// independent of the spawner's deadline kill, the pool request timeout, and the
// idle-reaper backstop.

type Engine = 'playwright' | 'puppeteer';

const config = {
  jobId: process.env.BROWSER_JOB_ID ?? randomUUID(),
  port: readNumberEnv('PORT', 8080),
  bodyLimitBytes: readNumberEnv('BROWSER_JOB_BODY_LIMIT_BYTES', 4_000_000),
  natsUrl: process.env.NATS_URL ?? 'nats://dd-nats.messaging.svc.cluster.local:4222',
  resultSubject: process.env.BROWSER_JOB_RESULT_SUBJECT ?? '',
  resultFanoutSubject: process.env.BROWSER_JOB_RESULT_FANOUT_SUBJECT ?? 'dd.remote.browser_jobs.results',
  eventsSubject: process.env.BROWSER_JOB_EVENTS_SUBJECT ?? '',
  maxMs: readNumberEnv('BROWSER_JOB_MAX_MS', 540_000),
  serveExitDelayMs: readNumberEnv('BROWSER_JOB_SERVE_EXIT_DELAY_MS', 500),
  headless: readBooleanEnv('BROWSER_JOB_HEADLESS', true),
  allowEvaluate: readBooleanEnv('BROWSER_JOB_ALLOW_EVALUATE', false),
  maxScreenshotBytes: readNumberEnv('BROWSER_JOB_MAX_SCREENSHOT_BYTES', 1_500_000),
  screenshotQuality: 70,
  defaultStepTimeoutMs: 15_000,
};

const StepBaseSchema = z.object({
  description: z.string().max(200).optional(),
  timeoutMs: z.number().int().min(100).max(300_000).optional(),
});

const StepSchema = z.discriminatedUnion('action', [
  StepBaseSchema.extend({
    action: z.literal('goto'),
    url: z.string().url(),
    waitUntil: z.enum(['load', 'domcontentloaded', 'networkidle']).optional(),
  }),
  StepBaseSchema.extend({ action: z.literal('click'), selector: z.string().min(1).max(800), nth: z.number().int().min(0).max(50).optional() }),
  StepBaseSchema.extend({ action: z.literal('fill'), selector: z.string().min(1).max(800), value: z.string().max(20_000) }),
  StepBaseSchema.extend({ action: z.literal('select'), selector: z.string().min(1).max(800), value: z.string().max(800) }),
  StepBaseSchema.extend({ action: z.literal('press'), selector: z.string().min(1).max(800).optional(), key: z.string().min(1).max(40) }),
  StepBaseSchema.extend({ action: z.literal('waitForSelector'), selector: z.string().min(1).max(800), state: z.enum(['attached', 'detached', 'visible', 'hidden']).optional() }),
  StepBaseSchema.extend({ action: z.literal('waitForUrl'), url: z.string().min(1).max(2000) }),
  StepBaseSchema.extend({ action: z.literal('waitForTimeout'), ms: z.number().int().min(0).max(60_000) }),
  StepBaseSchema.extend({ action: z.literal('extractText'), selector: z.string().min(1).max(800), name: z.string().min(1).max(120).optional() }),
  StepBaseSchema.extend({ action: z.literal('extractAttribute'), selector: z.string().min(1).max(800), attribute: z.string().min(1).max(120), name: z.string().min(1).max(120).optional() }),
  StepBaseSchema.extend({ action: z.literal('screenshot'), name: z.string().min(1).max(120).optional(), fullPage: z.boolean().optional() }),
  StepBaseSchema.extend({ action: z.literal('evaluate'), script: z.string().min(1).max(20_000), name: z.string().min(1).max(120).optional() }),
]);

type Step = z.infer<typeof StepSchema>;

const JobSpecSchema = z.object({
  jobId: z.string().min(1).max(200).optional(),
  requestId: z.string().min(1).max(200).nullish(),
  engine: z.enum(['playwright', 'puppeteer']).default('playwright'),
  url: z.string().url().nullish(),
  steps: z.array(StepSchema).min(1),
  timeoutMs: z.number().int().min(500).nullish(),
  viewport: z.object({ width: z.number().int().min(200).max(4000), height: z.number().int().min(200).max(4000) }).nullish(),
  userAgent: z.string().min(1).max(500).nullish(),
  extraHeaders: z.record(z.string(), z.string()).nullish(),
  captureFinalScreenshot: z.boolean().nullish(),
  failOnConsoleError: z.boolean().nullish(),
  maxMs: z.number().int().min(1000).nullish(),
});

type JobSpec = z.infer<typeof JobSpecSchema>;

type StepLogEntry = {
  index: number;
  action: Step['action'];
  status: 'ok' | 'error';
  durationMs: number;
  description?: string;
  error?: string;
};

type ConsoleLogEntry = { level: string; text: string; timestamp: string };

type ScreenshotPayload = {
  name: string;
  contentType: 'image/png' | 'image/jpeg';
  base64: string;
  bytes: number;
  truncated?: boolean;
};

type RunResult = {
  ok: boolean;
  jobId: string;
  requestId?: string;
  engine: Engine;
  durationMs: number;
  startedAt: string;
  finishedAt: string;
  finalUrl?: string;
  finalTitle?: string;
  steps: StepLogEntry[];
  extracted: Record<string, string>;
  screenshots: ScreenshotPayload[];
  consoleEntries: ConsoleLogEntry[];
  pageErrors: string[];
  error?: string;
};

interface ScenarioDriver {
  goto(url: string, waitUntil: 'load' | 'domcontentloaded' | 'networkidle' | undefined, timeoutMs: number): Promise<void>;
  click(selector: string, nth: number | undefined, timeoutMs: number): Promise<void>;
  fill(selector: string, value: string, timeoutMs: number): Promise<void>;
  select(selector: string, value: string, timeoutMs: number): Promise<void>;
  press(selector: string | undefined, key: string, timeoutMs: number): Promise<void>;
  waitForSelector(selector: string, state: 'attached' | 'detached' | 'visible' | 'hidden' | undefined, timeoutMs: number): Promise<void>;
  waitForUrl(pattern: string, timeoutMs: number): Promise<void>;
  waitForTimeout(ms: number): Promise<void>;
  extractText(selector: string, timeoutMs: number): Promise<string>;
  extractAttribute(selector: string, attribute: string, timeoutMs: number): Promise<string>;
  screenshot(name: string, fullPage: boolean): Promise<ScreenshotPayload | null>;
  evaluate(script: string, timeoutMs: number): Promise<unknown>;
  currentUrl(): Promise<string>;
  currentTitle(): Promise<string>;
  drainConsole(): ConsoleLogEntry[];
  drainPageErrors(): string[];
  close(): Promise<void>;
}

async function main(): Promise<void> {
  // JOB_SPEC_B64 is the marker for the direct nerdctl fallback path. When the
  // pool spawns us it injects PORT (and no spec), so we serve instead.
  if (process.env.JOB_SPEC_B64) {
    await runOneShot();
    return;
  }
  await runServe();
}

// one-shot mode: decode JOB_SPEC_B64, run once, publish to NATS, exit.
async function runOneShot(): Promise<void> {
  // Hard watchdog: never outlive the lifetime budget the spawner granted.
  const watchdog = armWatchdog();

  const startedAtIso = new Date().toISOString();
  const startedAtMs = Date.now();

  let spec: JobSpec | null = null;
  let parseError: string | null = null;
  try {
    const raw = process.env.JOB_SPEC_B64;
    if (!raw) throw new Error('JOB_SPEC_B64 is required');
    const decoded = Buffer.from(raw, 'base64').toString('utf8');
    spec = JobSpecSchema.parse(JSON.parse(decoded));
  } catch (error) {
    parseError = error instanceof Error ? error.message : String(error);
  }

  const engine: Engine = spec?.engine ?? 'playwright';
  const nats = await connectNats();

  if (!spec) {
    const result = failedResult(config.jobId, engine, startedAtIso, startedAtMs, parseError ?? 'invalid job spec');
    await publishResult(nats, result);
    await closeNats(nats);
    process.exit(0);
  }

  await publishEvent(nats, { kind: 'started', jobId: config.jobId, engine, atMs: Date.now() });

  const result = await runScenario(config.jobId, engine, spec, startedAtIso, startedAtMs);
  await publishResult(nats, result);
  await publishEvent(nats, {
    kind: 'finished',
    jobId: config.jobId,
    engine,
    ok: result.ok,
    durationMs: result.durationMs,
    atMs: Date.now(),
  });
  await closeNats(nats);
  clearTimeout(watchdog);
  process.exit(0);
}

// serve mode: warm HTTP worker managed by dd-container-pool. Handles exactly
// one /run, then reports unhealthy and exits so the pool replaces it.
async function runServe(): Promise<void> {
  let state: 'idle' | 'busy' | 'closing' = 'idle';

  const server = createServer((req, res) => {
    const path = (req.url ?? '/').split('?')[0];

    if (req.method === 'GET' && (path === '/healthz' || path === '/readyz')) {
      // Stay healthy only until we accept a job. After that we report 503 so the
      // pool retires this container instead of dispatching a second job to it.
      if (state === 'idle') return sendJson(res, 200, { ok: true, jobId: config.jobId, state });
      return sendJson(res, 503, { ok: false, jobId: config.jobId, state });
    }

    if (req.method === 'GET' && path === '/') {
      return sendJson(res, 200, { service: 'dd-browser-job-worker', mode: 'serve', jobId: config.jobId, state });
    }

    if (req.method === 'POST' && path === '/run') {
      if (state !== 'idle') {
        return sendJson(res, 409, { ok: false, error: 'worker already consumed (one job per container)' }, { connection: 'close' });
      }
      state = 'busy';
      void handleServeRun(req, res).finally(() => {
        state = 'closing';
        // Stop accepting new connections immediately. A dispatch that races in
        // after the pool returns us to idle then gets connection-refused, so the
        // pool retires us and reconciles a fresh worker instead of reusing one
        // that has already run its single job.
        server.close();
        scheduleExit(server);
      });
      return;
    }

    sendJson(res, 404, { ok: false, error: 'not found' });
  });

  server.listen(config.port, () => {
    console.log(`dd-browser-job-worker serve mode listening on :${config.port} (job ${config.jobId})`);
  });
}

async function handleServeRun(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const startedAtIso = new Date().toISOString();
  const startedAtMs = Date.now();
  const watchdog = armWatchdog();
  try {
    const raw = await readRequestBody(req, config.bodyLimitBytes);
    const spec = JobSpecSchema.parse(raw.length ? JSON.parse(raw) : {});
    const jobId = spec.jobId ?? config.jobId;
    const engine: Engine = spec.engine ?? 'playwright';
    const result = await runScenario(jobId, engine, spec, startedAtIso, startedAtMs);
    sendJson(res, result.ok ? 200 : 422, result, { connection: 'close' });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    sendJson(res, 400, failedResult(config.jobId, 'playwright', startedAtIso, startedAtMs, message), { connection: 'close' });
  } finally {
    clearTimeout(watchdog);
  }
}

function armWatchdog(): ReturnType<typeof setTimeout> {
  const watchdog = setTimeout(() => {
    console.error(`dd-browser-job-worker watchdog fired after ${config.maxMs}ms; exiting`);
    process.exit(2);
  }, config.maxMs);
  watchdog.unref?.();
  return watchdog;
}

function scheduleExit(server: ReturnType<typeof createServer>): void {
  setTimeout(() => {
    // The current response has flushed by now; drop any lingering keep-alive
    // sockets and exit so the pool reconciles a replacement.
    server.closeAllConnections?.();
    process.exit(0);
  }, config.serveExitDelayMs);
  // Hard backstop in case something keeps the loop alive.
  setTimeout(() => process.exit(0), config.serveExitDelayMs + 2000).unref?.();
}

function readRequestBody(req: IncomingMessage, limitBytes: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > limitBytes) {
        reject(new Error(`request body exceeds ${limitBytes} bytes`));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', (error) => reject(error));
  });
}

function sendJson(
  res: ServerResponse,
  status: number,
  body: unknown,
  extraHeaders: Record<string, string> = {},
): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(payload),
    ...extraHeaders,
  });
  res.end(payload);
}

async function runScenario(
  jobId: string,
  engine: Engine,
  input: JobSpec,
  startedAtIso: string,
  startedAtMs: number,
): Promise<RunResult> {
  const steps: StepLogEntry[] = [];
  const extracted: Record<string, string> = {};
  const screenshots: ScreenshotPayload[] = [];
  const consoleEntries: ConsoleLogEntry[] = [];
  const pageErrors: string[] = [];

  // Leave a few seconds of headroom under the hard lifetime so we can still
  // publish the result before the watchdog kills the process.
  const lifetimeBudget = Math.max(1000, config.maxMs - 4000);
  const overallTimeoutMs = Math.min(input.timeoutMs ?? lifetimeBudget, lifetimeBudget);
  const overallTimer = setTimeoutPromise(overallTimeoutMs).then(() => {
    throw new Error(`scenario exceeded overall timeout of ${overallTimeoutMs}ms`);
  });

  const work = (async (): Promise<{ finalUrl?: string; finalTitle?: string; ok: boolean; error?: string }> => {
    const driver = await openDriver(engine, input);
    try {
      const firstStep = input.steps[0];
      if (input.url && firstStep && firstStep.action !== 'goto') {
        await driver.goto(input.url, undefined, config.defaultStepTimeoutMs);
      }

      let stepIndex = 0;
      for (const step of input.steps) {
        const stepStart = Date.now();
        try {
          await runStep(driver, step, extracted, screenshots);
          steps.push({ index: stepIndex, action: step.action, status: 'ok', durationMs: Date.now() - stepStart, description: step.description });
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          steps.push({ index: stepIndex, action: step.action, status: 'error', durationMs: Date.now() - stepStart, description: step.description, error: message });
          return { ok: false, error: `step ${stepIndex} (${step.action}) failed: ${message}` };
        }
        stepIndex += 1;
      }

      if (input.captureFinalScreenshot ?? true) {
        try {
          const shot = await driver.screenshot('final', false);
          if (shot) screenshots.push(shot);
        } catch (error) {
          console.warn('final screenshot failed', error);
        }
      }

      const finalUrl = await driver.currentUrl().catch(() => undefined);
      const finalTitle = await driver.currentTitle().catch(() => undefined);
      consoleEntries.push(...driver.drainConsole());
      pageErrors.push(...driver.drainPageErrors());

      if ((input.failOnConsoleError ?? false) && consoleEntries.some((entry) => entry.level === 'error')) {
        return { ok: false, finalUrl, finalTitle, error: 'failOnConsoleError: page emitted at least one console error' };
      }
      return { ok: true, finalUrl, finalTitle };
    } finally {
      await driver.close().catch(() => undefined);
    }
  })();

  let outcome: { finalUrl?: string; finalTitle?: string; ok: boolean; error?: string };
  try {
    outcome = await Promise.race([work, overallTimer]);
  } catch (error) {
    outcome = { ok: false, error: error instanceof Error ? error.message : String(error) };
  }

  return {
    ok: outcome.ok,
    jobId,
    requestId: input.requestId ?? undefined,
    engine,
    durationMs: Date.now() - startedAtMs,
    startedAt: startedAtIso,
    finishedAt: new Date().toISOString(),
    finalUrl: outcome.finalUrl,
    finalTitle: outcome.finalTitle,
    steps,
    extracted,
    screenshots,
    consoleEntries,
    pageErrors,
    error: outcome.error,
  };
}

async function openDriver(engine: Engine, input: JobSpec): Promise<ScenarioDriver> {
  return engine === 'puppeteer' ? openPuppeteerDriver(input) : openPlaywrightDriver(input);
}

async function runStep(
  driver: ScenarioDriver,
  step: Step,
  extracted: Record<string, string>,
  screenshots: ScreenshotPayload[],
): Promise<void> {
  const timeoutMs = step.timeoutMs ?? config.defaultStepTimeoutMs;
  switch (step.action) {
    case 'goto':
      await driver.goto(step.url, step.waitUntil, timeoutMs);
      return;
    case 'click':
      await driver.click(step.selector, step.nth, timeoutMs);
      return;
    case 'fill':
      await driver.fill(step.selector, step.value, timeoutMs);
      return;
    case 'select':
      await driver.select(step.selector, step.value, timeoutMs);
      return;
    case 'press':
      await driver.press(step.selector, step.key, timeoutMs);
      return;
    case 'waitForSelector':
      await driver.waitForSelector(step.selector, step.state, timeoutMs);
      return;
    case 'waitForUrl':
      await driver.waitForUrl(step.url, timeoutMs);
      return;
    case 'waitForTimeout':
      await driver.waitForTimeout(step.ms);
      return;
    case 'extractText': {
      const value = await driver.extractText(step.selector, timeoutMs);
      extracted[step.name ?? `text:${step.selector}`] = value;
      return;
    }
    case 'extractAttribute': {
      const value = await driver.extractAttribute(step.selector, step.attribute, timeoutMs);
      extracted[step.name ?? `attr:${step.selector}@${step.attribute}`] = value;
      return;
    }
    case 'screenshot': {
      const shot = await driver.screenshot(step.name ?? `step-${Date.now()}`, step.fullPage ?? false);
      if (shot) screenshots.push(shot);
      return;
    }
    case 'evaluate': {
      if (!config.allowEvaluate) {
        throw new Error('evaluate steps are disabled (set BROWSER_JOB_ALLOW_EVALUATE=true to enable)');
      }
      const value = await driver.evaluate(step.script, timeoutMs);
      extracted[step.name ?? 'evaluate'] = stringifyEvaluateResult(value);
      return;
    }
  }
}

async function openPlaywrightDriver(input: JobSpec): Promise<ScenarioDriver> {
  const browser: PlaywrightBrowser = await playwrightChromium.launch({
    headless: config.headless,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  });
  const context = await browser.newContext({
    viewport: input.viewport ?? { width: 1280, height: 800 },
    userAgent: input.userAgent ?? undefined,
    extraHTTPHeaders: input.extraHeaders ?? undefined,
  });
  const page = await context.newPage();
  const consoleLog: ConsoleLogEntry[] = [];
  const errors: string[] = [];
  page.on('console', (msg) => consoleLog.push({ level: msg.type(), text: msg.text(), timestamp: new Date().toISOString() }));
  page.on('pageerror', (err) => errors.push(err.message));

  return {
    goto: async (url, waitUntil, timeoutMs) => { await page.goto(url, { waitUntil: waitUntil ?? 'load', timeout: timeoutMs }); },
    click: async (selector, nth, timeoutMs) => {
      const locator = nth !== undefined ? page.locator(selector).nth(nth) : page.locator(selector);
      await locator.click({ timeout: timeoutMs });
    },
    fill: async (selector, value, timeoutMs) => { await page.fill(selector, value, { timeout: timeoutMs }); },
    select: async (selector, value, timeoutMs) => { await page.selectOption(selector, value, { timeout: timeoutMs }); },
    press: async (selector, key, timeoutMs) => {
      if (selector) await page.press(selector, key, { timeout: timeoutMs });
      else await page.keyboard.press(key);
    },
    waitForSelector: async (selector, state, timeoutMs) => { await page.waitForSelector(selector, { state: state ?? 'visible', timeout: timeoutMs }); },
    waitForUrl: async (urlPattern, timeoutMs) => { await page.waitForURL(urlPattern, { timeout: timeoutMs }); },
    waitForTimeout: async (ms) => { await page.waitForTimeout(ms); },
    extractText: async (selector, timeoutMs) => {
      const handle = await page.waitForSelector(selector, { state: 'attached', timeout: timeoutMs });
      return ((await handle.textContent()) ?? '').trim();
    },
    extractAttribute: async (selector, attribute, timeoutMs) => {
      const handle = await page.waitForSelector(selector, { state: 'attached', timeout: timeoutMs });
      return (await handle.getAttribute(attribute)) ?? '';
    },
    screenshot: async (name, fullPage) => {
      const buffer = await page.screenshot({ type: 'jpeg', quality: config.screenshotQuality, fullPage });
      return clampScreenshot(name, 'image/jpeg', buffer);
    },
    evaluate: async (script) => page.evaluate(`(function(){ ${script} })()`),
    currentUrl: async () => page.url(),
    currentTitle: async () => page.title(),
    drainConsole: () => consoleLog.splice(0),
    drainPageErrors: () => errors.splice(0),
    close: async () => { await browser.close(); },
  } satisfies ScenarioDriver;
}

async function openPuppeteerDriver(input: JobSpec): Promise<ScenarioDriver> {
  const browser: PuppeteerBrowser = await puppeteer.launch({
    headless: config.headless,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
    executablePath: playwrightChromium.executablePath(),
  });
  const page = await browser.newPage();
  if (input.viewport) await page.setViewport(input.viewport);
  if (input.userAgent) await page.setUserAgent(input.userAgent);
  if (input.extraHeaders) await page.setExtraHTTPHeaders(input.extraHeaders);

  const consoleLog: ConsoleLogEntry[] = [];
  const errors: string[] = [];
  page.on('console', (msg) => consoleLog.push({ level: msg.type(), text: msg.text(), timestamp: new Date().toISOString() }));
  page.on('pageerror', (err: unknown) => errors.push(err instanceof Error ? err.message : String(err)));

  const elementByNth = async (selector: string, nth: number | undefined) => {
    const handles = await page.$$(selector);
    const handle = handles[nth ?? 0];
    if (!handle) throw new Error(`puppeteer: selector ${selector} did not match index ${nth ?? 0}`);
    return handle;
  };

  return {
    goto: async (url, waitUntil, timeoutMs) => { await page.goto(url, { waitUntil: mapPuppeteerWaitUntil(waitUntil), timeout: timeoutMs }); },
    click: async (selector, nth, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs, visible: true });
      const handle = await elementByNth(selector, nth);
      await handle.click();
      await handle.dispose();
    },
    fill: async (selector, value, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs, visible: true });
      const element = await page.$(selector);
      if (!element) throw new Error(`puppeteer: selector ${selector} not found`);
      await element.evaluate((node) => {
        if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) node.value = '';
      });
      await element.type(value);
      await element.dispose();
    },
    select: async (selector, value, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs });
      await page.select(selector, value);
    },
    press: async (selector, key, timeoutMs) => {
      if (selector) {
        await page.waitForSelector(selector, { timeout: timeoutMs });
        await page.focus(selector);
      }
      await page.keyboard.press(key as Parameters<typeof page.keyboard.press>[0]);
    },
    waitForSelector: async (selector, state, timeoutMs) => {
      const visible = state === undefined ? true : state === 'visible';
      const hidden = state === 'hidden' || state === 'detached';
      await page.waitForSelector(selector, { timeout: timeoutMs, visible, hidden });
    },
    waitForUrl: async (urlPattern, timeoutMs) => {
      await page.waitForFunction(
        (pattern: string, current: string) => {
          if (pattern.startsWith('/') && pattern.endsWith('/')) return new RegExp(pattern.slice(1, -1)).test(current);
          return current === pattern || current.includes(pattern);
        },
        { timeout: timeoutMs },
        urlPattern,
        page.url(),
      );
    },
    waitForTimeout: async (ms) => { await new Promise((resolve) => setTimeout(resolve, ms)); },
    extractText: async (selector, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs });
      return page.$eval(selector, (node) => (node.textContent ?? '').trim());
    },
    extractAttribute: async (selector, attribute, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs });
      return (await page.$eval(selector, (node, attr) => (node as Element).getAttribute(attr) ?? '', attribute)) as string;
    },
    screenshot: async (name, fullPage) => {
      const buffer = await page.screenshot({ type: 'jpeg', quality: config.screenshotQuality, fullPage });
      return clampScreenshot(name, 'image/jpeg', Buffer.from(buffer));
    },
    evaluate: async (script) => page.evaluate(`(function(){ ${script} })()`),
    currentUrl: async () => page.url(),
    currentTitle: async () => page.title(),
    drainConsole: () => consoleLog.splice(0),
    drainPageErrors: () => errors.splice(0),
    close: async () => { await browser.close(); },
  } satisfies ScenarioDriver;
}

function mapPuppeteerWaitUntil(
  waitUntil: 'load' | 'domcontentloaded' | 'networkidle' | undefined,
): 'load' | 'domcontentloaded' | 'networkidle0' | undefined {
  if (waitUntil === 'networkidle') return 'networkidle0';
  return waitUntil;
}

function clampScreenshot(name: string, contentType: 'image/png' | 'image/jpeg', buffer: Buffer): ScreenshotPayload {
  const truncated = buffer.byteLength > config.maxScreenshotBytes;
  const trimmed = truncated ? buffer.subarray(0, config.maxScreenshotBytes) : buffer;
  return { name, contentType, base64: trimmed.toString('base64'), bytes: buffer.byteLength, ...(truncated ? { truncated: true } : {}) };
}

function stringifyEvaluateResult(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function failedResult(jobId: string, engine: Engine, startedAtIso: string, startedAtMs: number, error: string): RunResult {
  return {
    ok: false,
    jobId,
    engine,
    durationMs: Date.now() - startedAtMs,
    startedAt: startedAtIso,
    finishedAt: new Date().toISOString(),
    steps: [],
    extracted: {},
    screenshots: [],
    consoleEntries: [],
    pageErrors: [],
    error,
  };
}

async function connectNats(): Promise<NatsConnection | null> {
  try {
    return await connect({ servers: config.natsUrl, name: `dd-browser-job-${config.jobId}`, maxReconnectAttempts: 5 });
  } catch (error) {
    console.error('dd-browser-job-worker could not connect to NATS:', error);
    return null;
  }
}

async function publishResult(nats: NatsConnection | null, result: RunResult): Promise<void> {
  if (!nats) {
    console.error('dd-browser-job-worker has no NATS connection; result dropped', JSON.stringify({ jobId: result.jobId, ok: result.ok }));
    return;
  }
  const codec = JSONCodec<RunResult>();
  const payload = codec.encode(result);
  try {
    if (config.resultSubject) nats.publish(config.resultSubject, payload);
    if (config.resultFanoutSubject) nats.publish(config.resultFanoutSubject, payload);
    await nats.flush();
  } catch (error) {
    console.error('dd-browser-job-worker result publish failed:', error);
  }
}

async function publishEvent(nats: NatsConnection | null, event: Record<string, unknown>): Promise<void> {
  if (!nats || !config.eventsSubject) return;
  try {
    nats.publish(config.eventsSubject, JSONCodec().encode({ type: 'browser-job-event', source: 'dd-browser-job-worker', ...event }));
    await nats.flush();
  } catch (error) {
    console.error('dd-browser-job-worker event publish failed:', error);
  }
}

async function closeNats(nats: NatsConnection | null): Promise<void> {
  if (!nats) return;
  try {
    await nats.drain();
  } catch {
    // best effort
  }
}

function readNumberEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const value = Number(raw);
  return Number.isFinite(value) ? value : fallback;
}

function readBooleanEnv(name: string, fallback: boolean): boolean {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  return raw === 'true' || raw === '1' || raw === 'yes';
}

function setTimeoutPromise(ms: number): Promise<never> {
  return new Promise((_resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout after ${ms}ms`)), ms);
    timer.unref?.();
  });
}

main().catch((error) => {
  console.error('dd-browser-job-worker fatal error:', error);
  process.exit(1);
});
