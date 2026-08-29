import type {
  FastifyInstance,
  FastifySchema,
  HTTPMethods,
  RouteHandlerMethod,
  onRequestHookHandler,
} from 'fastify';
import { z, type ZodType } from 'zod';

const SOURCE_FILE = 'remote/deployments/browser-test-server/src/server.ts';
const OPENAPI_VERSION = '3.1.0';
const JSON_SCHEMA_DIALECT = 'https://json-schema.org/draft/2020-12/schema';
const STANDARD_DOCS_ROUTES = ['/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'] as const;

export type ContractVisibility = 'public' | 'internal';
export type ContractAuth = 'public' | 'server-auth';
export type ContractRouteType = 'service' | 'user-generated';

type JsonObject = Record<string, unknown>;

export interface ApiResponseContract {
  description: string;
  schema?: ZodType;
  contentType?: string;
}

export interface ApiRouteContract {
  method: HTTPMethods;
  path: string;
  operationId: string;
  summary: string;
  description?: string;
  tags: string[];
  visibility: ContractVisibility;
  auth: ContractAuth;
  routeType: ContractRouteType;
  body?: ZodType;
  bodyDescription?: string;
  responses: Record<string, ApiResponseContract>;
  onRequest?: onRequestHookHandler;
  handler: RouteHandlerMethod;
}

export interface ApiDocuments {
  internalDocument: JsonObject;
  publicDocument: JsonObject;
  internalJson: string;
  publicJson: string;
  internalHtml: string;
  publicHtml: string;
  routeKeys: string[];
}

export interface ValidationIssue {
  path: string;
  message: string;
}

export class ContractValidationError extends Error {
  readonly issues: ValidationIssue[];

  constructor(issues: ValidationIssue[]) {
    super('request did not match the executable API contract');
    this.name = 'ContractValidationError';
    this.issues = issues.slice(0, 20);
  }
}

function jsonSchema(schema: ZodType): JsonObject {
  const converter = z.toJSONSchema as unknown as (
    value: ZodType,
    options?: { target?: 'draft-7' },
  ) => JsonObject;
  const result = structuredClone(converter(schema, { target: 'draft-7' }));
  delete result.$schema;
  return result;
}

function normalizeGeneratorSchema(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(normalizeGeneratorSchema);
  if (value === null || typeof value !== 'object') return value;

  const result = Object.fromEntries(
    Object.entries(value as JsonObject).map(([key, item]) => [
      key,
      normalizeGeneratorSchema(item),
    ]),
  ) as JsonObject;
  if (Object.hasOwn(result, 'const')) {
    const constant = result.const;
    result['x-dd-constant-value'] = constant;
    delete result.const;
    if (typeof constant === 'boolean') {
      result.type = 'boolean';
      delete result.enum;
    } else if (!Object.hasOwn(result, 'enum')) {
      result.enum = [constant];
    }
  }
  if (
    Array.isArray(result.enum) &&
    result.enum.length === 1 &&
    typeof result.enum[0] === 'boolean'
  ) {
    result['x-dd-constant-value'] = result.enum[0];
    result.type = 'boolean';
    delete result.enum;
  }
  return result;
}

function openApiSchema(schema: ZodType): JsonObject {
  return normalizeGeneratorSchema(jsonSchema(schema)) as JsonObject;
}

function validationIssues(error: z.ZodError): ValidationIssue[] {
  return error.issues.slice(0, 20).map((issue) => ({
    path: issue.path.length === 0 ? '$' : `$.${issue.path.map(String).join('.')}`.slice(0, 300),
    message: issue.message.slice(0, 500),
  }));
}

function responseSchemas(route: ApiRouteContract): Record<string, JsonObject> {
  const responses: Record<string, JsonObject> = {};
  for (const [status, response] of Object.entries(route.responses)) {
    if (response.schema) {
      responses[status] = jsonSchema(response.schema);
    }
  }
  return responses;
}

