import Fastify from 'fastify';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { createRequire } from 'node:module';
import { z } from 'zod';

const requireFromCwd = createRequire(import.meta.url);

import {
  chromium as playwrightChromium,
  type Browser as PlaywrightBrowser,
} from 'playwright';
import puppeteer, { type Browser as PuppeteerBrowser } from 'puppeteer';
import {
  Builder as SeleniumBuilder,
  By as SeleniumBy,
  until as seleniumUntil,
  type WebDriver,
  type WebElement,
} from 'selenium-webdriver';
import { Options as ChromeOptions } from 'selenium-webdriver/chrome.js';

// dd-browser-test-server
//
// Long-running Fastify service that runs Playwright, Puppeteer, and Selenium
// scenarios on demand from inside the cluster. The intended consumer is the
// remote test harness — operators POST a scenario describing a sequence of
// steps and receive structured results (logs, screenshots, extracted text).
//
// Goals:
// - Single binary that exposes all three drivers behind one HTTP API.
// - Bounded scenario DSL (no arbitrary script eval by default) so accidental
//   misuse cannot exfiltrate cluster secrets.
// - Same auth and observability shape as dd-web-scraper (SERVER_AUTH_SECRET,
//   /healthz, /metrics, /status).

type Tool = 'playwright' | 'puppeteer' | 'selenium';

const TOOLS = ['playwright', 'puppeteer', 'selenium'] as const;

const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();

const config = {
  host: process.env.HOST ?? '0.0.0.0',
  port: readNumberEnv('PORT', 8104),
  serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
  allowUnauthenticated: process.env.BROWSER_TEST_ALLOW_UNAUTHENTICATED === 'true',
  defaultTool: normalizeTool(process.env.BROWSER_TEST_DEFAULT_TOOL ?? 'playwright'),
  maxConcurrent: readNumberEnv('BROWSER_TEST_MAX_CONCURRENT', 2),
  defaultTimeoutMs: readNumberEnv('BROWSER_TEST_DEFAULT_TIMEOUT_MS', 30_000),
  maxTimeoutMs: readNumberEnv('BROWSER_TEST_MAX_TIMEOUT_MS', 180_000),
  defaultStepTimeoutMs: readNumberEnv('BROWSER_TEST_STEP_TIMEOUT_MS', 15_000),
  maxSteps: readNumberEnv('BROWSER_TEST_MAX_STEPS', 64),
  maxScreenshotBytes: readNumberEnv('BROWSER_TEST_MAX_SCREENSHOT_BYTES', 1_500_000),
  screenshotQuality: clampNumber(readNumberEnv('BROWSER_TEST_SCREENSHOT_QUALITY', 70), 1, 100),
  browserHeadless: readBooleanEnv('BROWSER_TEST_HEADLESS', true),
  // evaluate / arbitrary script execution must be opt-in, since this service
  // sits behind the gateway and a stolen auth header should not be a remote
  // code-execution primitive.
  allowEvaluate: readBooleanEnv('BROWSER_TEST_ALLOW_EVALUATE', false),
  // Optional override for a Chromium binary. Defaults to the Playwright
  // bundled Chromium (selected at runtime by playwrightChromium.executablePath()).
  chromiumExecutablePath: process.env.BROWSER_TEST_CHROMIUM_PATH ?? null,
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
  StepBaseSchema.extend({
    action: z.literal('click'),
    selector: z.string().min(1).max(800),
    nth: z.number().int().min(0).max(50).optional(),
  }),
  StepBaseSchema.extend({
    action: z.literal('fill'),
    selector: z.string().min(1).max(800),
    value: z.string().max(20_000),
  }),
  StepBaseSchema.extend({
    action: z.literal('select'),
    selector: z.string().min(1).max(800),
    value: z.string().max(800),
  }),
  StepBaseSchema.extend({
    action: z.literal('press'),
    selector: z.string().min(1).max(800).optional(),
    key: z.string().min(1).max(40),
  }),
  StepBaseSchema.extend({
    action: z.literal('waitForSelector'),
    selector: z.string().min(1).max(800),
    state: z.enum(['attached', 'detached', 'visible', 'hidden']).optional(),
  }),
  StepBaseSchema.extend({
    action: z.literal('waitForUrl'),
    url: z.string().min(1).max(2000),
  }),
  StepBaseSchema.extend({
    action: z.literal('waitForTimeout'),
    ms: z.number().int().min(0).max(60_000),
  }),
  StepBaseSchema.extend({
    action: z.literal('extractText'),
    selector: z.string().min(1).max(800),
    name: z.string().min(1).max(120).optional(),
  }),
  StepBaseSchema.extend({
    action: z.literal('extractAttribute'),
    selector: z.string().min(1).max(800),
    attribute: z.string().min(1).max(120),
    name: z.string().min(1).max(120).optional(),
  }),
  StepBaseSchema.extend({
    action: z.literal('screenshot'),
    name: z.string().min(1).max(120).optional(),
    fullPage: z.boolean().optional(),
  }),
  StepBaseSchema.extend({
    action: z.literal('evaluate'),
    script: z.string().min(1).max(20_000),
    name: z.string().min(1).max(120).optional(),
  }),
]);

