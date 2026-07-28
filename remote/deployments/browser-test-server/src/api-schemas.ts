import { z } from 'zod';

export const TOOLS = ['playwright', 'puppeteer', 'selenium'] as const;
export const ToolSchema = z.enum(TOOLS);
export type Tool = z.infer<typeof ToolSchema>;

export const CONTRACT_LIMITS = {
  maxSteps: 64,
  maxTimeoutMs: 180_000,
  maxStepTimeoutMs: 300_000,
  maxWaitMs: 60_000,
  maxSelectorLength: 800,
  maxValueLength: 20_000,
  maxHeaderNameLength: 120,
  maxHeaderValueLength: 2_000,
} as const;

const StepBaseSchema = z
  .object({
    description: z.string().max(200).optional(),
    timeoutMs: z.number().int().min(100).max(CONTRACT_LIMITS.maxStepTimeoutMs).optional(),
  })
  .strict();

export const StepSchema = z.discriminatedUnion('action', [
  StepBaseSchema.extend({
    action: z.literal('goto'),
    url: z.string().url(),
    waitUntil: z.enum(['load', 'domcontentloaded', 'networkidle']).optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('click'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    nth: z.number().int().min(0).max(50).optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('fill'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    value: z.string().max(CONTRACT_LIMITS.maxValueLength),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('select'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    value: z.string().max(CONTRACT_LIMITS.maxSelectorLength),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('press'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength).optional(),
    key: z.string().min(1).max(40),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('waitForSelector'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    state: z.enum(['attached', 'detached', 'visible', 'hidden']).optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('waitForUrl'),
    url: z.string().min(1).max(2_000),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('waitForTimeout'),
    ms: z.number().int().min(0).max(CONTRACT_LIMITS.maxWaitMs),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('extractText'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    name: z.string().min(1).max(120).optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('extractAttribute'),
    selector: z.string().min(1).max(CONTRACT_LIMITS.maxSelectorLength),
    attribute: z.string().min(1).max(120),
    name: z.string().min(1).max(120).optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('screenshot'),
    name: z.string().min(1).max(120).optional(),
    fullPage: z.boolean().optional(),
  }).strict(),
  StepBaseSchema.extend({
    action: z.literal('evaluate'),
    script: z.string().min(1).max(CONTRACT_LIMITS.maxValueLength),
    name: z.string().min(1).max(120).optional(),
  }).strict(),
]);
export type Step = z.infer<typeof StepSchema>;

export const RunRequestSchema = z
  .object({
    requestId: z.string().min(1).max(120).optional(),
    tool: ToolSchema.optional(),
    url: z.string().url().optional(),
    steps: z.array(StepSchema).min(1).max(CONTRACT_LIMITS.maxSteps),
    timeoutMs: z.number().int().min(500).max(CONTRACT_LIMITS.maxTimeoutMs).optional(),
    viewport: z
      .object({
        width: z.number().int().min(200).max(4_000),
        height: z.number().int().min(200).max(4_000),
      })
      .strict()
      .optional(),
    userAgent: z.string().min(1).max(500).optional(),
    extraHeaders: z
      .record(
        z.string().min(1).max(CONTRACT_LIMITS.maxHeaderNameLength),
        z.string().max(CONTRACT_LIMITS.maxHeaderValueLength),
      )
      .optional(),
    captureFinalScreenshot: z.boolean().optional(),
    failOnConsoleError: z.boolean().optional(),
  })
  .strict();
export type RunRequest = z.infer<typeof RunRequestSchema>;

export const StepLogEntrySchema = z
  .object({
    index: z.number().int().min(0),
    action: z.enum([
      'goto',
      'click',
      'fill',
      'select',
      'press',
      'waitForSelector',
      'waitForUrl',
      'waitForTimeout',
      'extractText',
      'extractAttribute',
      'screenshot',
      'evaluate',
    ]),
    status: z.enum(['ok', 'error']),
    durationMs: z.number().int().min(0),
    description: z.string().max(200).optional(),
    error: z.string().max(4_000).optional(),
  })
  .strict();
export type StepLogEntry = z.infer<typeof StepLogEntrySchema>;

export const ConsoleLogEntrySchema = z
  .object({
    level: z.string().max(40),
    text: z.string().max(20_000),
    timestamp: z.string(),
  })
  .strict();
export type ConsoleLogEntry = z.infer<typeof ConsoleLogEntrySchema>;

export const ScreenshotPayloadSchema = z
  .object({
    name: z.string().min(1).max(120),
    contentType: z.enum(['image/png', 'image/jpeg']),
    base64: z.string(),
    bytes: z.number().int().min(0),
    truncated: z.boolean().optional(),
  })
  .strict();
export type ScreenshotPayload = z.infer<typeof ScreenshotPayloadSchema>;

export const RunResultSchema = z
  .object({
    ok: z.boolean(),
    requestId: z.string().min(1).max(120),
    tool: ToolSchema,
    durationMs: z.number().int().min(0),
    startedAt: z.string(),
    finishedAt: z.string(),
    finalUrl: z.string().optional(),
    finalTitle: z.string().optional(),
    steps: z.array(StepLogEntrySchema),
    extracted: z.record(z.string(), z.string()),
    screenshots: z.array(ScreenshotPayloadSchema),
    consoleEntries: z.array(ConsoleLogEntrySchema),
    pageErrors: z.array(z.string().max(20_000)),
    error: z.string().max(20_000).optional(),
  })
  .strict();
export type RunResult = z.infer<typeof RunResultSchema>;

export const ValidationIssueSchema = z
  .object({
    path: z.string().max(300),
    message: z.string().max(500),
  })
  .strict();

export const ValidationErrorResponseSchema = z
  .object({
    ok: z.literal(false),
    error: z.literal('invalid_request'),
    issues: z.array(ValidationIssueSchema).max(20),
  })
  .strict();

export const UnauthorizedResponseSchema = z
  .object({
    ok: z.literal(false),
    error: z.literal('unauthorized'),
  })
  .strict();

export const ConcurrencyResponseSchema = z
  .object({
    ok: z.literal(false),
    error: z.literal('browser-test concurrency limit reached'),
    maxConcurrent: z.number().int().min(1),
  })
  .strict();

export const ServiceDescriptorSchema = z
  .object({
    service: z.literal('dd-browser-test-server'),
    ok: z.literal(true),
    endpoints: z
      .object({
        run: z.string(),
        tools: z.string(),
        status: z.string(),
        healthz: z.string(),
        metrics: z.string(),
        openapi: z.string(),
        docs: z.string(),
      })
      .strict(),
    tools: z.array(ToolSchema),
    defaultTool: ToolSchema,
    browserHeadless: z.boolean(),
    allowEvaluate: z.boolean(),
  })
  .strict();

export const ToolsDescriptorSchema = z
  .object({
    default: z.string().regex(/^(playwright|puppeteer|selenium)$/),
    tools: z.array(
      z
        .object({
          name: ToolSchema,
          version: z.string(),
          supportsHeadless: z.boolean(),
          supportsEvaluate: z.boolean(),
        })
        .strict(),
    ),
  })
  .strict();

export const StatusDescriptorSchema = z
  .object({
    ok: z.literal(true),
    service: z.literal('dd-browser-test-server'),
    serverStartedAt: z.string(),
    serverInstanceId: z.string(),
    inFlight: z.number().int().min(0),
    maxConcurrent: z.number().int().min(1),
    defaultTool: ToolSchema,
    defaultTimeoutMs: z.number().int().min(500),
    maxTimeoutMs: z.number().int().min(500),
    maxSteps: z.number().int().min(1),
    browserHeadless: z.boolean(),
    allowEvaluate: z.boolean(),
  })
  .strict();

export const HealthDescriptorSchema = z
  .object({
    ok: z.literal(true),
    service: z.literal('dd-browser-test-server'),
    serverStartedAt: z.string(),
    serverInstanceId: z.string(),
    inFlight: z.number().int().min(0),
  })
  .strict();