function openApiResponses(route: ApiRouteContract): JsonObject {
  const responses: JsonObject = {};
  for (const [status, response] of Object.entries(route.responses).sort(([left], [right]) =>
    left.localeCompare(right),
  )) {
    const item: JsonObject = { description: response.description };
    if (response.schema) {
      item.content = {
        [response.contentType ?? 'application/json']: {
          schema: openApiSchema(response.schema),
        },
      };
    }
    responses[status] = item;
  }
  return responses;
}

function authDescription(auth: ContractAuth): string {
  return auth === 'public'
    ? 'public'
    : 'X-Server-Auth, Authorization Bearer, or legacy X-Auth service secret';
}

function security(auth: ContractAuth): JsonObject[] {
  if (auth === 'public') return [];
  return [{ serverAuth: [] }, { bearer_auth: [] }, { legacyAuth: [] }];
}

function routeOperation(route: ApiRouteContract): JsonObject {
  const operation: JsonObject = {
    operationId: route.operationId,
    summary: route.summary,
    description: route.description ?? route.summary,
    tags: [...route.tags],
    responses: openApiResponses(route),
    security: security(route.auth),
    'x-dd-auth': authDescription(route.auth),
    'x-dd-handlers': [route.operationId],
    'x-dd-implementation': 'fastify-zod-executable-contract',
    'x-dd-route-type': route.routeType,
    'x-dd-source-files': [SOURCE_FILE],
    'x-dd-source-path': route.path,
    'x-dd-source-paths': [route.path],
    'x-dd-visibility': route.visibility,
  };
  if (route.body) {
    operation.requestBody = {
      required: true,
      description: route.bodyDescription ?? 'JSON request body validated by the runtime Zod schema.',
      content: {
        'application/json': {
          schema: openApiSchema(route.body),
        },
      },
    };
  }
  return operation;
}

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJson);
  if (value === null || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value as JsonObject)
      .filter(([, item]) => item !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, sortJson(item)]),
  );
}

export function canonicalJson(value: unknown): string {
  return `${JSON.stringify(sortJson(value), null, 2)}\n`;
}

function publicProjection(internalDocument: JsonObject): JsonObject {
  const document = structuredClone(internalDocument);
  const paths = document.paths as JsonObject;
  const usedTags = new Set<string>();
  let operations = 0;

  for (const [path, rawPathItem] of Object.entries(paths)) {
    const pathItem = rawPathItem as JsonObject;
    for (const [method, rawOperation] of Object.entries(pathItem)) {
      const operation = rawOperation as JsonObject;
      if (operation['x-dd-visibility'] !== 'public') {
        delete pathItem[method];
        continue;
      }
      operations += 1;
      for (const tag of (operation.tags as string[] | undefined) ?? []) usedTags.add(tag);
      for (const extension of [
        'x-dd-auth',
        'x-dd-handlers',
        'x-dd-implementation',
        'x-dd-source-files',
        'x-dd-source-path',
        'x-dd-source-paths',
      ]) {
        delete operation[extension];
      }
      operation.security = [];
    }
    if (Object.keys(pathItem).length === 0) delete paths[path];
  }

  document.components = {};
  document.info = {
    title: 'browser-test-server API (public)',
    version: '0.1.0',
    description: 'Fail-closed public subset. Only operations explicitly marked public are included.',
  };
  document.tags = ((document.tags as Array<{ name: string }> | undefined) ?? []).filter((tag) =>
    usedTags.has(tag.name),
  );
  document['x-dd-contract-scope'] = 'public';
  document['x-dd-operation-count'] = operations;
  document['x-dd-route-count'] = Object.keys(paths).length;
  return document;
}

function scalarHtml(title: string, specUrl: string): string {
  const escapedTitle = title.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  const escapedUrl = specUrl.replaceAll('&', '&amp;').replaceAll('"', '&quot;');
  return `<!doctype html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n  <meta name="viewport" content="width=device-width, initial-scale=1">\n  <title>${escapedTitle}</title>\n</head>\n<body>\n  <script id="api-reference" data-url="${escapedUrl}"></script>\n  <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>\n</body>\n</html>\n`;
}