type Step = z.infer<typeof StepSchema>;

const RunRequestSchema = z.object({
  requestId: z.string().min(1).max(120).optional(),
  tool: z.enum(TOOLS).optional(),
  url: z.string().url().optional(),
  steps: z.array(StepSchema).min(1).max(config.maxSteps),
  timeoutMs: z.number().int().min(500).max(config.maxTimeoutMs).optional(),
  viewport: z
    .object({
      width: z.number().int().min(200).max(4000),
      height: z.number().int().min(200).max(4000),
    })
    .optional(),
  userAgent: z.string().min(1).max(500).optional(),
  extraHeaders: z.record(z.string().min(1).max(120), z.string().max(2000)).optional(),
  captureFinalScreenshot: z.boolean().optional(),
  failOnConsoleError: z.boolean().optional(),
});

type RunRequest = z.infer<typeof RunRequestSchema>;

type StepLogEntry = {
  index: number;
  action: Step['action'];
  status: 'ok' | 'error';
  durationMs: number;
  description?: string;
  error?: string;
};

type ConsoleLogEntry = {
  level: string;
  text: string;
  timestamp: string;
};

type ScreenshotPayload = {
  name: string;
  contentType: 'image/png' | 'image/jpeg';
  base64: string;
  bytes: number;
  truncated?: boolean;
};

