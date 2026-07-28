import { Type, type Static } from '@fastify/type-provider-typebox';
import type { FastifySchema } from 'fastify';

export const SERVICE_NAME = 'browser-test-server';
export const RUNTIME_SERVICE_NAME = 'dd-browser-test-server';
export const TOOLS = ['playwright', 'puppeteer', 'selenium'] as const;

const JSON_SCHEMA_DIALECT = 'https://json-schema.org/draft/2020-12/schema';
const SERVER_SOURCE = 'remote/deployments/browser-test-server/src/server.ts';
const CONTRACT_SOURCE = 'remote/deployments/browser-test-server/src/api-contract.ts';
const SCALAR_BROWSER_URL =
  'https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.46.4/dist/browser/standalone.js';

export const ToolSchema = Type.Union([
  Type.Literal('playwright'),
  Type.Literal('puppeteer'),
  Type.Literal('selenium'),
], {
  $id: 'BrowserTool',
  description: 'Browser automation implementation used for this scenario.',
});

const StepDescriptionSchema = Type.Optional(
  Type.String({ minLength: 1, maxLength: 200 }),
);
const StepTimeoutSchema = Type.Optional(
  Type.Integer({ minimum: 100, maximum: 300_000 }),
);

const GotoStepSchema = Type.Object(
  {
    action: Type.Literal('goto'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    url: Type.String({ format: 'uri', maxLength: 2_000 }),
    waitUntil: Type.Optional(
      Type.Union([
        Type.Literal('load'),
        Type.Literal('domcontentloaded'),
        Type.Literal('networkidle'),
      ]),
    ),
  },
  { additionalProperties: false },
);

const ClickStepSchema = Type.Object(
  {
    action: Type.Literal('click'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    nth: Type.Optional(Type.Integer({ minimum: 0, maximum: 50 })),
  },
  { additionalProperties: false },
);

const FillStepSchema = Type.Object(
  {
    action: Type.Literal('fill'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    value: Type.String({ maxLength: 20_000 }),
  },
  { additionalProperties: false },
);

const SelectStepSchema = Type.Object(
  {
    action: Type.Literal('select'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    value: Type.String({ maxLength: 800 }),
  },
  { additionalProperties: false },
);

const PressStepSchema = Type.Object(
  {
    action: Type.Literal('press'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.Optional(Type.String({ minLength: 1, maxLength: 800 })),
    key: Type.String({ minLength: 1, maxLength: 40 }),
  },
  { additionalProperties: false },
);

const WaitForSelectorStepSchema = Type.Object(
  {
    action: Type.Literal('waitForSelector'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    state: Type.Optional(
      Type.Union([
        Type.Literal('attached'),
        Type.Literal('detached'),
        Type.Literal('visible'),
        Type.Literal('hidden'),
      ]),
    ),
  },
  { additionalProperties: false },
);

const WaitForUrlStepSchema = Type.Object(
  {
    action: Type.Literal('waitForUrl'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    url: Type.String({ minLength: 1, maxLength: 2_000 }),
  },
  { additionalProperties: false },
);

const WaitForTimeoutStepSchema = Type.Object(
  {
    action: Type.Literal('waitForTimeout'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    ms: Type.Integer({ minimum: 0, maximum: 60_000 }),
  },
  { additionalProperties: false },
);

const ExtractTextStepSchema = Type.Object(
  {
    action: Type.Literal('extractText'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    name: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
  },
  { additionalProperties: false },
);

const ExtractAttributeStepSchema = Type.Object(
  {
    action: Type.Literal('extractAttribute'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    selector: Type.String({ minLength: 1, maxLength: 800 }),
    attribute: Type.String({ minLength: 1, maxLength: 120 }),
    name: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
  },
  { additionalProperties: false },
);

const ScreenshotStepSchema = Type.Object(
  {
    action: Type.Literal('screenshot'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    name: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
    fullPage: Type.Optional(Type.Boolean()),
  },
  { additionalProperties: false },
);

const EvaluateStepSchema = Type.Object(
  {
    action: Type.Literal('evaluate'),
    description: StepDescriptionSchema,
    timeoutMs: StepTimeoutSchema,
    script: Type.String({ minLength: 1, maxLength: 20_000 }),
    name: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
  },
  { additionalProperties: false },
);

export const StepSchema = Type.Union(
  [
    GotoStepSchema,
    ClickStepSchema,
    FillStepSchema,
    SelectStepSchema,
    PressStepSchema,
    WaitForSelectorStepSchema,
    WaitForUrlStepSchema,
    WaitForTimeoutStepSchema,
    ExtractTextStepSchema,
    ExtractAttributeStepSchema,
    ScreenshotStepSchema,
    EvaluateStepSchema,
  ],
  {
    $id: 'BrowserScenarioStep',
    description: 'One bounded declarative browser action. Arbitrary evaluate is runtime opt-in.',
  },
);

export const RunRequestSchema = Type.Object(
  {
    requestId: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
    tool: Type.Optional(ToolSchema),
    url: Type.Optional(Type.String({ format: 'uri', maxLength: 2_000 })),
    steps: Type.Array(StepSchema, { minItems: 1, maxItems: 64 }),
    timeoutMs: Type.Optional(Type.Integer({ minimum: 500, maximum: 180_000 })),
    viewport: Type.Optional(
      Type.Object(
        {
          width: Type.Integer({ minimum: 200, maximum: 4_000 }),
          height: Type.Integer({ minimum: 200, maximum: 4_000 }),
        },
        { additionalProperties: false },
      ),
    ),
    userAgent: Type.Optional(Type.String({ minLength: 1, maxLength: 500 })),
    extraHeaders: Type.Optional(
      Type.Record(
        Type.String({ minLength: 1, maxLength: 120 }),
        Type.String({ maxLength: 2_000 }),
        { maxProperties: 64 },
      ),
    ),
    captureFinalScreenshot: Type.Optional(Type.Boolean()),
    failOnConsoleError: Type.Optional(Type.Boolean()),
  },
  {
    $id: 'BrowserRunRequest',
    additionalProperties: false,
    description: 'A bounded browser scenario. The schema is the runtime validator and SDK contract.',
  },
);

export const StepLogEntrySchema = Type.Object(
  {
    index: Type.Integer({ minimum: 0 }),
    action: Type.Union([
      Type.Literal('goto'),
      Type.Literal('click'),
      Type.Literal('fill'),
      Type.Literal('select'),
      Type.Literal('press'),
      Type.Literal('waitForSelector'),
      Type.Literal('waitForUrl'),
      Type.Literal('waitForTimeout'),
      Type.Literal('extractText'),
      Type.Literal('extractAttribute'),
      Type.Literal('screenshot'),
      Type.Literal('evaluate'),
    ]),
    status: Type.Union([Type.Literal('ok'), Type.Literal('error')]),
    durationMs: Type.Integer({ minimum: 0 }),
    description: Type.Optional(Type.String()),
    error: Type.Optional(Type.String()),
  },
  { additionalProperties: false },
);

export const ConsoleLogEntrySchema = Type.Object(
  {
    level: Type.String(),
    text: Type.String(),
    timestamp: Type.String({ format: 'date-time' }),
  },
  { additionalProperties: false },
);

export const ScreenshotPayloadSchema = Type.Object(
  {
    name: Type.String(),
    contentType: Type.Union([Type.Literal('image/png'), Type.Literal('image/jpeg')]),
    base64: Type.String({ contentEncoding: 'base64' }),
    bytes: Type.Integer({ minimum: 0 }),
    truncated: Type.Optional(Type.Boolean()),
  },
  { additionalProperties: false },
);

export const RunResultSchema = Type.Object(
  {
    ok: Type.Boolean(),
    requestId: Type.String(),
    tool: ToolSchema,
    durationMs: Type.Integer({ minimum: 0 }),
    startedAt: Type.String({ format: 'date-time' }),
    finishedAt: Type.String({ format: 'date-time' }),
    finalUrl: Type.Optional(Type.String()),
    finalTitle: Type.Optional(Type.String()),
    steps: Type.Array(StepLogEntrySchema),
    extracted: Type.Record(Type.String(), Type.String()),
    screenshots: Type.Array(ScreenshotPayloadSchema),
    consoleEntries: Type.Array(ConsoleLogEntrySchema),
    pageErrors: Type.Array(Type.String()),
    error: Type.Optional(Type.String()),
  },
  { $id: 'BrowserRunResult', additionalProperties: false },
);

export const ErrorResponseSchema = Type.Object(
  {
    ok: Type.Literal(false),
    error: Type.String(),
    details: Type.Optional(Type.Any()),
    maxConcurrent: Type.Optional(Type.Integer({ minimum: 1 })),
  },
  { $id: 'BrowserApiError', additionalProperties: false },
);

const ServiceDescriptorSchema = Type.Object(
  {
    service: Type.Literal(RUNTIME_SERVICE_NAME),
    ok: Type.Literal(true),
    endpoints: Type.Object(
      {
        run: Type.String(),
        tools: Type.String(),
        status: Type.String(),
        healthz: Type.String(),
        metrics: Type.String(),
        publicOpenApi: Type.String(),
        internalOpenApi: Type.String(),
      },
      { additionalProperties: false },
    ),
    tools: Type.Array(ToolSchema),
    defaultTool: ToolSchema,
    browserHeadless: Type.Boolean(),
    allowEvaluate: Type.Boolean(),
  },
  { additionalProperties: false },
);

const ToolDescriptorSchema = Type.Object(
  {
    name: ToolSchema,
    version: Type.String(),
    supportsHeadless: Type.Boolean(),
    supportsEvaluate: Type.Boolean(),
  },
  { additionalProperties: false },
);

const ToolsDescriptorSchema = Type.Object(
  {
    default: ToolSchema,
    tools: Type.Array(ToolDescriptorSchema),
  },
  { additionalProperties: false },
);

const StatusDescriptorSchema = Type.Object(
  {
    ok: Type.Literal(true),
    service: Type.Literal(RUNTIME_SERVICE_NAME),
    serverStartedAt: Type.String({ format: 'date-time' }),
    serverInstanceId: Type.String(),
    inFlight: Type.Integer({ minimum: 0 }),
    maxConcurrent: Type.Integer({ minimum: 1 }),
    defaultTool: ToolSchema,
    defaultTimeoutMs: Type.Integer({ minimum: 500 }),
    maxTimeoutMs: Type.Integer({ minimum: 500 }),
    maxSteps: Type.Integer({ minimum: 1 }),
    browserHeadless: Type.Boolean(),
    allowEvaluate: Type.Boolean(),
  },
  { additionalProperties: false },
);

const HealthDescriptorSchema = Type.Object(
  {
    ok: Type.Literal(true),
    service: Type.Literal(RUNTIME_SERVICE_NAME),
    serverStartedAt: Type.String({ format: 'date-time' }),
    serverInstanceId: Type.String(),
    inFlight: Type.Integer({ minimum: 0 }),
  },
  { additionalProperties: false },
);

const OpenApiDocumentSchema = Type.Any({
  description: 'OpenAPI 3.1 document generated from the executable Fastify route schemas.',
});
const HtmlDocumentSchema = Type.String({ description: 'Scalar API reference HTML.' });
const MetricsDocumentSchema = Type.String({ description: 'Prometheus text exposition format.' });

type Visibility = 'public' | 'internal';
type OperationOptions = {
  operationId: string;
  path: string;
  handler: string;
  summary: string;
  description: string;
  tags: string[];
  visibility: Visibility;
  body?: unknown;
  response: Record<number | string, unknown>;
};

function operationSchema(options: OperationOptions): FastifySchema {
  const auth =
    options.visibility === 'public'
      ? 'public'
      : 'X-Server-Auth or Bearer SERVER_AUTH_SECRET';
  return {
    operationId: options.operationId,
    summary: options.summary,
    description: options.description,
    tags: options.tags,
    ...(options.body === undefined ? {} : { body: options.body }),
    response: options.response,
    security: options.visibility === 'public' ? [] : [{ bearer_auth: [] }],
    'x-dd-auth': auth,
    'x-dd-handlers': [options.handler],
    'x-dd-implementation': 'fastify-typebox',
    'x-dd-route-type': options.tags[0] ?? 'service',
    'x-dd-source-files': [SERVER_SOURCE, CONTRACT_SOURCE],
    'x-dd-source-path': options.path,
    'x-dd-source-paths': [options.path],
    'x-dd-visibility': options.visibility,
  } as FastifySchema;
}

const publicJsonResponse = { 200: OpenApiDocumentSchema };
const publicHtmlResponse = { 200: HtmlDocumentSchema };
const internalJsonResponse = { 200: OpenApiDocumentSchema, 401: ErrorResponseSchema };
const internalHtmlResponse = { 200: HtmlDocumentSchema, 401: ErrorResponseSchema };

export const routeSchemas = {
  root: operationSchema({
    operationId: 'getBrowserTestServiceDescriptor',
    path: '/',
    handler: 'serviceDescriptor',
    summary: 'Get the browser test service descriptor.',
    description: 'Returns service capabilities and canonical API endpoint locations.',
    tags: ['service'],
    visibility: 'internal',
    response: { 200: ServiceDescriptorSchema, 401: ErrorResponseSchema },
  }),
  browserTestRoot: operationSchema({
    operationId: 'getBrowserTestServiceDescriptorAlias',
    path: '/browser-test',
    handler: 'serviceDescriptor',
    summary: 'Get the browser test service descriptor through its compatibility alias.',
    description: 'Compatibility alias for the authenticated service descriptor.',
    tags: ['service'],
    visibility: 'internal',
    response: { 200: ServiceDescriptorSchema, 401: ErrorResponseSchema },
  }),
  tools: operationSchema({
    operationId: 'listBrowserAutomationTools',
    path: '/tools',
    handler: 'toolsDescriptor',
    summary: 'List installed browser automation tools.',
    description: 'Returns supported engines, installed versions, and capability flags.',
    tags: ['browser-execution'],
    visibility: 'internal',
    response: { 200: ToolsDescriptorSchema, 401: ErrorResponseSchema },
  }),
  browserTestTools: operationSchema({
    operationId: 'listBrowserAutomationToolsAlias',
    path: '/browser-test/tools',
    handler: 'toolsDescriptor',
    summary: 'List browser automation tools through the compatibility alias.',
    description: 'Compatibility alias for the authenticated tool inventory.',
    tags: ['browser-execution'],
    visibility: 'internal',
    response: { 200: ToolsDescriptorSchema, 401: ErrorResponseSchema },
  }),
  status: operationSchema({
    operationId: 'getBrowserTestStatus',
    path: '/status',
    handler: 'statusDescriptor',
    summary: 'Get detailed browser test service status.',
    description: 'Returns process identity, concurrency, timeouts, and configured feature flags.',
    tags: ['operations'],
    visibility: 'internal',
    response: { 200: StatusDescriptorSchema, 401: ErrorResponseSchema },
  }),
  browserTestStatus: operationSchema({
    operationId: 'getBrowserTestStatusAlias',
    path: '/browser-test/status',
    handler: 'statusDescriptor',
    summary: 'Get browser test status through the compatibility alias.',
    description: 'Compatibility alias for the authenticated detailed status endpoint.',
    tags: ['operations'],
    visibility: 'internal',
    response: { 200: StatusDescriptorSchema, 401: ErrorResponseSchema },
  }),
  healthz: operationSchema({
    operationId: 'getBrowserTestHealth',
    path: '/healthz',
    handler: 'healthDescriptor',
    summary: 'Check browser test service health.',
    description: 'Public Kubernetes-compatible health endpoint with no sensitive configuration.',
    tags: ['observability'],
    visibility: 'public',
    response: { 200: HealthDescriptorSchema },
  }),
  browserTestHealthz: operationSchema({
    operationId: 'getBrowserTestHealthAlias',
    path: '/browser-test/healthz',
    handler: 'healthDescriptor',
    summary: 'Check service health through the compatibility alias.',
    description: 'Authenticated compatibility alias for the public standard health endpoint.',
    tags: ['observability'],
    visibility: 'internal',
    response: { 200: HealthDescriptorSchema, 401: ErrorResponseSchema },
  }),
  metrics: operationSchema({
    operationId: 'getBrowserTestMetrics',
    path: '/metrics',
    handler: 'renderMetrics',
    summary: 'Get Prometheus metrics.',
    description: 'Public Prometheus text exposition for browser scenario counters and durations.',
    tags: ['observability'],
    visibility: 'public',
    response: { 200: MetricsDocumentSchema },
  }),
  browserTestMetrics: operationSchema({
    operationId: 'getBrowserTestMetricsAlias',
    path: '/browser-test/metrics',
    handler: 'renderMetrics',
    summary: 'Get Prometheus metrics through the compatibility alias.',
    description: 'Authenticated compatibility alias for the standard metrics endpoint.',
    tags: ['observability'],
    visibility: 'internal',
    response: { 200: MetricsDocumentSchema, 401: ErrorResponseSchema },
  }),
  publicOpenApi: operationSchema({
    operationId: 'getBrowserTestPublicOpenApi',
    path: '/openapi.json',
    handler: 'sendPublicOpenApi',
    summary: 'Get the fail-closed public OpenAPI contract.',
    description: 'Serves the exact committed public projection generated from this executable contract.',
    tags: ['documentation'],
    visibility: 'public',
    response: publicJsonResponse,
  }),
  publicOpenApiAlias: operationSchema({
    operationId: 'getBrowserTestPublicOpenApiAlias',
    path: '/api/docs.json',
    handler: 'sendPublicOpenApi',
    summary: 'Get the public OpenAPI contract through the standard compatibility alias.',
    description: 'Byte-identical alias for /openapi.json.',
    tags: ['documentation'],
    visibility: 'public',
    response: publicJsonResponse,
  }),
  publicScalar: operationSchema({
    operationId: 'getBrowserTestPublicScalarReference',
    path: '/api/docs',
    handler: 'publicScalarReference',
    summary: 'Open the public Scalar API reference.',
    description: 'Interactive API reference backed only by the fail-closed public projection.',
    tags: ['documentation'],
    visibility: 'public',
    response: publicHtmlResponse,
  }),
  publicScalarAlias: operationSchema({
    operationId: 'getBrowserTestPublicScalarReferenceAlias',
    path: '/docs/api',
    handler: 'publicScalarReference',
    summary: 'Open the public Scalar API reference through the standard compatibility alias.',
    description: 'Compatibility alias for /api/docs.',
    tags: ['documentation'],
    visibility: 'public',
    response: publicHtmlResponse,
  }),
  internalOpenApi: operationSchema({
    operationId: 'getBrowserTestInternalOpenApi',
    path: '/internal/openapi.json',
    handler: 'sendInternalOpenApi',
    summary: 'Get the complete executable OpenAPI contract.',
    description: 'Bearer-authenticated full contract including operational and browser execution routes.',
    tags: ['documentation'],
    visibility: 'internal',
    response: internalJsonResponse,
  }),
  internalScalar: operationSchema({
    operationId: 'getBrowserTestInternalScalarReference',
    path: '/internal/docs/api',
    handler: 'internalScalarReference',
    summary: 'Open the complete authenticated Scalar API reference.',
    description: 'Interactive API reference backed by the complete executable contract.',
    tags: ['documentation'],
    visibility: 'internal',
    response: internalHtmlResponse,
  }),
  run: operationSchema({
    operationId: 'runBrowserScenario',
    path: '/run',
    handler: 'runScenario',
    summary: 'Run a bounded browser automation scenario.',
    description:
      'Validates and executes a declarative Playwright, Puppeteer, or Selenium scenario. Arbitrary evaluate remains runtime opt-in.',
    tags: ['browser-execution'],
    visibility: 'internal',
    body: RunRequestSchema,
    response: {
      200: RunResultSchema,
      400: ErrorResponseSchema,
      401: ErrorResponseSchema,
      422: RunResultSchema,
      429: ErrorResponseSchema,
      500: RunResultSchema,
    },
  }),
} as const;

export const OPENAPI_SWAGGER_OPTIONS = {
  openapi: {
    openapi: '3.1.0',
    jsonSchemaDialect: JSON_SCHEMA_DIALECT,
    info: {
      title: 'Browser Test Server API',
      version: '0.1.0',
      description:
        'Executable browser automation API. Fastify TypeBox route schemas are the runtime validator, OpenAPI source, and SDK source of truth.',
    },
    tags: [
      { name: 'service', description: 'Service discovery and capabilities.' },
      { name: 'browser-execution', description: 'Typed browser scenario execution.' },
      { name: 'operations', description: 'Authenticated runtime status.' },
      { name: 'observability', description: 'Health and metrics.' },
      { name: 'documentation', description: 'Public and authenticated API contracts.' },
    ],
    components: {
      securitySchemes: {
        bearer_auth: {
          type: 'http',
          scheme: 'bearer',
          bearerFormat: 'opaque',
          description:
            'SERVER_AUTH_SECRET supplied as Authorization: Bearer, X-Server-Auth, or X-Auth.',
        },
      },
    },
    'x-dd-contract-scope': 'internal',
    'x-dd-generated-by': '@fastify/swagger + @fastify/type-provider-typebox',
    'x-dd-language': 'node',
    'x-dd-service': SERVICE_NAME,
    'x-dd-standard-docs-routes': [
      '/openapi.json',
      '/api/docs.json',
      '/api/docs',
      '/docs/api',
    ],
  },
};

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(sortJson);
  }
  if (value === null || typeof value !== 'object') {
    return value;
  }
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, entry]) => [key, sortJson(entry)]),
  );
}

export function canonicalJson(value: unknown): string {
  return `${JSON.stringify(sortJson(value), null, 2)}\n`;
}

export function finalizeInternalOpenApiDocument(raw: unknown): Record<string, unknown> {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    throw new TypeError('Fastify Swagger did not produce an OpenAPI object');
  }
  const document = structuredClone(raw as Record<string, unknown>);
  const paths =
    document.paths && typeof document.paths === 'object'
      ? (document.paths as Record<string, unknown>)
      : {};
  let operationCount = 0;
  for (const pathItem of Object.values(paths)) {
    if (!pathItem || typeof pathItem !== 'object' || Array.isArray(pathItem)) continue;
    for (const method of ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace']) {
      if (method in pathItem) operationCount += 1;
    }
  }
  document.openapi = '3.1.0';
  document.jsonSchemaDialect = JSON_SCHEMA_DIALECT;
  document['x-dd-contract-scope'] = 'internal';
  document['x-dd-generated-by'] = '@fastify/swagger + @fastify/type-provider-typebox';
  document['x-dd-language'] = 'node';
  document['x-dd-operation-count'] = operationCount;
  document['x-dd-route-count'] = operationCount;
  document['x-dd-service'] = SERVICE_NAME;
  document['x-dd-standard-docs-routes'] = [
    '/openapi.json',
    '/api/docs.json',
    '/api/docs',
    '/docs/api',
  ];
  return document;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

export function scalarApiReferenceHtml(specUrl: string, title: string): string {
  const safeTitle = escapeHtml(title);
  const configuration = JSON.stringify({
    url: specUrl,
    pageTitle: title,
    theme: 'default',
    hideClientButton: false,
    persistAuth: false,
  });
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>${safeTitle}</title>
</head>
<body>
  <noscript><p>JavaScript is required for the interactive reference. Open <a href="${escapeHtml(specUrl)}">the OpenAPI JSON</a>.</p></noscript>
  <div id="app"></div>
  <script src="${SCALAR_BROWSER_URL}"></script>
  <script>Scalar.createApiReference('#app', ${configuration});</script>
</body>
</html>
`;
}

export type Tool = Static<typeof ToolSchema>;
export type Step = Static<typeof StepSchema>;
export type RunRequest = Static<typeof RunRequestSchema>;
export type StepLogEntry = Static<typeof StepLogEntrySchema>;
export type ConsoleLogEntry = Static<typeof ConsoleLogEntrySchema>;
export type ScreenshotPayload = Static<typeof ScreenshotPayloadSchema>;
export type RunResult = Static<typeof RunResultSchema>;