export class ApiContractRegistry {
  readonly #routes: ApiRouteContract[] = [];

  register(app: FastifyInstance, route: ApiRouteContract): void {
    if (this.#routes.some((item) => item.method === route.method && item.path === route.path)) {
      throw new Error(`duplicate executable API route: ${route.method} ${route.path}`);
    }
    this.#routes.push(route);

    const bodySchema = route.body;
    // Fastify's default AJV configuration mutates objects while evaluating
    // `oneOf` branches (`removeAdditional`). A discriminated-union scenario can
    // therefore lose fields in an earlier branch before its matching branch is
    // considered. Keep the generated JSON Schema for OpenAPI documentation,
    // but make the original Zod schema the single authoritative request-body
    // validator. Fastify still parses JSON, applies body limits, and serializes
    // responses from the schemas below.
    const schema: FastifySchema = {
      response: responseSchemas(route),
    };

    app.route({
      method: route.method,
      url: route.path,
      schema,
      ...(route.onRequest ? { onRequest: route.onRequest } : {}),
      ...(bodySchema
        ? {
            preValidation: async (request) => {
              const parsed = bodySchema.safeParse(request.body);
              if (!parsed.success) throw new ContractValidationError(validationIssues(parsed.error));
              request.body = parsed.data;
            },
          }
        : {}),
      handler: route.handler,
    });
  }

  documents(): ApiDocuments {
    const paths: JsonObject = {};
    const tags = new Set<string>();
    const routeKeys: string[] = [];

    for (const route of [...this.#routes].sort((left, right) =>
      `${left.path}\u0000${left.method}`.localeCompare(`${right.path}\u0000${right.method}`),
    )) {
      const method = route.method.toLowerCase();
      const pathItem = (paths[route.path] as JsonObject | undefined) ?? {};
      pathItem[method] = routeOperation(route);
      paths[route.path] = pathItem;
      routeKeys.push(`${route.method} ${route.path}`);
      for (const tag of route.tags) tags.add(tag);
    }

    const internalDocument: JsonObject = {
      openapi: OPENAPI_VERSION,
      jsonSchemaDialect: JSON_SCHEMA_DIALECT,
      info: {
        title: 'browser-test-server API',
        version: '0.1.0',
        description:
          'Executable Fastify contract generated from the same Zod schemas and route descriptors used for runtime validation, response serialization, and handler dispatch.',
      },
      tags: [...tags].sort().map((name) => ({ name })),
      paths,
      components: {
        securitySchemes: {
          bearer_auth: {
            type: 'http',
            scheme: 'bearer',
            bearerFormat: 'opaque service token',
          },
          legacyAuth: {
            type: 'apiKey',
            in: 'header',
            name: 'X-Auth',
          },
          serverAuth: {
            type: 'apiKey',
            in: 'header',
            name: 'X-Server-Auth',
          },
        },
      },
      'x-dd-contract-scope': 'internal',
      'x-dd-generated-by': 'browser-test-server executable Fastify/Zod route registry',
      'x-dd-language': 'node',
      'x-dd-operation-count': this.#routes.length,
      'x-dd-route-count': this.#routes.length,
      'x-dd-service': 'browser-test-server',
      'x-dd-standard-docs-routes': [...STANDARD_DOCS_ROUTES],
    };
    const publicDocument = publicProjection(internalDocument);

    return {
      internalDocument,
      publicDocument,
      internalJson: canonicalJson(internalDocument),
      publicJson: canonicalJson(publicDocument),
      internalHtml: scalarHtml('browser-test-server internal API', '/internal/openapi.json'),
      publicHtml: scalarHtml('browser-test-server public API', '/openapi.json'),
      routeKeys: routeKeys.sort(),
    };
  }
}