type RunResult = {
  ok: boolean;
  requestId: string;
  tool: Tool;
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

const metrics = {
  inFlight: 0,
  total: new Map<string, number>(),
  durationSumMs: new Map<Tool, number>(),
  durationCount: new Map<Tool, number>(),
};

let playwrightBrowser: PlaywrightBrowser | null = null;
let playwrightBrowserPromise: Promise<PlaywrightBrowser> | null = null;
let puppeteerBrowser: PuppeteerBrowser | null = null;
let puppeteerBrowserPromise: Promise<PuppeteerBrowser> | null = null;

const fastify = Fastify({
  logger: true,
  bodyLimit: 2_097_152,
});

fastify.addHook('onRequest', async (request, reply) => {
  const path = request.url.split('?')[0] ?? request.url;
  if (request.method !== 'POST') return;
  if (path !== '/run') return;
  if (isAuthorized(request.headers)) return;
  return reply.code(401).send({ ok: false, error: 'unauthorized' });
});

fastify.get('/', async () => serviceDescriptor());
fastify.get('/browser-test', async () => serviceDescriptor());
fastify.get('/tools', async () => toolsDescriptor());
fastify.get('/browser-test/tools', async () => toolsDescriptor());
fastify.get('/status', async () => statusDescriptor());
fastify.get('/browser-test/status', async () => statusDescriptor());
fastify.get('/healthz', async () => healthDescriptor());
fastify.get('/browser-test/healthz', async () => healthDescriptor());
fastify.get('/metrics', async (_request, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});
fastify.get('/browser-test/metrics', async (_request, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});

fastify.post('/run', async (request, reply) => {
  const parsed = RunRequestSchema.safeParse(request.body);
  if (!parsed.success) {
    return reply.code(400).send({ ok: false, error: parsed.error.format() });
  }

  if (metrics.inFlight >= config.maxConcurrent) {
    return reply.code(429).send({
      ok: false,
      error: 'browser-test concurrency limit reached',
      maxConcurrent: config.maxConcurrent,
    });
  }

  const tool: Tool = parsed.data.tool ?? config.defaultTool;
  const requestId = parsed.data.requestId ?? randomUUID();
  const startedAtIso = new Date().toISOString();
  const startedAtMs = Date.now();
  metrics.inFlight += 1;

  try {
    const result = await runScenario(tool, parsed.data, requestId, startedAtIso);
    recordMetric(tool, result.ok ? 'ok' : 'error', result.durationMs);
    if (!result.ok) {
      return reply.code(422).send(result);
    }
    return result;
  } catch (error) {
    const durationMs = Date.now() - startedAtMs;
    recordMetric(tool, 'error', durationMs);
    const message = error instanceof Error ? error.message : String(error);
    return reply.code(500).send({
      ok: false,
      requestId,
      tool,
      durationMs,
      startedAt: startedAtIso,
      finishedAt: new Date().toISOString(),
      steps: [],
      extracted: {},
      screenshots: [],
      consoleEntries: [],
      pageErrors: [],
      error: message,
    });
  } finally {
    metrics.inFlight -= 1;
  }
});

async function runScenario(
  tool: Tool,
  input: RunRequest,
  requestId: string,
  startedAtIso: string,
): Promise<RunResult> {
  const startedAtMs = Date.now();
  const steps: StepLogEntry[] = [];
  const extracted: Record<string, string> = {};
  const screenshots: ScreenshotPayload[] = [];
  const consoleEntries: ConsoleLogEntry[] = [];
  const pageErrors: string[] = [];

  const overallTimeoutMs = clampNumber(
    input.timeoutMs ?? config.defaultTimeoutMs,
    500,
    config.maxTimeoutMs,
  );
  const overallTimer = setTimeoutPromise(overallTimeoutMs).then(() => {
    throw new Error(`scenario exceeded overall timeout of ${overallTimeoutMs}ms`);
  });

  const work = (async (): Promise<{ finalUrl?: string; finalTitle?: string; ok: boolean; error?: string }> => {
    const driver = await openDriver(tool, input);
    try {
      // Optional opening goto: if the request specifies a top-level url and
      // the first step isn't a goto, navigate first to keep the scenario
      // declarative.
      const firstStep = input.steps[0];
      if (input.url && firstStep && firstStep.action !== 'goto') {
        await driver.goto(input.url, undefined, config.defaultStepTimeoutMs);
      }

      let stepIndex = 0;
      for (const step of input.steps) {
        const stepStart = Date.now();
        try {
          await runStep(driver, step, extracted, screenshots);
          steps.push({
            index: stepIndex,
            action: step.action,
            status: 'ok',
            durationMs: Date.now() - stepStart,
            description: step.description,
          });
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          steps.push({
            index: stepIndex,
            action: step.action,
            status: 'error',
            durationMs: Date.now() - stepStart,
            description: step.description,
            error: message,
          });
          return { ok: false, error: `step ${stepIndex} (${step.action}) failed: ${message}` };
        }
        stepIndex += 1;
      }

      if (input.captureFinalScreenshot ?? true) {
        try {
          const shot = await driver.screenshot('final', false);
          if (shot) screenshots.push(shot);
        } catch (error) {
          // best effort; do not fail the whole run for a screenshot.
          fastify.log.warn(
            { err: error, requestId },
            'browser-test final screenshot failed',
          );
        }
      }

      const finalUrl = await driver.currentUrl().catch(() => undefined);
      const finalTitle = await driver.currentTitle().catch(() => undefined);

      consoleEntries.push(...driver.drainConsole());
      pageErrors.push(...driver.drainPageErrors());

      const consoleErrorTriggered =
        (input.failOnConsoleError ?? false) &&
        consoleEntries.some((entry) => entry.level === 'error');

      if (consoleErrorTriggered) {
        return {
          ok: false,
          finalUrl,
          finalTitle,
          error: 'failOnConsoleError: page emitted at least one console error',
        };
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
    const message = error instanceof Error ? error.message : String(error);
    outcome = { ok: false, error: message };
  }

  const finishedAtIso = new Date().toISOString();
  const durationMs = Date.now() - startedAtMs;

  return {
    ok: outcome.ok,
    requestId,
    tool,
    durationMs,
    startedAt: startedAtIso,
    finishedAt: finishedAtIso,
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

interface ScenarioDriver {
  goto(url: string, waitUntil: 'load' | 'domcontentloaded' | 'networkidle' | undefined, timeoutMs: number): Promise<void>;
  click(selector: string, nth: number | undefined, timeoutMs: number): Promise<void>;
  fill(selector: string, value: string, timeoutMs: number): Promise<void>;
  select(selector: string, value: string, timeoutMs: number): Promise<void>;
  press(selector: string | undefined, key: string, timeoutMs: number): Promise<void>;
  waitForSelector(
    selector: string,
    state: 'attached' | 'detached' | 'visible' | 'hidden' | undefined,
    timeoutMs: number,
  ): Promise<void>;
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

async function openDriver(tool: Tool, input: RunRequest): Promise<ScenarioDriver> {
  if (tool === 'playwright') return openPlaywrightDriver(input);
  if (tool === 'puppeteer') return openPuppeteerDriver(input);
  return openSeleniumDriver(input);
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
      const key = step.name ?? `text:${step.selector}`;
      extracted[key] = value;
      return;
    }
    case 'extractAttribute': {
      const value = await driver.extractAttribute(step.selector, step.attribute, timeoutMs);
      const key = step.name ?? `attr:${step.selector}@${step.attribute}`;
      extracted[key] = value;
      return;
    }
    case 'screenshot': {
      const shot = await driver.screenshot(step.name ?? `step-${Date.now()}`, step.fullPage ?? false);
      if (shot) screenshots.push(shot);
      return;
    }
    case 'evaluate': {
      if (!config.allowEvaluate) {
        throw new Error('evaluate steps are disabled (set BROWSER_TEST_ALLOW_EVALUATE=true to enable)');
      }
      const value = await driver.evaluate(step.script, timeoutMs);
      const key = step.name ?? 'evaluate';
      extracted[key] = stringifyEvaluateResult(value);
      return;
    }
  }
}

// --- Playwright driver ---------------------------------------------------

async function getPlaywrightBrowser(): Promise<PlaywrightBrowser> {
  if (playwrightBrowser) return playwrightBrowser;
  if (playwrightBrowserPromise) return playwrightBrowserPromise;
  playwrightBrowserPromise = (async () => {
    const browser = await playwrightChromium.launch({
      headless: config.browserHeadless,
      args: ['--no-sandbox', '--disable-dev-shm-usage'],
      ...(config.chromiumExecutablePath
        ? { executablePath: config.chromiumExecutablePath }
        : {}),
    });
    browser.on('disconnected', () => {
      playwrightBrowser = null;
      playwrightBrowserPromise = null;
    });
    playwrightBrowser = browser;
    return browser;
  })();
  try {
    return await playwrightBrowserPromise;
  } finally {
    if (!playwrightBrowser) playwrightBrowserPromise = null;
  }
}

async function openPlaywrightDriver(input: RunRequest): Promise<ScenarioDriver> {
  const browser = await getPlaywrightBrowser();
  const context = await browser.newContext({
    viewport: input.viewport ?? { width: 1280, height: 800 },
    userAgent: input.userAgent,
    extraHTTPHeaders: input.extraHeaders,
  });
  const page = await context.newPage();
  const console: ConsoleLogEntry[] = [];
  const errors: string[] = [];
  page.on('console', (msg) => {
    console.push({
      level: msg.type(),
      text: msg.text(),
      timestamp: new Date().toISOString(),
    });
  });
  page.on('pageerror', (err) => errors.push(err.message));

  return {
    goto: async (url, waitUntil, timeoutMs) => {
      await page.goto(url, { waitUntil: waitUntil ?? 'load', timeout: timeoutMs });
    },
    click: async (selector, nth, timeoutMs) => {
      const locator = nth !== undefined ? page.locator(selector).nth(nth) : page.locator(selector);
      await locator.click({ timeout: timeoutMs });
    },
    fill: async (selector, value, timeoutMs) => {
      await page.fill(selector, value, { timeout: timeoutMs });
    },
    select: async (selector, value, timeoutMs) => {
      await page.selectOption(selector, value, { timeout: timeoutMs });
    },
    press: async (selector, key, timeoutMs) => {
      if (selector) {
        await page.press(selector, key, { timeout: timeoutMs });
      } else {
        await page.keyboard.press(key);
      }
    },
    waitForSelector: async (selector, state, timeoutMs) => {
      await page.waitForSelector(selector, { state: state ?? 'visible', timeout: timeoutMs });
    },
    waitForUrl: async (urlPattern, timeoutMs) => {
      await page.waitForURL(urlPattern, { timeout: timeoutMs });
    },
    waitForTimeout: async (ms) => {
      await page.waitForTimeout(ms);
    },
    extractText: async (selector, timeoutMs) => {
      const handle = await page.waitForSelector(selector, { state: 'attached', timeout: timeoutMs });
      const text = (await handle.textContent()) ?? '';
      return text.trim();
    },
    extractAttribute: async (selector, attribute, timeoutMs) => {
      const handle = await page.waitForSelector(selector, { state: 'attached', timeout: timeoutMs });
      return (await handle.getAttribute(attribute)) ?? '';
    },
    screenshot: async (name, fullPage) => {
      const buffer = await page.screenshot({ type: 'jpeg', quality: config.screenshotQuality, fullPage });
      return clampScreenshot(name, 'image/jpeg', buffer);
    },
    evaluate: async (script) => {
      return await page.evaluate(`(function(){ ${script} })()`);
    },
    currentUrl: async () => page.url(),
    currentTitle: async () => page.title(),
    drainConsole: () => console.splice(0),
    drainPageErrors: () => errors.splice(0),
    close: async () => {
      await context.close();
    },
  } satisfies ScenarioDriver;
}

// --- Puppeteer driver ----------------------------------------------------

async function getPuppeteerBrowser(): Promise<PuppeteerBrowser> {
  if (puppeteerBrowser) return puppeteerBrowser;
  if (puppeteerBrowserPromise) return puppeteerBrowserPromise;
  puppeteerBrowserPromise = (async () => {
    const browser = await puppeteer.launch({
      headless: config.browserHeadless,
      args: ['--no-sandbox', '--disable-dev-shm-usage'],
      executablePath: config.chromiumExecutablePath ?? playwrightChromium.executablePath(),
    });
    browser.on('disconnected', () => {
      puppeteerBrowser = null;
      puppeteerBrowserPromise = null;
    });
    puppeteerBrowser = browser;
    return browser;
  })();
  try {
    return await puppeteerBrowserPromise;
  } finally {
    if (!puppeteerBrowser) puppeteerBrowserPromise = null;
  }
}

async function openPuppeteerDriver(input: RunRequest): Promise<ScenarioDriver> {
  const browser = await getPuppeteerBrowser();
  const context = await browser.createBrowserContext();
  const page = await context.newPage();
  if (input.viewport) await page.setViewport(input.viewport);
  if (input.userAgent) await page.setUserAgent(input.userAgent);
  if (input.extraHeaders) await page.setExtraHTTPHeaders(input.extraHeaders);

  const console: ConsoleLogEntry[] = [];
  const errors: string[] = [];
  page.on('console', (msg) => {
    console.push({
      level: msg.type(),
      text: msg.text(),
      timestamp: new Date().toISOString(),
    });
  });
  page.on('pageerror', (err: unknown) => {
    errors.push(err instanceof Error ? err.message : String(err));
  });

  const elementByNth = async (selector: string, nth: number | undefined) => {
    const handles = await page.$$(selector);
    const index = nth ?? 0;
    const handle = handles[index];
    if (!handle) {
      throw new Error(`puppeteer: selector ${selector} did not match index ${index}`);
    }
    return handle;
  };

  return {
    goto: async (url, waitUntil, timeoutMs) => {
      await page.goto(url, { waitUntil: mapPuppeteerWaitUntil(waitUntil), timeout: timeoutMs });
    },
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
        if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
          node.value = '';
        }
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
          if (pattern.startsWith('/') && pattern.endsWith('/')) {
            const re = new RegExp(pattern.slice(1, -1));
            return re.test(current);
          }
          return current === pattern || current.includes(pattern);
        },
        { timeout: timeoutMs },
        urlPattern,
        page.url(),
      );
    },
    waitForTimeout: async (ms) => {
      await new Promise((resolve) => setTimeout(resolve, ms));
    },
    extractText: async (selector, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs });
      const text = await page.$eval(selector, (node) => (node.textContent ?? '').trim());
      return text;
    },
    extractAttribute: async (selector, attribute, timeoutMs) => {
      await page.waitForSelector(selector, { timeout: timeoutMs });
      return (await page.$eval(
        selector,
        (node, attr) => (node as Element).getAttribute(attr) ?? '',
        attribute,
      )) as string;
    },
    screenshot: async (name, fullPage) => {
      const buffer = await page.screenshot({
        type: 'jpeg',
        quality: config.screenshotQuality,
        fullPage,
      });
      return clampScreenshot(name, 'image/jpeg', Buffer.from(buffer));
    },
    evaluate: async (script) => {
      return await page.evaluate(`(function(){ ${script} })()`);
    },
    currentUrl: async () => page.url(),
    currentTitle: async () => page.title(),
    drainConsole: () => console.splice(0),
    drainPageErrors: () => errors.splice(0),
    close: async () => {
      await context.close();
    },
  } satisfies ScenarioDriver;
}

