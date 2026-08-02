import Fastify, {
  type FastifyBaseLogger,
  type FastifyInstance,
  type FastifyReply,
  type FastifyRequest,
  type RouteHandlerMethod,
  type onRequestHookHandler,
} from 'fastify';
import { initTelemetry, instrumentFastify, loggerMixin } from '@dd/telemetry';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
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

import {
  CONTRACT_LIMITS,
  ConcurrencyResponseSchema,
  HealthDescriptorSchema,
  RunRequestSchema,
  RunResultSchema,
  ServiceDescriptorSchema,
  StatusDescriptorSchema,
  ToolsDescriptorSchema,
  UnauthorizedResponseSchema,
  ValidationErrorResponseSchema,
  TOOLS,
  type ConsoleLogEntry,
  type RunRequest,
  type RunResult,
  type ScreenshotPayload,
  type Step,
  type StepLogEntry,
  type Tool,
} from './api-schemas.js';
import {
  ApiContractRegistry,
  ContractValidationError,
  type ApiDocuments,
  type ApiRouteContract,
} from './api-contract.js';

const OPENAPI_EXPORT_FLAG = '--export-openapi';
const OPENAPI_CONTENT_TYPE = 'application/vnd.oai.openapi+json;version=3.1';
const MAX_BODY_BYTES = 2_097_152;

const configuredMaxTimeoutMs = clampNumber(
  readNumberEnv('BROWSER_TEST_MAX_TIMEOUT_MS', CONTRACT_LIMITS.maxTimeoutMs),
  500,
  CONTRACT_LIMITS.maxTimeoutMs,
);
const configuredMaxSteps = clampNumber(
  readNumberEnv('BROWSER_TEST_MAX_STEPS', CONTRACT_LIMITS.maxSteps),
  1,
  CONTRACT_LIMITS.maxSteps,
);

const config = {
  host: process.env.HOST ?? '0.0.0.0',
  port: clampNumber(readNumberEnv('PORT', 8104), 1, 65_535),
  serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
  allowUnauthenticated: process.env.BROWSER_TEST_ALLOW_UNAUTHENTICATED === 'true',
  defaultTool: normalizeTool(process.env.BROWSER_TEST_DEFAULT_TOOL ?? 'playwright'),
  maxConcurrent: clampNumber(readNumberEnv('BROWSER_TEST_MAX_CONCURRENT', 2), 1, 32),
  defaultTimeoutMs: clampNumber(
    readNumberEnv('BROWSER_TEST_DEFAULT_TIMEOUT_MS', 30_000),
    500,
    configuredMaxTimeoutMs,
  ),
  maxTimeoutMs: configuredMaxTimeoutMs,
  defaultStepTimeoutMs: clampNumber(
    readNumberEnv('BROWSER_TEST_STEP_TIMEOUT_MS', 15_000),
    100,
    CONTRACT_LIMITS.maxStepTimeoutMs,
  ),
  maxSteps: configuredMaxSteps,
  maxScreenshotBytes: clampNumber(
    readNumberEnv('BROWSER_TEST_MAX_SCREENSHOT_BYTES', 1_500_000),
    1_024,
    10_000_000,
  ),
  screenshotQuality: clampNumber(readNumberEnv('BROWSER_TEST_SCREENSHOT_QUALITY', 70), 1, 100),
  browserHeadless: readBooleanEnv('BROWSER_TEST_HEADLESS', true),
  allowEvaluate: readBooleanEnv('BROWSER_TEST_ALLOW_EVALUATE', false),
  chromiumExecutablePath: process.env.BROWSER_TEST_CHROMIUM_PATH ?? null,
};

const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();

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
let appLogger: FastifyBaseLogger | null = null;

export interface BuildAppOptions {
  authSecret?: string | null;
  allowUnauthenticated?: boolean;
  instrumentTelemetry?: boolean;
}

export interface BuiltApp {
  app: FastifyInstance;
  documents: ApiDocuments;
}

