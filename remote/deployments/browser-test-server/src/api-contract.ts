import fastifySwagger from '@fastify/swagger';
import type { FastifyInstance } from 'fastify';
import {
  createJsonSchemaTransform,
  jsonSchemaTransformObject,
  serializerCompiler,
  validatorCompiler,
} from 'fastify-type-provider-zod';
import { z } from 'zod/v4';

export const TOOLS = ['playwright', 'puppeteer', 'selenium'] as const;
export const CONTRACT_MAX_STEPS = 64;
export const CONTRACT_MAX_TIMEOUT_MS = 300_000;

export type Tool = (typeof TOOLS)[number];

const ToolSchema = z.enum(TOOLS).describe('Browser automation engine used for the scenario.');

const StepBaseSchema = z.object({
  description: z.string().max(200).optional(),
  timeoutMs: z.number().int().min(100).max(CONTRACT_MAX_TIMEOUT_MS).optional(),
});

export const StepSchema = z
  .discriminatedUnion('action', [
    StepBaseSchema.extend({
      action: z.literal('goto'),
      url: z.url(),
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
      url: z.string().min(1).max(2_000),
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
  ])
  .describe('One bounded browser-automation action. Arbitrary evaluate steps are disabled by default.');

export const RunRequestSchema = z
  .object({
    requestId: z.string().min(1).max(120).optional(),
    tool: ToolSchema.optional(),
    url: z.url().optional(),
    steps: z.array(StepSchema).min(1).max(CONTRACT_MAX_STEPS),
    timeoutMs: z.number().int().min(500).max(CONTRACT_MAX_TIMEOUT_MS).optional(),
    viewport: z
      .object({
        width: z.number().int().min(200).max(4_000),
        height: z.number().int().min(200).max(4_000),
      })
      .optional(),
    userAgent: z.string().min(1).max(500).optional(),
    extraHeaders: z.record(z.string().min(1).max(120), z.string().max(2_000)).optional(),
    captureFinalScreenshot: z.boolean().optional(),
    failOnConsoleError: z.boolean().optional(),
  })
  .strict()
  .describe('Declarative browser scenario. The schema is also the runtime request validator.');

const StepLogEntrySchema = z.object({
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
  description: z.string().optional(),
  error: z.string().optional(),
});

const ConsoleLogEntrySchema = z.object({
  level: z.string(),
  text: z.string(),
  timestamp: z.iso.datetime(),
});

const ScreenshotPayloadSchema = z.object({
  name: z.string(),
  contentType: z.enum(['image/png', 'image/jpeg']),
  base64: z.string(),
  bytes: z.number().int().min(0),
  truncated: z.boolean().optional(),
});

export const RunResultSchema = z
  .object({
    ok: z.boolean(),
    requestId: z.string(),
    tool: ToolSchema,
    durationMs: z.number().int().min(0),
    startedAt: z.iso.datetime(),
    finishedAt: z.iso.datetime(),
    finalUrl: z.string().optional(),
    finalTitle: z.string().optional(),
    steps: z.array(StepLogEntrySchema),
    extracted: z.record(z.string(), z.string()),
    screenshots: z.array(ScreenshotPayloadSchema),
    consoleEntries: z.array(ConsoleLogEntrySchema),
    pageErrors: z.array(z.string()),
    error: z.string().optional(),
  })
  .describe('Structured scenario result shared by runtime validation, OpenAPI, and generated SDKs.');

export const ErrorResponseSchema = z.object({
  ok: z.literal(false),
  error: z.string(),
  details: z.unknown().optional(),
  maxConcurrent: z.number().int().min(1).optional(),
  maxSteps: z.number().int().min(1).optional(),
  maxTimeoutMs: z.number().int().min(500).optional(),
});

export const ServiceDescriptorSchema = z.object({
  service: z.literal('dd-browser-test-server'),
  ok: z.literal(true),
  endpoints: z.object({
    run: z.string(),
    tools: z.string(),
    status: z.string(),
    healthz: z.string(),
    metrics: z.string(),
    openapi: z.string(),
    docs: z.string(),
  }),
  tools: z.array(ToolSchema),
  defaultTool: ToolSchema,
  browserHeadless: z.boolean(),
  allowEvaluate: z.boolean(),
});

export const ToolsDescriptorSchema = z.object({
  default: ToolSchema,
  tools: z.array(
    z.object({
      name: ToolSchema,
      version: z.string(),
      supportsHeadless: z.boolean(),
      supportsEvaluate: z.boolean(),
    }),
  ),
});

export const StatusDescriptorSchema = z.object({
  ok: z.literal(true),
  service: z.literal('dd-browser-test-server'),
  serverStartedAt: z.iso.datetime(),
  serverInstanceId: z.uuid(),
  inFlight: z.number().int().min(0),
  maxConcurrent: z.number().int().min(1),
  defaultTool: ToolSchema,
  defaultTimeoutMs: z.number().int().min(500),
  maxTimeoutMs: z.number().int().min(500),
  maxSteps: z.number().int().min(1),
  browserHeadless: z.boolean(),
  allowEvaluate: z.boolean(),
});

export const HealthDescriptorSchema = z.object({
  ok: z.literal(true),
  service: z.literal('dd-browser-test-server'),
  serverStartedAt: z.iso.datetime(),
  serverInstanceId: z.uuid(),
  inFlight: z.number().int().min(0),
});

export const MetricsTextSchema = z.string();

export const SERVER_AUTH_SECURITY = [
  { ServerAuth: [] },
  { BearerAuth: [] },
] as const;

export function configureExecutableOpenApi(app: FastifyInstance): void {
  app.setValidatorCompiler(validatorCompiler);
  app.setSerializerCompiler(serializerCompiler);

  app.register(fastifySwagger, {
    mode: 'dynamic',
    openapi: {
      openapi: '3.1.0',
      info: {
        title: 'DD Browser Test Server API',
        version: '1.0.0',
        description:
          'Private, typed HTTP API for bounded Playwright, Puppeteer, and Selenium scenarios. The document is generated from the same Zod schemas and Fastify route declarations used by the running server.',
      },
      servers: [],
      tags: [
        { name: 'service', description: 'Service discovery and health.' },
        { name: 'browser', description: 'Authenticated browser scenario execution.' },
        { name: 'observability', description: 'Metrics and runtime status.' },
      ],
      components: {
        securitySchemes: {
          ServerAuth: {
            type: 'apiKey',
            in: 'header',
            name: 'x-server-auth',
            description: 'Private server-to-server shared secret.',
          },
          BearerAuth: {
            type: 'http',
            scheme: 'bearer',
            description: 'Compatibility bearer form of the server authentication secret.',
          },
        },
      },
    },
    transform: createJsonSchemaTransform({
      zodToJsonConfig: { target: 'draft-2020-12' },
    }),
    transformObject: jsonSchemaTransformObject,
  });
}

export function registerDocumentationRoutes(app: FastifyInstance): void {
  const hiddenSchema = { hide: true } as const;
  const sendDocument = async (_request: unknown, reply: { type: (value: string) => unknown; send: (value: unknown) => unknown }) => {
    reply.type('application/json; charset=utf-8');
    return reply.send(app.swagger());
  };

  app.get('/openapi.json', { schema: hiddenSchema }, sendDocument);
  app.get('/api/docs.json', { schema: hiddenSchema }, sendDocument);

  app.register(async function scalarPrimary(scope) {
    await scope.register(import('@scalar/fastify-api-reference'), {
      routePrefix: '/api/docs',
      configuration: {
        title: 'DD Browser Test Server API',
        url: '/openapi.json',
        hideClientButton: false,
      },
    });
  });

  app.register(async function scalarCompatibilityAlias(scope) {
    await scope.register(import('@scalar/fastify-api-reference'), {
      routePrefix: '/docs/api',
      configuration: {
        title: 'DD Browser Test Server API',
        url: '/openapi.json',
        hideClientButton: false,
      },
    });
  });
}

export function stableOpenApiJson(app: FastifyInstance): string {
  return `${JSON.stringify(sortJson(app.swagger()), null, 2)}\n`;
}

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJson);
  if (value === null || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => [key, sortJson(nested)]),
  );
}

export type Step = z.infer<typeof StepSchema>;
export type RunRequest = z.infer<typeof RunRequestSchema>;
export type RunResult = z.infer<typeof RunResultSchema>;