function mapPuppeteerWaitUntil(
  waitUntil: 'load' | 'domcontentloaded' | 'networkidle' | undefined,
): 'load' | 'domcontentloaded' | 'networkidle0' | undefined {
  if (waitUntil === 'networkidle') return 'networkidle0';
  return waitUntil;
}

// --- Selenium driver -----------------------------------------------------

async function openSeleniumDriver(input: RunRequest): Promise<ScenarioDriver> {
  const options = new ChromeOptions();
  if (config.browserHeadless) options.addArguments('--headless=new');
  options.addArguments('--no-sandbox', '--disable-dev-shm-usage');
  const binary = config.chromiumExecutablePath ?? playwrightChromium.executablePath();
  if (binary) options.setChromeBinaryPath(binary);
  if (input.userAgent) options.addArguments(`--user-agent=${input.userAgent}`);

  const driver: WebDriver = await new SeleniumBuilder()
    .forBrowser('chrome')
    .setChromeOptions(options)
    .build();

  if (input.viewport) {
    await driver.manage().window().setRect({
      width: input.viewport.width,
      height: input.viewport.height,
      x: 0,
      y: 0,
    });
  }

  // Selenium doesn't expose a console feed that's portable across drivers,
  // so we collect logs via the WebDriver "browser" log type when available
  // and treat absence as an empty list.
  const console: ConsoleLogEntry[] = [];
  const errors: string[] = [];
  const collectConsole = async () => {
    try {
      const entries = await driver.manage().logs().get('browser');
      for (const entry of entries) {
        console.push({
          level: entry.level.name.toLowerCase(),
          text: entry.message,
          timestamp: new Date(entry.timestamp).toISOString(),
        });
      }
    } catch {
      // Some chromedriver versions don't enable browser logs; ignore.
    }
  };

  const findOne = async (selector: string, nth: number, timeoutMs: number): Promise<WebElement> => {
    await driver.wait(seleniumUntil.elementLocated(SeleniumBy.css(selector)), timeoutMs);
    const elements = await driver.findElements(SeleniumBy.css(selector));
    const handle = elements[nth];
    if (!handle) throw new Error(`selenium: selector ${selector} did not match index ${nth}`);
    return handle;
  };

  return {
    goto: async (url, _waitUntil, timeoutMs) => {
      await driver.manage().setTimeouts({ pageLoad: timeoutMs });
      await driver.get(url);
    },
    click: async (selector, nth, timeoutMs) => {
      const element = await findOne(selector, nth ?? 0, timeoutMs);
      await driver.wait(seleniumUntil.elementIsVisible(element), timeoutMs);
      await element.click();
    },
    fill: async (selector, value, timeoutMs) => {
      const element = await findOne(selector, 0, timeoutMs);
      await element.clear();
      await element.sendKeys(value);
    },
    select: async (selector, value, timeoutMs) => {
      const element = await findOne(selector, 0, timeoutMs);
      // Selenium's Select helper requires an extra import; using sendKeys
      // works for text-based <option> and avoids that. Callers that need
      // value-based select should use "extractAttribute" then fall back.
      await element.sendKeys(value);
    },
    press: async (selector, key, timeoutMs) => {
      if (selector) {
        const element = await findOne(selector, 0, timeoutMs);
        await element.sendKeys(key);
      } else {
        await driver
          .actions({ async: true })
          .keyDown(key)
          .keyUp(key)
          .perform();
      }
    },
    waitForSelector: async (selector, state, timeoutMs) => {
      if (state === 'detached' || state === 'hidden') {
        await driver.wait(async () => {
          const elements = await driver.findElements(SeleniumBy.css(selector));
          if (elements.length === 0) return true;
          if (state === 'hidden') {
            const first = elements[0];
            if (!first) return true;
            return !(await first.isDisplayed());
          }
          return false;
        }, timeoutMs);
        return;
      }
      const located = await driver.wait(
        seleniumUntil.elementLocated(SeleniumBy.css(selector)),
        timeoutMs,
      );
      if (state !== 'attached') {
        await driver.wait(seleniumUntil.elementIsVisible(located), timeoutMs);
      }
    },
    waitForUrl: async (urlPattern, timeoutMs) => {
      const condition: Parameters<WebDriver['wait']>[0] =
        urlPattern.startsWith('/') && urlPattern.endsWith('/')
          ? seleniumUntil.urlMatches(new RegExp(urlPattern.slice(1, -1)))
          : seleniumUntil.urlContains(urlPattern);
      await driver.wait(condition, timeoutMs);
    },
    waitForTimeout: async (ms) => {
      await driver.sleep(ms);
    },
    extractText: async (selector, timeoutMs) => {
      const element = await findOne(selector, 0, timeoutMs);
      return (await element.getText()).trim();
    },
    extractAttribute: async (selector, attribute, timeoutMs) => {
      const element = await findOne(selector, 0, timeoutMs);
      return (await element.getAttribute(attribute)) ?? '';
    },
    screenshot: async (name) => {
      const base64 = await driver.takeScreenshot();
      const buffer = Buffer.from(base64, 'base64');
      return clampScreenshot(name, 'image/png', buffer);
    },
    evaluate: async (script) => {
      return await driver.executeScript(`return (function(){ ${script} })();`);
    },
    currentUrl: async () => driver.getCurrentUrl(),
    currentTitle: async () => driver.getTitle(),
    drainConsole: () => {
      // Selenium uses pull-based logs; we already populated `console` lazily,
      // but we also pull one final batch on drain.
      void collectConsole();
      return console.splice(0);
    },
    drainPageErrors: () => errors.splice(0),
    close: async () => {
      await driver.quit();
    },
  } satisfies ScenarioDriver;
}