function boundedValidationIssues(
  validation: Array<{ instancePath?: string; message?: string }> | undefined,
): Array<{ path: string; message: string }> {
  return (validation ?? [])
    .slice(0, 20)
    .map((issue) => ({
      path: (issue.instancePath || '$').slice(0, 300),
      message: (issue.message || 'invalid value').slice(0, 500),
    }));
}

function requestAuthorized(
  headers: Record<string, string | string[] | undefined>,
  expected: string | null,
  allowUnauthenticated: boolean,
): boolean {
  if (allowUnauthenticated) return true;
  if (!expected) return false;
  const candidate =
    pickHeader(headers, 'x-server-auth') ??
    pickHeader(headers, 'authorization') ??
    pickHeader(headers, 'x-auth');
  if (!candidate) return false;
  const provided = candidate.replace(/^Bearer\s+/i, '');
  if (provided.length !== expected.length) return false;
  try {
    return timingSafeEqual(Buffer.from(provided), Buffer.from(expected));
  } catch {
    return false;
  }
}

export async function buildApp(options: BuildAppOptions = {}): Promise<BuiltApp> {
  const app = Fastify({
    logger: { mixin: loggerMixin },
    bodyLimit: MAX_BODY_BYTES,
  });
  if (options.instrumentTelemetry) {
    instrumentFastify(app, { service: 'dd-browser-test-server' });
  }
  appLogger = app.log;

  const authSecret =
    options.authSecret === undefined ? config.serverAuthSecret : options.authSecret;
  const allowUnauthenticated =
    options.allowUnauthenticated ?? config.allowUnauthenticated;
  const requireAuth: onRequestHookHandler = async (request, reply) => {
    if (
      requestAuthorized(
        request.headers as Record<string, string | string[] | undefined>,
        authSecret,
        allowUnauthenticated,
      )
    ) {
      return;
    }
    await reply.code(401).send({ ok: false, error: 'unauthorized' });
  };

  app.setErrorHandler(async (error, request, reply) => {
    if (error instanceof ContractValidationError) {
      await reply.code(400).send({
        ok: false,
        error: 'invalid_request',
        issues: error.issues,
      });
      return;
    }
    const fastifyValidation = (
      error as { validation?: Array<{ instancePath?: string; message?: string }> }
    ).validation;
    if (fastifyValidation) {
      await reply.code(400).send({
        ok: false,
        error: 'invalid_request',
        issues: boundedValidationIssues(fastifyValidation),
      });
      return;
    }
    request.log.error({ err: error }, 'browser-test request failed before handler completion');
    await reply.code(500).send({ ok: false, error: 'internal_error' });
  });

  const registry = new ApiContractRegistry();
  const register = (route: ApiRouteContract) => registry.register(app, route);
  let documents: ApiDocuments | undefined;
  const docs = () => {
    if (!documents) throw new Error('API documents requested before router finalization');
    return documents;
  };

  const serviceHandler: RouteHandlerMethod = async () => serviceDescriptor();
  const toolsHandler: RouteHandlerMethod = async () => toolsDescriptor();
  const statusHandler: RouteHandlerMethod = async () => statusDescriptor();
  const healthHandler: RouteHandlerMethod = async () => healthDescriptor();
  const metricsHandler: RouteHandlerMethod = async (_request, reply) => {
    await reply.type('text/plain; version=0.0.4; charset=utf-8').send(renderMetrics());
  };
  const publicJsonHandler: RouteHandlerMethod = async (_request, reply) => {
    await reply.type(OPENAPI_CONTENT_TYPE).send(docs().publicJson);
  };
  const internalJsonHandler: RouteHandlerMethod = async (_request, reply) => {
    await reply.type(OPENAPI_CONTENT_TYPE).send(docs().internalJson);
  };
  const publicHtmlHandler: RouteHandlerMethod = async (_request, reply) => {
    await reply.type('text/html; charset=utf-8').send(docs().publicHtml);
  };
  const internalHtmlHandler: RouteHandlerMethod = async (_request, reply) => {
    await reply.type('text/html; charset=utf-8').send(docs().internalHtml);
  };
  const runHandler: RouteHandlerMethod = async (request, reply) => {
    const input = request.body as RunRequest;
    const runtimeIssues: Array<{ path: string; message: string }> = [];
    if (input.steps.length > config.maxSteps) {
      runtimeIssues.push({
        path: '$.steps',
        message: `deployment permits at most ${config.maxSteps} steps`,
      });
    }
    if (input.timeoutMs !== undefined && input.timeoutMs > config.maxTimeoutMs) {
      runtimeIssues.push({
        path: '$.timeoutMs',
        message: `deployment permits at most ${config.maxTimeoutMs}ms`,
      });
    }
    if (runtimeIssues.length > 0) {
      await reply.code(400).send({
        ok: false,
        error: 'invalid_request',
        issues: runtimeIssues,
      });
      return;
    }

    if (metrics.inFlight >= config.maxConcurrent) {
      await reply.code(429).send({
        ok: false,
        error: 'browser-test concurrency limit reached',
        maxConcurrent: config.maxConcurrent,
      });
      return;
    }

    const tool: Tool = input.tool ?? config.defaultTool;
    const requestId = input.requestId ?? randomUUID();
    const startedAtIso = new Date().toISOString();
    const startedAtMs = Date.now();
    metrics.inFlight += 1;

    try {
      const result = await runScenario(tool, input, requestId, startedAtIso);
      recordMetric(tool, result.ok ? 'ok' : 'error', result.durationMs);
      if (!result.ok) {
        await reply.code(422).send(result);
        return;
      }
      await reply.code(200).send(result);
    } catch (error) {
      const durationMs = Date.now() - startedAtMs;
      recordMetric(tool, 'error', durationMs);
      request.log.error({ err: error, requestId, tool }, 'browser-test scenario crashed');
      await reply.code(500).send({
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
        error: 'internal browser-test failure',
      } satisfies RunResult);
    } finally {
      metrics.inFlight -= 1;
    }
  };

  const unauthorized = {
    description: 'Service authentication is missing or invalid.',
    schema: UnauthorizedResponseSchema,
  };
  const validation = {
    description: 'The request does not satisfy the executable Zod contract.',
    schema: ValidationErrorResponseSchema,
  };

  for (const [path, operationId] of [
    ['/', 'getBrowserTestService'],
    ['/browser-test', 'getBrowserTestServiceCompatibilityAlias'],
  ] as const) {
    register({
      method: 'GET',
      path,
      operationId,
      summary: 'Describe the browser-test service and supported HTTP endpoints.',
      tags: ['service'],
      visibility: 'internal',
      auth: 'server-auth',
      routeType: 'service',
      responses: {
        '200': { description: 'Browser-test service descriptor.', schema: ServiceDescriptorSchema },
        '401': unauthorized,
      },
      onRequest: requireAuth,
      handler: serviceHandler,
    });
  }

  for (const [path, operationId] of [
    ['/tools', 'listBrowserAutomationTools'],
    ['/browser-test/tools', 'listBrowserAutomationToolsCompatibilityAlias'],
  ] as const) {
    register({
      method: 'GET',
      path,
      operationId,
      summary: 'List supported browser drivers and their runtime versions.',
      tags: ['browser-automation'],
      visibility: 'internal',
      auth: 'server-auth',
      routeType: 'user-generated',
      responses: {
        '200': { description: 'Supported browser drivers.', schema: ToolsDescriptorSchema },
        '401': unauthorized,
      },
      onRequest: requireAuth,
      handler: toolsHandler,
    });
  }

  for (const [path, operationId] of [
    ['/status', 'getBrowserTestStatus'],
    ['/browser-test/status', 'getBrowserTestStatusCompatibilityAlias'],
  ] as const) {
    register({
      method: 'GET',
      path,
      operationId,
      summary: 'Return bounded browser-test runtime status.',
      tags: ['operations'],
      visibility: 'internal',
      auth: 'server-auth',
      routeType: 'user-generated',
      responses: {
        '200': { description: 'Browser-test runtime status.', schema: StatusDescriptorSchema },
        '401': unauthorized,
      },
      onRequest: requireAuth,
      handler: statusHandler,
    });
  }

  register({
    method: 'GET',
    path: '/healthz',
    operationId: 'getBrowserTestHealth',
    summary: 'Return a public liveness response for Kubernetes probes.',
    tags: ['operations'],
    visibility: 'public',
    auth: 'public',
    routeType: 'service',
    responses: {
      '200': { description: 'Browser-test process is alive.', schema: HealthDescriptorSchema },
    },
    handler: healthHandler,
  });
  register({
    method: 'GET',
    path: '/browser-test/healthz',
    operationId: 'getBrowserTestHealthCompatibilityAlias',
    summary: 'Authenticated compatibility alias for browser-test liveness.',
    tags: ['operations'],
    visibility: 'internal',
    auth: 'server-auth',
    routeType: 'service',
    responses: {
      '200': { description: 'Browser-test process is alive.', schema: HealthDescriptorSchema },
      '401': unauthorized,
    },
    onRequest: requireAuth,
    handler: healthHandler,
  });

  register({
    method: 'GET',
    path: '/metrics',
    operationId: 'getBrowserTestPrometheusMetrics',
    summary: 'Return bounded Prometheus text exposition.',
    tags: ['operations'],
    visibility: 'public',
    auth: 'public',
    routeType: 'service',
    responses: {
      '200': {
        description: 'Prometheus metrics for browser-test execution.',
        schema: z.string(),
        contentType: 'text/plain',
      },
    },
    handler: metricsHandler,
  });
  register({
    method: 'GET',
    path: '/browser-test/metrics',
    operationId: 'getBrowserTestPrometheusMetricsCompatibilityAlias',
    summary: 'Authenticated compatibility alias for Prometheus metrics.',
    tags: ['operations'],
    visibility: 'internal',
    auth: 'server-auth',
    routeType: 'service',
    responses: {
      '200': {
        description: 'Prometheus metrics for browser-test execution.',
        schema: z.string(),
        contentType: 'text/plain',
      },
      '401': unauthorized,
    },
    onRequest: requireAuth,
    handler: metricsHandler,
  });

  for (const [path, operationId] of [
    ['/openapi.json', 'getBrowserTestPublicOpenApi'],
    ['/api/docs.json', 'getBrowserTestPublicOpenApiCompatibilityAlias'],
  ] as const) {
    register({
      method: 'GET',
      path,
      operationId,
      summary: 'Return the fail-closed public OpenAPI 3.1 contract.',
      tags: ['documentation'],
      visibility: 'public',
      auth: 'public',
      routeType: 'service',
      responses: {
        '200': {
          description: 'Canonical public OpenAPI document.',
          schema: z.string(),
          contentType: OPENAPI_CONTENT_TYPE,
        },
      },
      handler: publicJsonHandler,
    });
  }

  for (const [path, operationId] of [
    ['/api/docs', 'getBrowserTestPublicApiReference'],
    ['/docs/api', 'getBrowserTestPublicApiReferenceCompatibilityAlias'],
  ] as const) {
    register({
      method: 'GET',
      path,
      operationId,
      summary: 'Return the Scalar reference for the fail-closed public contract.',
      tags: ['documentation'],
      visibility: 'public',
      auth: 'public',
      routeType: 'service',
      responses: {
        '200': {
          description: 'Human-readable public API reference.',
          schema: z.string(),
          contentType: 'text/html',
        },
      },
      handler: publicHtmlHandler,
    });
  }

  register({
    method: 'GET',
    path: '/internal/openapi.json',
    operationId: 'getBrowserTestInternalOpenApi',
    summary: 'Return the complete typed OpenAPI contract to trusted callers.',
    tags: ['documentation'],
    visibility: 'internal',
    auth: 'server-auth',
    routeType: 'service',
    responses: {
      '200': {
        description: 'Complete internal OpenAPI document.',
        schema: z.string(),
        contentType: OPENAPI_CONTENT_TYPE,
      },
      '401': unauthorized,
    },
    onRequest: requireAuth,
    handler: internalJsonHandler,
  });
  register({
    method: 'GET',
    path: '/internal/docs/api',
    operationId: 'getBrowserTestInternalApiReference',
    summary: 'Return the Scalar reference for the complete internal contract.',
    tags: ['documentation'],
    visibility: 'internal',
    auth: 'server-auth',
    routeType: 'service',
    responses: {
      '200': {
        description: 'Human-readable internal API reference.',
        schema: z.string(),
        contentType: 'text/html',
      },
      '401': unauthorized,
    },
    onRequest: requireAuth,
    handler: internalHtmlHandler,
  });

  register({
    method: 'POST',
    path: '/run',
    operationId: 'runBrowserScenario',
    summary: 'Run a bounded browser scenario with Playwright, Puppeteer, or Selenium.',
    description:
      'Runs the declarative scenario DSL. Arbitrary page evaluation remains disabled unless the deployment explicitly opts in.',
    tags: ['browser-automation'],
    visibility: 'internal',
    auth: 'server-auth',
    routeType: 'user-generated',
    body: RunRequestSchema,
    bodyDescription: 'Browser driver, limits, viewport, and declarative scenario steps.',
    responses: {
      '200': { description: 'Scenario completed successfully.', schema: RunResultSchema },
      '400': validation,
      '401': unauthorized,
      '422': { description: 'Scenario ran but a step or page assertion failed.', schema: RunResultSchema },
      '429': {
        description: 'The configured concurrent scenario limit is full.',
        schema: ConcurrencyResponseSchema,
      },
      '500': { description: 'The scenario crashed unexpectedly.', schema: RunResultSchema },
    },
    onRequest: requireAuth,
    handler: runHandler,
  });

  documents = registry.documents();
  await app.ready();
  return { app, documents };
}

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
          appLogger?.warn(
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
        (pattern: string) => {
          const current = window.location.href;
          if (pattern.startsWith('/') && pattern.endsWith('/')) {
            const re = new RegExp(pattern.slice(1, -1));
            return re.test(current);
          }
          return current === pattern || current.includes(pattern);
        },
        { timeout: timeoutMs },
        urlPattern,
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
      openapi: 'GET /openapi.json',
      docs: 'GET /docs/api',
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

async function closeBrowsers(): Promise<void> {
  if (playwrightBrowser) {
    try {
      await playwrightBrowser.close();
    } catch {
      // best effort during shutdown
    }
    playwrightBrowser = null;
    playwrightBrowserPromise = null;
  }
  if (puppeteerBrowser) {
    try {
      await puppeteerBrowser.close();
    } catch {
      // best effort during shutdown
    }
    puppeteerBrowser = null;
    puppeteerBrowserPromise = null;
  }
}

async function startServer(): Promise<void> {
  const telemetry = initTelemetry('dd-browser-test-server');
  const { app } = await buildApp({ instrumentTelemetry: true });
  let closing = false;
  const shutdown = async (signal: NodeJS.Signals) => {
    if (closing) return;
    closing = true;
    app.log.info({ signal }, 'browser-test shutting down');
    try {
      await app.close();
    } finally {
      await closeBrowsers();
      await telemetry.shutdown();
    }
  };
  process.once('SIGTERM', () => void shutdown('SIGTERM'));
  process.once('SIGINT', () => void shutdown('SIGINT'));

  const address = await app.listen({ host: config.host, port: config.port });
  app.log.info({ address }, 'dd-browser-test-server listening');
}

async function runCli(): Promise<void> {
  if (process.argv.includes(OPENAPI_EXPORT_FLAG)) {
    const { app, documents } = await buildApp({
      authSecret: 'contract-export-only',
      instrumentTelemetry: false,
    });
    try {
      process.stdout.write(documents.internalJson);
    } finally {
      await app.close();
    }
    return;
  }
  await startServer();
}

function isMainModule(): boolean {
  const entry = process.argv[1];
  return entry !== undefined && pathToFileURL(entry).href === import.meta.url;
}

if (isMainModule()) {
  runCli().catch((error) => {
    console.error(error instanceof Error ? error.stack : String(error));
    process.exitCode = 1;
  });
}

export type { RunRequest, RunResult, Step };