// --- Helpers -------------------------------------------------------------

function clampScreenshot(
  name: string,
  contentType: 'image/png' | 'image/jpeg',
  buffer: Buffer,
): ScreenshotPayload {
  const truncated = buffer.byteLength > config.maxScreenshotBytes;
  const trimmed = truncated ? buffer.subarray(0, config.maxScreenshotBytes) : buffer;
  return {
    name,
    contentType,
    base64: trimmed.toString('base64'),
    bytes: buffer.byteLength,
    ...(truncated ? { truncated: true } : {}),
  };
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

function isAuthorized(headers: Record<string, string | string[] | undefined>): boolean {
  if (config.allowUnauthenticated) return true;
  if (!config.serverAuthSecret) return false;
  const candidate =
    pickHeader(headers, 'x-server-auth') ??
    pickHeader(headers, 'authorization') ??
    pickHeader(headers, 'x-auth');
  if (!candidate) return false;
  const provided = candidate.replace(/^Bearer\s+/i, '');
  const expected = config.serverAuthSecret;
  if (provided.length !== expected.length) return false;
  try {
    return timingSafeEqual(Buffer.from(provided), Buffer.from(expected));
  } catch {
    return false;
  }
}

function pickHeader(
  headers: Record<string, string | string[] | undefined>,
  key: string,
): string | null {
  const value = headers[key];
  if (Array.isArray(value)) return value[0] ?? null;
  return value ?? null;
}

function recordMetric(tool: Tool, status: 'ok' | 'error', durationMs: number) {
  const key = `${tool}:${status}`;
  metrics.total.set(key, (metrics.total.get(key) ?? 0) + 1);
  metrics.durationSumMs.set(tool, (metrics.durationSumMs.get(tool) ?? 0) + durationMs);
  metrics.durationCount.set(tool, (metrics.durationCount.get(tool) ?? 0) + 1);
}

function renderMetrics(): string {
  const lines: string[] = [];
  lines.push('# HELP browser_test_runs_total Total scenario runs grouped by tool and status.');
  lines.push('# TYPE browser_test_runs_total counter');
  for (const [key, count] of metrics.total) {
    const [tool, status] = key.split(':');
    lines.push(`browser_test_runs_total{tool="${tool}",status="${status}"} ${count}`);
  }
  lines.push('# HELP browser_test_in_flight Current in-flight scenarios.');
  lines.push('# TYPE browser_test_in_flight gauge');
  lines.push(`browser_test_in_flight ${metrics.inFlight}`);
  lines.push('# HELP browser_test_duration_ms_sum Total duration in milliseconds per tool.');
  lines.push('# TYPE browser_test_duration_ms_sum counter');
  for (const [tool, sum] of metrics.durationSumMs) {
    lines.push(`browser_test_duration_ms_sum{tool="${tool}"} ${sum}`);
  }
  lines.push('# HELP browser_test_duration_ms_count Number of completed runs per tool.');
  lines.push('# TYPE browser_test_duration_ms_count counter');
  for (const [tool, count] of metrics.durationCount) {
    lines.push(`browser_test_duration_ms_count{tool="${tool}"} ${count}`);
  }
  return `${lines.join('\n')}\n`;
}

function serviceDescriptor() {
  return {
    service: 'dd-browser-test-server',
    ok: true,
    endpoints: {
      run: 'POST /run',
      tools: 'GET /browser-test/tools',
      status: 'GET /browser-test/status',
      healthz: 'GET /browser-test/healthz',
      metrics: 'GET /browser-test/metrics',
    },
    tools: TOOLS,
    defaultTool: config.defaultTool,
    browserHeadless: config.browserHeadless,
    allowEvaluate: config.allowEvaluate,
  };
}

function toolsDescriptor() {
  return {
    default: config.defaultTool,
    tools: TOOLS.map((tool) => ({
      name: tool,
      version: resolveToolVersion(tool),
      supportsHeadless: true,
      supportsEvaluate: tool !== 'selenium' || config.allowEvaluate,
    })),
  };
}

function statusDescriptor() {
  return {
    ok: true,
    service: 'dd-browser-test-server',
    serverStartedAt,
    serverInstanceId,
    inFlight: metrics.inFlight,
    maxConcurrent: config.maxConcurrent,
    defaultTool: config.defaultTool,
    defaultTimeoutMs: config.defaultTimeoutMs,
    maxTimeoutMs: config.maxTimeoutMs,
    maxSteps: config.maxSteps,
    browserHeadless: config.browserHeadless,
    allowEvaluate: config.allowEvaluate,
  };
}

function healthDescriptor() {
  return {
    ok: true,
    service: 'dd-browser-test-server',
    serverStartedAt,
    serverInstanceId,
    inFlight: metrics.inFlight,
  };
}

function resolveToolVersion(tool: Tool): string {
  try {
    if (tool === 'playwright') {
      return (requireFromCwd('playwright/package.json') as { version: string }).version;
    }
    if (tool === 'puppeteer') {
      return (requireFromCwd('puppeteer/package.json') as { version: string }).version;
    }
    return (requireFromCwd('selenium-webdriver/package.json') as { version: string }).version;
  } catch {
    return 'unknown';
  }
}

function normalizeTool(value: string): Tool {
  const lower = value.toLowerCase().trim();
  if ((TOOLS as readonly string[]).includes(lower)) return lower as Tool;
  return 'playwright';
}

function readNumberEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  return value;
}

function readBooleanEnv(name: string, fallback: boolean): boolean {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  return raw === 'true' || raw === '1' || raw === 'yes';
}

function clampNumber(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, value));
}

function setTimeoutPromise(ms: number): Promise<never> {
  return new Promise((_resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout after ${ms}ms`)), ms);
    timer.unref?.();
  });
}

async function shutdown(signal: NodeJS.Signals) {
  fastify.log.info({ signal }, 'browser-test shutting down');
  try {
    await fastify.close();
  } finally {
    if (playwrightBrowser) {
      try {
        await playwrightBrowser.close();
      } catch {
        // ignore
      }
    }
    if (puppeteerBrowser) {
      try {
        await puppeteerBrowser.close();
      } catch {
        // ignore
      }
    }
    process.exit(0);
  }
}

process.on('SIGTERM', () => void shutdown('SIGTERM'));
process.on('SIGINT', () => void shutdown('SIGINT'));

fastify
  .listen({ host: config.host, port: config.port })
  .then((address) => {
    fastify.log.info({ address }, 'dd-browser-test-server listening');
  })
  .catch((err) => {
    fastify.log.error({ err }, 'dd-browser-test-server failed to start');
    process.exit(1);
  });

export type { RunRequest, RunResult, Step };
