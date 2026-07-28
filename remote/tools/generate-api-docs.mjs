#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

const METHOD_ORDER = ['GET', 'POST', 'PATCH', 'DELETE', 'PUT', 'OPTIONS'];
const METHOD_CALLS = new Map([
  ['get', 'GET'],
  ['post', 'POST'],
  ['patch', 'PATCH'],
  ['delete', 'DELETE'],
  ['put', 'PUT'],
  ['options', 'OPTIONS'],
]);

const SERVICE_ROUTE_PATHS = new Set([
  '/',
  '/healthz',
  '/livez',
  '/readyz',
  '/metrics',
  '/docs/api',
  '/api/docs',
  '/api/docs.json',
  '/graphql/schema',
  '/api/graphql/schema',
  '/api-docs',
  '/api-docs/',
  '/api-docs.json',
  '/favicon.ico',
]);

const STANDARD_DOCS_ROUTES = ['/docs/api', '/api/docs', '/api/docs.json'];
const CENTRAL_DOCS_ROUTES = ['/api-docs', '/api-docs.json'];

const RUST_DEPLOYMENT_ALLOWLIST = new Set([
  'agent-worker-broker-rs',
  'apostille-services-server-rs',
  'auth-server-rs',
  'bastion-rs',
  'billing-server-rs',
  'build-server-rs',
  'cluster-mcp-rs',
  'container-pool-rs',
  'contract-service-rs',
  'dataset-labeling-rs',
  'dd-benefactor-marketing-rs',
  'dd-compliance-rs',
  'dd-document-rs',
  'dd-escrow-rs',
  'dd-git-rs',
  'des-simulator-rs',
  'fiducia-customer.rs',
  'dd-music-rs',
  'knowledge-graph-builder-rs',
  'dd-ocr-rs',
  'dd-sound-recorder-rs',
  'economics-server-rs',
  'fabrication-server-rs',
  'formal-methods-service-rs',
  'formal-methods-server-rs',
  'fabrication-server-rs',
  'mdp-optimizer-rs',
  'patent-filing-rs',
  'public-data-server-rs',
  'rest-api-rs',
  'runtime-config-rs',
  'rust-vapi-phone-rs',
  'trading-server-rs',
  'usacc-rest-api-backend-rs',
  'wal-gateway-rs',
  'web-home-rs',
  'webrtc-media-rs',
  'webrtc-signaling-rs',
]);

const RUST_ROUTE_SOURCE_OVERRIDES = new Map([
  ['billing-server-rs', 'src/api/mod.rs'],
  ['build-server-rs', 'src/http.rs'],
  ['dd-compliance-rs', 'src/routes.rs'],
  ['formal-methods-service-rs', 'src/routes/mod.rs'],
  ['usacc-rest-api-backend-rs', 'src/routes/mod.rs'],
]);

// Subscriber receive surface auto-mounted by `dd_runtime_config_client::router()`
// (Rust) and `registerRuntimeConfigRoutes()` (Node). The doc scanner does not
// see these via `.route("...")` literals because they live inside the shared
// helper crate, so we inject them whenever a service depends on the client.
const RUNTIME_CONFIG_SUBSCRIBER_ROUTES = [
  {
    path: '/internal/runtime-config',
    methods: ['GET'],
    handlers: ['dd_runtime_config_client::handle_get'],
    purposeHint:
      'Subscriber surface: returns the runtime-config snapshot this process currently has applied. Mounted by the shared dd-runtime-config-client helper.',
  },
  {
    path: '/internal/update-runtime-config',
    methods: ['POST'],
    handlers: ['dd_runtime_config_client::handle_apply'],
    purposeHint:
      'Subscriber surface: dd-runtime-config pushes a RuntimeConfigApplyRequest payload here every 5 min (cron) and on admin demand; the helper swaps the in-memory snapshot atomically.',
  },
  {
    path: '/internal/runtime-config/reset',
    methods: ['POST'],
    handlers: ['dd_runtime_config_client::handle_reset'],
    purposeHint:
      'Subscriber surface: drops all runtime-config overrides from this process, returning it to its boot-time env defaults.',
  },
];

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(__dirname, '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const checkOnly = process.argv.includes('--check');
const serviceFlagIndex = process.argv.indexOf('--service');
const serviceFilter = serviceFlagIndex >= 0 ? process.argv[serviceFlagIndex + 1] : undefined;
if (serviceFlagIndex >= 0 && (!serviceFilter || serviceFilter.startsWith('--'))) {
  throw new Error('--service requires a deployment directory name');
}

async function pathExists(path) {
  return existsSync(path);
}

async function readUtf8(path) {
  return readFile(path, 'utf8');
}

function deploymentGitlinks() {
  const output = execFileSync(
    'git',
    ['ls-files', '--stage', '--', 'remote/deployments'],
    { cwd: repoRoot, encoding: 'utf8' },
  );
  return new Set(
    output
      .split(/\r?\n/)
      .map((line) => line.match(/^160000 [0-9a-f]{40} \d+\t(.+)$/)?.[1])
      .filter(Boolean),
  );
}

async function unavailableIndexedGitlinkServices(indexPath) {
  if (!existsSync(indexPath)) return [];
  const gitlinks = deploymentGitlinks();
  const current = JSON.parse(await readUtf8(indexPath));
  return (current.services ?? [])
    .filter((service) => {
      const generatedPath = service.generated?.[0];
      if (typeof generatedPath !== 'string') return false;
      const marker = '/generated/';
      const markerIndex = generatedPath.indexOf(marker);
      if (markerIndex < 0) return false;
      const deploymentPath = generatedPath.slice(0, markerIndex);
      return (
        gitlinks.has(deploymentPath) &&
        !existsSync(resolve(repoRoot, deploymentPath, '.git'))
      );
    })
    .map((service) => service.service)
    .filter((service) => typeof service === 'string')
    .sort();
}

function sortMethods(methods) {
  return [...new Set(methods)].sort(
    (left, right) => METHOD_ORDER.indexOf(left) - METHOD_ORDER.indexOf(right),
  );
}

function findMatchingParen(source, open) {
  let depth = 0;
  let quote = null;
  let escaped = false;
  for (let index = open; index < source.length; index += 1) {
    const char = source[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (char === '\\') {
        escaped = true;
      } else if (char === quote) {
        quote = null;
      }
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (char === '(') {
      depth += 1;
    } else if (char === ')') {
      depth -= 1;
      if (depth === 0) {
        return index;
      }
    }
  }
  throw new Error(`unable to match parenthesis at ${open}`);
}

function extractHandlerNames(call) {
  const handlers = [];
  for (const callName of METHOD_CALLS.keys()) {
    const pattern = new RegExp(`\\b${callName}\\s*\\(\\s*([a-zA-Z_][a-zA-Z0-9_]*)`, 'g');
    for (const match of call.matchAll(pattern)) {
      handlers.push(match[1]);
    }
  }
  return [...new Set(handlers)].sort();
}

function extractAxumRoutesFromSource(source, sourceFile, prefix = '') {
  const routes = [];
  let cursor = 0;
  for (;;) {
    const routeIndex = source.indexOf('.route(', cursor);
    if (routeIndex === -1) {
      break;
    }
    const open = source.indexOf('(', routeIndex);
    const close = findMatchingParen(source, open);
    const call = source.slice(routeIndex, close + 1);
    const pathMatch = /\.route\(\s*"([^"]+)"/.exec(call);
    if (pathMatch) {
      const methods = [];
      for (const [callName, method] of METHOD_CALLS) {
        if (new RegExp(`\\b${callName}\\s*\\(`).test(call)) {
          methods.push(method);
        }
      }
      routes.push({
        path: `${prefix}${pathMatch[1]}`,
        methods: sortMethods(methods),
        handlers: extractHandlerNames(call),
        sourceFile,
      });
    }
    cursor = close + 1;
  }
  return routes;
}

function extractPythonRoutes(source, sourceFile) {
  const routes = [];
  const methodBlocks = [
    ['GET', /def do_GET\(self\).*?(?=\n    def |\nclass |\nif __name__|$)/gs],
    ['POST', /def do_POST\(self\).*?(?=\n    def |\nclass |\nif __name__|$)/gs],
  ];
  for (const [method, blockPattern] of methodBlocks) {
    for (const blockMatch of source.matchAll(blockPattern)) {
      const block = blockMatch[0];
      for (const match of block.matchAll(/path (?:in \{([^}]+)\}|== "([^"]+)")/g)) {
        const candidates = match[1]
          ? [...match[1].matchAll(/"([^"]+)"/g)].map((item) => item[1])
          : [match[2]];
        for (const path of candidates.filter(Boolean)) {
          routes.push({ path, methods: [method], handlers: [`do_${method}`], sourceFile });
        }
      }
    }
  }
  return mergeRoutes(routes);
}

function gleamSegmentsToPath(rawSegments) {
  const parts = [];
  for (const segment of rawSegments.split(',').map((item) => item.trim()).filter(Boolean)) {
    const literal = /^"([^"]+)"$/.exec(segment);
    if (literal) {
      parts.push(literal[1]);
    } else {
      parts.push(`:${segment.replace(/[^a-zA-Z0-9_]/g, '') || 'value'}`);
    }
  }
  return `/${parts.join('/')}`;
}

function gleamAnnotationPath(path) {
  return path.replace(/<([a-zA-Z0-9_]+)>/g, ':$1');
}

function routePathWithoutQuery(path) {
  return path.split('?')[0];
}

function assertGleamAnnotationsMatchTypedRoutes(annotations, typedRoutes, sourceFile) {
  if (annotations.length === 0) {
    return;
  }
  if (typedRoutes.length === 0) {
    throw new Error(
      `Gleam API doc annotations in ${relative(repoRoot, sourceFile)} must sit beside typed \`case req.method, path\` route arms so methods stay checked against source.`,
    );
  }
  for (const annotation of annotations) {
    const method = annotation.methods[0];
    const pathBase = routePathWithoutQuery(annotation.path);
    const matchesTypedRoute = typedRoutes.some((route) => {
      return route.methods.includes(method) && routePathWithoutQuery(route.path) === pathBase;
    });
    if (!matchesTypedRoute) {
      throw new Error(
        `Gleam API doc annotation ${method} ${annotation.path} in ${relative(repoRoot, sourceFile)} does not match a typed route arm.`,
      );
    }
  }
}

function extractGleamRoutes(source, sourceFile) {
  const annotations = [];
  const typedRoutes = [];
  const inferredRoutes = [];
  for (const match of source.matchAll(/\/\/\/\/\s*(GET|POST|PATCH|DELETE|PUT|OPTIONS)\s+([^\s]+)(?:\s+(.*))?/g)) {
    annotations.push({
      path: gleamAnnotationPath(match[2]),
      methods: [match[1]],
      handlers: [],
      sourceFile,
      purposeHint: match[3]?.trim() ?? '',
    });
  }
  for (const match of source.matchAll(/\b(Get|Post|Patch|Delete|Put|Options),\s*\[([^\]]*)\]/g)) {
    typedRoutes.push({
      path: gleamSegmentsToPath(match[2]),
      methods: [match[1].toUpperCase()],
      handlers: [],
      sourceFile,
    });
  }
  assertGleamAnnotationsMatchTypedRoutes(annotations, typedRoutes, sourceFile);
  if (typedRoutes.length === 0) {
    for (const match of source.matchAll(/^\s*\[([^\]]*)\]\s*->/gm)) {
      const path = gleamSegmentsToPath(match[1]);
      if (path !== '/') {
        inferredRoutes.push({
          path,
          methods: ['GET', 'POST'],
          handlers: [],
          sourceFile,
          notes: 'Method is inferred from route body; inspect source for exact method guard.',
        });
      }
    }
  }
  return mergeRoutes([...annotations, ...typedRoutes, ...inferredRoutes]);
}

function extractNodeRoutes(source, sourceFile) {
  const routes = [];
  for (const match of source.matchAll(/\bfastify\.(get|post|patch|delete|put|options)\(\s*['"]([^'"]+)['"]/g)) {
    routes.push({
      path: match[2],
      methods: [METHOD_CALLS.get(match[1])],
      handlers: [],
      sourceFile,
    });
  }
  for (const match of source.matchAll(/request\.method === '([A-Z]+)' && url\.pathname === '([^']+)'/g)) {
    routes.push({ path: match[2], methods: [match[1]], handlers: [], sourceFile });
  }
  for (const match of source.matchAll(/request\.method === '([A-Z]+)' && \(([^)]*url\.pathname[^)]*)\)/g)) {
    for (const pathMatch of match[2].matchAll(/url\.pathname === '([^']+)'/g)) {
      routes.push({ path: pathMatch[1], methods: [match[1]], handlers: [], sourceFile });
    }
  }
  for (const match of source.matchAll(/request\.method !== '([A-Z]+)' \|\| url\.pathname !== '([^']+)'/g)) {
    routes.push({ path: match[2], methods: [match[1]], handlers: [], sourceFile });
  }
  return mergeRoutes(routes);
}

function extractFsharpRoutes(source, sourceFile) {
  const routes = [];
  const methodNames = new Map([
    ['Get', 'GET'],
    ['Post', 'POST'],
    ['Put', 'PUT'],
    ['Delete', 'DELETE'],
    ['Patch', 'PATCH'],
  ]);
  for (const match of source.matchAll(/\bapp\.Map(Get|Post|Put|Delete|Patch)\(\s*"([^"]+)"\s*,\s*toReqDelegate\s+([a-zA-Z_][a-zA-Z0-9_]*)/g)) {
    routes.push({
      path: match[2],
      methods: [methodNames.get(match[1])],
      handlers: [match[3]],
      sourceFile,
    });
  }
  return mergeRoutes(routes);
}

function javaHandlerName(expression) {
  const trimmed = expression.trim();
  const newHandler = /^new\s+([a-zA-Z_][a-zA-Z0-9_]*)/.exec(trimmed);
  if (newHandler) {
    return newHandler[1];
  }
  const staticCall = /^([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)?)\s*\(/.exec(trimmed);
  if (staticCall) {
    return staticCall[1];
  }
  const symbol = /^([a-zA-Z_][a-zA-Z0-9_]*)$/.exec(trimmed);
  return symbol?.[1] ?? null;
}

function extractJavaVertxRoutes(source, sourceFile) {
  const routes = [];
  const routePattern = /\brouter\.(get|post|patch|delete|put|options)\s*\(/g;
  for (const match of source.matchAll(routePattern)) {
    const open = source.indexOf('(', match.index);
    const close = findMatchingParen(source, open);
    const pathMatch = /^\s*"([^"]+)"/.exec(source.slice(open + 1, close));
    if (!pathMatch) {
      continue;
    }

    const semi = source.indexOf(';', close);
    if (semi === -1) {
      continue;
    }
    const chain = source.slice(close + 1, semi);
    const handlers = [];
    let cursor = 0;
    for (;;) {
      const handlerIndex = chain.indexOf('.handler(', cursor);
      if (handlerIndex === -1) {
        break;
      }
      const handlerOpen = chain.indexOf('(', handlerIndex);
      const handlerClose = findMatchingParen(chain, handlerOpen);
      const handler = javaHandlerName(chain.slice(handlerOpen + 1, handlerClose));
      if (handler) {
        handlers.push(handler);
      }
      cursor = handlerClose + 1;
    }

    routes.push({
      path: pathMatch[1],
      methods: [METHOD_CALLS.get(match[1])],
      handlers: [...new Set(handlers)].sort(),
      sourceFile,
    });
  }
  return mergeRoutes(routes);
}

function extractDartStringConstants(source) {
  const constants = new Map();
  for (const match of source.matchAll(/\bconst\s+String\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*'([^']+)'/g)) {
    constants.set(match[1], match[2]);
  }
  return constants;
}

function resolveDartPathExpression(rawExpression, constants) {
  const expression = rawExpression.trim();
  const quoted = /^'([^']+)'$/.exec(expression);
  if (quoted) {
    return quoted[1];
  }
  return constants.get(expression) ?? null;
}

function resolveDartStartsWithPath(rawExpression, constants) {
  const expression = rawExpression.trim();
  const quoted = /^'([^']+)'$/.exec(expression);
  if (!quoted) {
    return null;
  }
  const literal = quoted[1];
  const interpolated = /^\$([a-zA-Z_][a-zA-Z0-9_]*)(.*)$/.exec(literal);
  if (interpolated) {
    const prefix = constants.get(interpolated[1]);
    return prefix ? `${prefix}${interpolated[2]}` : null;
  }
  const braced = /^\$\{([a-zA-Z_][a-zA-Z0-9_]*)\}(.*)$/.exec(literal);
  if (braced) {
    const prefix = constants.get(braced[1]);
    return prefix ? `${prefix}${braced[2]}` : null;
  }
  return literal;
}

function extractDartRoutes(source, sourceFile) {
  const constants = extractDartStringConstants(source);
  const routes = [];
  for (const match of source.matchAll(/\bif\s*\(([\s\S]*?)\)\s*\{/g)) {
    const condition = match[1];
    const methods = [...condition.matchAll(/method\s*==\s*'([A-Z]+)'/g)].map((item) => item[1]);
    if (methods.length === 0) {
      continue;
    }
    const paths = [];
    for (const pathMatch of condition.matchAll(/\bpath\s*==\s*('[^']+'|[a-zA-Z_][a-zA-Z0-9_]*)/g)) {
      const path = resolveDartPathExpression(pathMatch[1], constants);
      if (path) {
        paths.push(path);
      }
    }
    for (const prefixMatch of condition.matchAll(/\bpath\.startsWith\(\s*('[^']+')\s*\)/g)) {
      const prefix = resolveDartStartsWithPath(prefixMatch[1], constants);
      if (prefix) {
        paths.push(prefix.endsWith('/') ? `${prefix}*` : `${prefix}/*`);
      }
    }
    for (const path of [...new Set(paths)]) {
      routes.push({
        path,
        methods,
        handlers: [],
        sourceFile,
      });
    }
  }
  return mergeRoutes(routes);
}

function classifyRoute(serviceName, route) {
  if (serviceName === 'rest-api-rs' && route.path.startsWith('/internal/db')) {
    return 'internal-db';
  }
  if (
    route.path === '/internal/runtime-config' ||
    route.path === '/internal/update-runtime-config' ||
    route.path === '/internal/runtime-config/reset'
  ) {
    return 'runtime-config';
  }
  if (SERVICE_ROUTE_PATHS.has(route.path) || route.path.endsWith('/healthz') || route.path.endsWith('/metrics')) {
    return 'service';
  }
  return 'user-generated';
}

function routePurpose(routeType, route) {
  if (route.purposeHint) {
    return route.purposeHint;
  }
  if (route.path === '/docs/api' || route.path === '/api/docs') {
    return 'Human-readable generated API documentation.';
  }
  if (route.path === '/api/docs.json') {
    return 'Machine-readable generated API route metadata.';
  }
  if (route.path === '/graphql' || route.path === '/api/graphql') {
    return 'GraphQL endpoint for typed remote REST API queries, guarded subservice calls, and the optional GraphiQL IDE on GET.';
  }
  if (route.path === '/graphql/schema' || route.path === '/api/graphql/schema') {
    return 'Machine-readable GraphQL SDL schema for the remote REST API service; protected by internal auth by default.';
  }
  if (route.path === '/api-docs' || route.path === '/api-docs/') {
    return 'Central generated API documentation index.';
  }
  if (route.path === '/api-docs.json') {
    return 'Machine-readable central generated API documentation index.';
  }
  if (route.path === '/healthz' || route.path.endsWith('/healthz')) {
    return 'Health check.';
  }
  if (route.path === '/livez') {
    return 'Liveness check.';
  }
  if (route.path === '/readyz') {
    return 'Readiness check.';
  }
  if (route.path === '/metrics' || route.path.endsWith('/metrics')) {
    return 'Prometheus metrics.';
  }
  if (route.path === '/') {
    return 'Service descriptor, home redirect, or root RPC endpoint.';
  }
  if (routeType === 'internal-db') {
    return 'Internal operator database inspection route. Not part of the public REST contract.';
  }
  if (routeType === 'runtime-config') {
    return 'dd-runtime-config subscriber surface. Auto-mounted by the shared receiver helper (see remote/libs/runtime-config-client-rs).';
  }
  return 'Custom code-first route derived from the service router.';
}

function routeAuth(routeType, route) {
  if (routeType === 'internal-db') {
    return 'operator secret';
  }
  if (routeType === 'runtime-config') {
    return route.methods.includes('POST') ? 'X-Server-Auth (RUNTIME_CONFIG_SERVER_SECRET)' : 'service-defined';
  }
  if (route.path.includes('/webhooks/')) {
    return 'webhook signature';
  }
  if (route.path === '/graphql/schema' || route.path === '/api/graphql/schema') {
    return 'operator secret by default';
  }
  if (
    route.path === '/healthz' ||
    route.path === '/livez' ||
    route.path === '/readyz' ||
    route.path === '/metrics' ||
    route.path === '/'
  ) {
    return 'public';
  }
  if (
    route.path === '/docs/api' ||
    route.path === '/api/docs' ||
    route.path === '/api/docs.json' ||
    route.path === '/api-docs' ||
    route.path === '/api-docs/' ||
    route.path === '/api-docs.json'
  ) {
    return 'public';
  }
  return 'service-defined';
}

function mergeRoutes(routes) {
  const byPath = new Map();
  for (const route of routes) {
    if (!route.path || route.path === '//' || route.path.includes('..')) {
      continue;
    }
    const key = route.path;
    const current = byPath.get(key) ?? {
      ...route,
      methods: [],
      handlers: [],
      sourceFiles: new Set(),
    };
    current.methods = sortMethods([...(current.methods ?? []), ...(route.methods ?? [])]);
    current.handlers = [...new Set([...(current.handlers ?? []), ...(route.handlers ?? [])])].sort();
    current.sourceFiles.add(route.sourceFile);
    if (route.purposeHint && !current.purposeHint) {
      current.purposeHint = route.purposeHint;
    }
    if (route.notes && !current.notes) {
      current.notes = route.notes;
    }
    byPath.set(key, current);
  }
  return [...byPath.values()]
    .map((route) => ({
      ...route,
      sourceFiles: [...route.sourceFiles].sort(),
    }))
    .sort((left, right) => left.path.localeCompare(right.path));
}

function normalizeRoutes(serviceName, rawRoutes) {
  return mergeRoutes(rawRoutes).map((route) => {
    const routeType = classifyRoute(serviceName, route);
    return {
      path: route.path,
      methods: route.methods,
      routeType,
      implementation: routeType === 'internal-db' ? 'internal-operator' : routeType === 'service' ? 'service' : 'code-first',
      auth: routeAuth(routeType, route),
      purpose: routePurpose(routeType, route),
      handlers: route.handlers ?? [],
      sourceFiles: route.sourceFiles.map((file) => relative(repoRoot, file).split(sep).join('/')),
      notes: route.notes ?? '',
    };
  });
}

async function rustDependsOnRuntimeConfigClient(deploymentDir) {
  const cargo = join(deploymentDir, 'Cargo.toml');
  if (!(await pathExists(cargo))) {
    return false;
  }
  const source = await readUtf8(cargo);
  return /dd-runtime-config-client\s*=/.test(source);
}

async function gleamDependsOnRuntimeConfigClient(deploymentDir) {
  const gleamToml = join(deploymentDir, 'gleam.toml');
  if (!(await pathExists(gleamToml))) {
    return false;
  }
  const source = await readUtf8(gleamToml);
  return /dd_runtime_config_client\s*=/.test(source);
}

async function pythonContainsRuntimeConfigHandler(file) {
  // The Python helper lives inline in the service file. Detect by class
  // name so we don't drift if the route constants are renamed.
  if (!(await pathExists(file))) {
    return false;
  }
  const source = await readUtf8(file);
  return source.includes('class RuntimeConfigClient');
}

async function nodeRegistersRuntimeConfigRoutes(file) {
  if (!(await pathExists(file))) {
    return false;
  }
  const source = await readUtf8(file);
  return source.includes('registerRuntimeConfigRoutes(');
}

function injectRuntimeConfigRoutes(rawRoutes, sourceFile) {
  // Drop any locally-parsed copies first so the injected entries are
  // authoritative on methods/auth/etc. Path-only Gleam routers (e.g.
  // gleamlang-server, gleam-lambda-runner) infer both GET and POST for
  // every arm; we want the docs to reflect the actual canonical methods
  // exposed by the shared client.
  const canonicalPaths = new Set(RUNTIME_CONFIG_SUBSCRIBER_ROUTES.map((route) => route.path));
  for (let index = rawRoutes.length - 1; index >= 0; index -= 1) {
    if (canonicalPaths.has(rawRoutes[index].path)) {
      rawRoutes.splice(index, 1);
    }
  }
  for (const route of RUNTIME_CONFIG_SUBSCRIBER_ROUTES) {
    rawRoutes.push({
      path: route.path,
      methods: route.methods.slice(),
      handlers: route.handlers.slice(),
      purposeHint: route.purposeHint,
      sourceFile,
    });
  }
}

async function discoverRustServices() {
  const deploymentsDir = resolve(repoRoot, 'remote/deployments');
  const entries = await readdir(deploymentsDir, { withFileTypes: true });
  const services = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || !RUST_DEPLOYMENT_ALLOWLIST.has(entry.name)) {
      continue;
    }
    const deploymentDir = join(deploymentsDir, entry.name);
    const main = join(
      deploymentDir,
      RUST_ROUTE_SOURCE_OVERRIDES.get(entry.name) ?? 'src/main.rs',
    );
    if (!(await pathExists(main))) {
      continue;
    }
    const source = await readUtf8(main);
    if (!source.includes('.route(')) {
      continue;
    }
    const rawRoutes = extractAxumRoutesFromSource(source, main);
    if (entry.name === 'rest-api-rs') {
      const dbRoutes = join(deploymentDir, 'src/db_routes.rs');
      if ((await pathExists(dbRoutes)) && source.includes('/internal/db')) {
        // Internal DB tooling is intentionally not part of the public REST
        // docs unless the main router exposes its private mount point.
        rawRoutes.push(...extractAxumRoutesFromSource(await readUtf8(dbRoutes), dbRoutes, '/internal/db'));
      }
      const graphqlRoutes = join(deploymentDir, 'src/graphql_routes.rs');
      if ((await pathExists(graphqlRoutes)) && source.includes('graphql_routes::router()')) {
        rawRoutes.push(...extractAxumRoutesFromSource(await readUtf8(graphqlRoutes), graphqlRoutes));
      }
    }
    if (entry.name === 'contract-service-rs' && source.includes('blockchain::router()')) {
      // The keyless blockchain feature suite mounts its routes from per-feature
      // submodules under src/blockchain/. Scan those source files so the docs
      // stay derived from `.route("...")` declarations (no manual inventory).
      const blockchainDir = join(deploymentDir, 'src/blockchain');
      if (await pathExists(blockchainDir)) {
        const blockchainFiles = (await readdir(blockchainDir)).filter((file) => file.endsWith('.rs')).sort();
        for (const file of blockchainFiles) {
          const filePath = join(blockchainDir, file);
          const fileSource = await readUtf8(filePath);
          if (fileSource.includes('.route(')) {
            rawRoutes.push(...extractAxumRoutesFromSource(fileSource, filePath));
          }
        }
      }
    }
    if (entry.name === 'contract-service-rs' && source.includes('solana_features::router()')) {
      const featuresFile = join(deploymentDir, 'src/solana_features.rs');
      if (await pathExists(featuresFile)) {
        const featuresSource = await readUtf8(featuresFile);
        rawRoutes.push(...extractAxumRoutesFromSource(featuresSource, featuresFile));
      }
    }
    if (await rustDependsOnRuntimeConfigClient(deploymentDir)) {
      injectRuntimeConfigRoutes(rawRoutes, join(repoRoot, 'remote/libs/runtime-config-client-rs/src/lib.rs'));
    }
    services.push({
      service: entry.name,
      language: 'rust',
      deploymentDir,
      routes: normalizeRoutes(entry.name, rawRoutes),
    });
  }
  return services;
}

async function discoverExtraServices() {
  const specs = [
    {
      service: 'ai-ml-pipeline',
      language: 'python',
      file: 'remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py',
      parser: extractPythonRoutes,
    },
    {
      service: 'browser-test-server',
      language: 'node',
      file: 'remote/deployments/browser-test-server/src/server.ts',
      parser: extractNodeRoutes,
      deploymentDir: 'remote/deployments/browser-test-server',
    },
    {
      service: 'dart-server',
      language: 'dart',
      files: [
        'remote/deployments/dart-server/bin/server.dart',
        'remote/deployments/dart-server/lib/server/http_isolate.dart',
        'remote/deployments/dart-server/lib/server/gateway_isolate.dart',
      ],
      parser: extractDartRoutes,
      deploymentDir: 'remote/deployments/dart-server',
    },
    {
      service: 'dev-server',
      language: 'node',
      file: 'remote/deployments/dev-server/src/server.ts',
      parser: extractNodeRoutes,
      deploymentDir: 'remote/deployments/dev-server',
    },
    {
      service: 'fsharp-ws-server',
      language: 'fsharp',
      file: 'remote/deployments/fsharp-ws-server/Program.fs',
      parser: extractFsharpRoutes,
      deploymentDir: 'remote/deployments/fsharp-ws-server',
    },
    {
      service: 'gleam-lambda-runner',
      language: 'gleam',
      file: 'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/http_server.gleam',
      deploymentDir: 'remote/deployments/gleam-lambda-runner',
      parser: extractGleamRoutes,
    },
    {
      service: 'gleam-mcp-server',
      language: 'gleam',
      file: 'remote/deployments/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
      deploymentDir: 'remote/deployments/gleam-mcp-server',
      parser: extractGleamRoutes,
    },
    {
      service: 'gleamlang-server',
      language: 'gleam',
      file: 'remote/deployments/gleamlang-server/src/gleamlang_server/http_server.gleam',
      deploymentDir: 'remote/deployments/gleamlang-server',
      parser: extractGleamRoutes,
    },
    {
      service: 'gleamlang-ws-server',
      language: 'gleam',
      file: 'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server/http_server.gleam',
      deploymentDir: 'remote/deployments/gleamlang-ws-server',
      parser: extractGleamRoutes,
    },
    {
      service: 'gleamlang-presence-server',
      language: 'gleam',
      file: 'remote/deployments/gleamlang-presence-server/src/gleamlang_presence_server/http_server.gleam',
      deploymentDir: 'remote/deployments/gleamlang-presence-server',
      parser: extractGleamRoutes,
    },
    {
      service: 'gleamlang-server-nats-bridge',
      language: 'node',
      file: 'remote/deployments/gleamlang-server/nats-bridge.mjs',
      parser: extractNodeRoutes,
      deploymentDir: 'remote/deployments/gleamlang-server',
      outputName: 'api-docs.nats-bridge',
    },
    {
      service: 'spark-pipeline-server',
      language: 'java',
      file: 'remote/deployments/spark-pipeline-server/src/main/java/com/oresoftware/dd/sparkpipeline/MainVerticle.java',
      parser: extractJavaVertxRoutes,
      deploymentDir: 'remote/deployments/spark-pipeline-server',
    },
    {
      service: 'web-scraper-service',
      language: 'node',
      file: 'remote/deployments/web-scraper-service/src/server.ts',
      parser: extractNodeRoutes,
      deploymentDir: 'remote/deployments/web-scraper-service',
    },
  ];
  const services = [];
  for (const spec of specs) {
    const files = (spec.files ?? [spec.file]).map((file) => resolve(repoRoot, file));
    if (!(await pathExists(files[0]))) {
      continue;
    }
    const rawRoutes = [];
    for (const file of files) {
      if (await pathExists(file)) {
        rawRoutes.push(...spec.parser(await readUtf8(file), file));
      }
    }
    const deploymentDir = resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(files[0])));
    // Python services: the helper is inline so we look for the marker class
    // directly. Gleam services: detect the path dep in gleam.toml. Either
    // way we inject the same three routes the Rust client emits.
    if (spec.language === 'python' && (await pythonContainsRuntimeConfigHandler(files[0]))) {
      injectRuntimeConfigRoutes(rawRoutes, files[0]);
    } else if (spec.language === 'node' && (await nodeRegistersRuntimeConfigRoutes(files[0]))) {
      injectRuntimeConfigRoutes(
        rawRoutes,
        join(repoRoot, 'remote/deployments/dev-server/src/runtime-config.ts'),
      );
    } else if (
      spec.language === 'gleam' &&
      (await gleamDependsOnRuntimeConfigClient(deploymentDir))
    ) {
      injectRuntimeConfigRoutes(
        rawRoutes,
        join(repoRoot, 'remote/libs/runtime-config-client-gleam/src/dd_runtime_config_client.gleam'),
      );
    }
    services.push({
      service: spec.service,
      language: spec.language,
      deploymentDir,
      moduleDir: dirname(files[0]),
      outputName: spec.outputName ?? 'api-docs',
      routes: normalizeRoutes(spec.service, rawRoutes),
    });
  }
  return services;
}

const OPENAPI_METHODS = new Set(['GET', 'PUT', 'POST', 'DELETE', 'OPTIONS', 'HEAD', 'PATCH', 'TRACE']);

function openApiPathFromSource(sourcePath) {
  const pathOnly = sourcePath.split('?', 1)[0] || '/';
  let wildcardIndex = 0;
  const normalized = pathOnly
    .replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, '{$1}')
    .replace(/<([a-zA-Z_][a-zA-Z0-9_]*)>/g, '{$1}')
    .replace(/\*/g, () => {
      const suffix = wildcardIndex === 0 ? '' : String(wildcardIndex + 1);
      wildcardIndex += 1;
      return `{wildcard${suffix}}`;
    });
  return normalized.startsWith('/') ? normalized : `/${normalized}`;
}

function openApiPathParameters(path) {
  return [...path.matchAll(/\{([^}]+)\}/g)].map((match) => ({
    name: match[1],
    in: 'path',
    required: true,
    schema: { type: 'string' },
  }));
}

function openApiQueryParameters(sourcePath) {
  const query = sourcePath.split('?', 2)[1];
  if (!query) {
    return [];
  }
  const names = query
    .split('&')
    .map((part) => part.split('=', 1)[0])
    .filter(Boolean);
  return [...new Set(names)].sort().map((name) => ({
    name,
    in: 'query',
    required: false,
    schema: { type: 'string' },
  }));
}

function mergeOpenApiParameters(...groups) {
  const byKey = new Map();
  for (const parameter of groups.flat()) {
    const key = `${parameter.in}:${parameter.name}`;
    const current = byKey.get(key);
    if (!current) {
      byKey.set(key, structuredClone(parameter));
      continue;
    }
    current.required = current.in === 'path' ? true : current.required === true && parameter.required === true;
  }
  return [...byKey.values()].sort((left, right) => {
    return `${left.in}:${left.name}`.localeCompare(`${right.in}:${right.name}`);
  });
}

function openApiOperationId(service, route, method) {
  const path = openApiPathFromSource(route.path);
  const identity = [service, method, path].join('\0');
  const digest = createHash('sha256').update(identity).digest('hex').slice(0, 12);
  const stem = [service, method, path]
    .join('_')
    .replace(/[^a-zA-Z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .toLowerCase();
  return `${stem || 'operation'}_${digest}`;
}

function openApiVisibility(route) {
  if (route.routeType === 'internal-db' || route.routeType === 'runtime-config') {
    return 'internal';
  }
  return route.auth === 'public' ? 'public' : 'internal';
}

function openApiSecurity(route) {
  if (openApiVisibility(route) === 'public') {
    return [];
  }
  if (route.auth.includes('X-Server-Auth')) {
    return [{ serverAuth: [] }];
  }
  if (route.auth.includes('operator secret')) {
    return [{ operatorSecret: [] }];
  }
  if (route.auth.includes('webhook signature')) {
    return [{ webhookSignature: [] }];
  }
  return undefined;
}

function buildOpenApi(docs) {
  const paths = {};
  const tags = new Set();
  let operationCount = 0;
  for (const route of docs.routes) {
    const path = openApiPathFromSource(route.path);
    const pathItem = paths[path] ?? {};
    tags.add(route.routeType);
    for (const method of route.methods) {
      if (!OPENAPI_METHODS.has(method)) {
        continue;
      }
      const methodName = method.toLowerCase();
      const visibility = openApiVisibility(route);
      const existing = pathItem[methodName];
      if (existing) {
        if (existing['x-dd-auth'] !== route.auth || existing['x-dd-visibility'] !== visibility || existing['x-dd-route-type'] !== route.routeType) {
          throw new Error(`ambiguous OpenAPI merge for ${docs.service} ${method} ${path}: query/path variants must share auth, visibility, and route type`);
        }
        existing.parameters = mergeOpenApiParameters(
          existing.parameters ?? [],
          openApiQueryParameters(route.path),
        );
        existing['x-dd-handlers'] = [...new Set([...(existing['x-dd-handlers'] ?? []), ...(route.handlers ?? [])])].sort();
        existing['x-dd-source-files'] = [...new Set([...(existing['x-dd-source-files'] ?? []), ...(route.sourceFiles ?? [])])].sort();
        existing['x-dd-source-paths'] = [...new Set([...(existing['x-dd-source-paths'] ?? []), route.path])].sort();
        continue;
      }
      const security = openApiSecurity(route);
      const operation = {
        operationId: openApiOperationId(docs.service, route, method),
        summary: route.purpose,
        description: route.notes || route.purpose,
        tags: [route.routeType],
        parameters: mergeOpenApiParameters(
          openApiPathParameters(path),
          openApiQueryParameters(route.path),
        ),
        responses: {
          default: {
            description: 'Response produced by the registered service handler.',
          },
        },
        'x-dd-auth': route.auth,
        'x-dd-handlers': route.handlers,
        'x-dd-implementation': route.implementation,
        'x-dd-route-type': route.routeType,
        'x-dd-source-files': route.sourceFiles,
        'x-dd-source-path': route.path,
        'x-dd-source-paths': [route.path],
        'x-dd-visibility': visibility,
      };
      if (security !== undefined) {
        operation.security = security;
      }
      pathItem[methodName] = operation;
      operationCount += 1;
    }
    paths[path] = pathItem;
  }

  return {
    openapi: '3.1.0',
    jsonSchemaDialect: 'https://json-schema.org/draft/2020-12/schema',
    info: {
      title: `${docs.service} API`,
      version: '0.1.0',
      description:
        'Generated from the service route registrations. Request and response schemas become authoritative as this service migrates to its native typed OpenAPI adapter.',
    },
    tags: [...tags].sort().map((name) => ({ name })),
    paths,
    components: {
      securitySchemes: {
        serverAuth: {
          type: 'apiKey',
          in: 'header',
          name: 'X-Server-Auth',
        },
        operatorSecret: {
          type: 'apiKey',
          in: 'header',
          name: 'X-Operator-Secret',
        },
        webhookSignature: {
          type: 'apiKey',
          in: 'header',
          name: 'X-Webhook-Signature',
        },
      },
    },
    'x-dd-contract-scope': 'internal',
    'x-dd-generated-by': docs.generatedBy,
    'x-dd-language': docs.language,
    'x-dd-operation-count': operationCount,
    'x-dd-route-count': docs.routeCount,
    'x-dd-service': docs.service,
    'x-dd-standard-docs-routes': docs.standardDocsRoutes,
  };
}

function operationEntriesForDocument(document) {
  const entries = [];
  for (const pathItem of Object.values(document.paths ?? {})) {
    for (const method of [...OPENAPI_METHODS].map((value) => value.toLowerCase())) {
      if (pathItem[method]) {
        entries.push({ method, operation: pathItem[method] });
      }
    }
  }
  return entries;
}

function buildPublicOpenApi(openapi) {
  const document = structuredClone(openapi);
  for (const [path, pathItem] of Object.entries(document.paths)) {
    for (const method of [...OPENAPI_METHODS].map((value) => value.toLowerCase())) {
      if (pathItem[method]?.['x-dd-visibility'] !== 'public') {
        delete pathItem[method];
      }
    }
    if (Object.keys(pathItem).length === 0) {
      delete document.paths[path];
    }
  }
  document.info.title = `${document.info.title} (public)`;
  document.info.description =
    'Fail-closed public subset. Only operations explicitly marked public are included.';
  document['x-dd-contract-scope'] = 'public';
  document['x-dd-route-count'] = new Set(
    operationEntriesForDocument(document).flatMap((entry) => entry.operation['x-dd-source-paths'] ?? []),
  ).size;
  document['x-dd-operation-count'] = Object.values(document.paths).reduce(
    (count, pathItem) =>
      count +
      [...OPENAPI_METHODS]
        .map((value) => value.toLowerCase())
        .filter((method) => pathItem[method])
        .length,
    0,
  );
  return document;
}

function buildPublicDocs(docs) {
  const routes = docs.routes.filter((route) => openApiVisibility(route) === 'public');
  const routeTypeCounts = routes.reduce((acc, route) => {
    acc[route.routeType] = (acc[route.routeType] ?? 0) + 1;
    return acc;
  }, {});
  return {
    ...docs,
    routeCount: routes.length,
    routeTypeCounts,
    routes,
    contractScope: 'public',
  };
}

function buildDocs(service) {
  const routes = service.routes;
  const routeTypeCounts = routes.reduce((acc, route) => {
    acc[route.routeType] = (acc[route.routeType] ?? 0) + 1;
    return acc;
  }, {});
  return {
    ok: true,
    generatedBy: 'remote/tools/generate-api-docs.mjs',
    service: service.service,
    language: service.language,
    routeCount: routes.length,
    routeTypeCounts,
    standardDocsRoutes: STANDARD_DOCS_ROUTES,
    routes,
  };
}

function assertStandardDocsRoutes(service) {
  for (const path of STANDARD_DOCS_ROUTES) {
    const route = service.routes.find((candidate) => {
      return candidate.path === path && candidate.methods.includes('GET');
    });
    if (!route) {
      throw new Error(
        `${service.service} must mount GET ${path} before generating API docs. Generated docs are committed artifacts, not a substitute for the service endpoint.`,
      );
    }
  }
}

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function renderDocsHtml(docs) {
  const rows = docs.routes
    .map((route) => {
      const methods = route.methods.map((method) => `<span class="method">${escapeHtml(method)}</span>`).join('');
      const handlers = route.handlers.length
        ? route.handlers.map((handler) => `<code>${escapeHtml(handler)}</code>`).join(' ')
        : '<span class="muted">derived from route pattern</span>';
      return `<tr>
  <td data-label="Type"><span class="badge ${escapeHtml(route.routeType)}">${escapeHtml(route.routeType)}</span><div class="muted">${escapeHtml(route.implementation)}</div></td>
  <td data-label="Methods"><div class="methods">${methods}</div></td>
  <td data-label="Path"><code>${escapeHtml(route.path)}</code></td>
  <td data-label="Purpose">${escapeHtml(route.purpose)}${route.notes ? `<div class="muted">${escapeHtml(route.notes)}</div>` : ''}</td>
  <td data-label="Handlers">${handlers}</td>
  <td data-label="Auth">${escapeHtml(route.auth)}</td>
</tr>`;
    })
    .join('\n');
  const optionalSummaryRows = [
    docs.routeTypeCounts['internal-db']
      ? `      <span>${docs.routeTypeCounts['internal-db']} internal-db</span>`
      : null,
    docs.routeTypeCounts['runtime-config']
      ? `      <span>${docs.routeTypeCounts['runtime-config']} runtime-config</span>`
      : null,
  ]
    .filter(Boolean)
    .join('\n');
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(docs.service)} API docs</title>
  <style>
    :root { color-scheme: light; --bg:#f7f8fa; --panel:#fff; --ink:#17202a; --muted:#5b6672; --line:#d8dee6; --code:#eef2f6; --service:#52687a; --custom:#1f6f5b; --internal:#8a5a12; --runtime:#3a4d8a; }
    * { box-sizing: border-box; }
    body { margin:0; background:var(--bg); color:var(--ink); font:14px/1.5 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    header, main { width:min(1180px, calc(100% - 32px)); margin:0 auto; }
    header { padding:28px 0 18px; }
    h1 { margin:0 0 6px; font-size:30px; line-height:1.15; letter-spacing:0; }
    p { margin:0; color:var(--muted); }
    .summary { display:flex; flex-wrap:wrap; gap:10px; margin-top:18px; }
    .summary span, .badge { display:inline-flex; align-items:center; min-height:26px; border:1px solid var(--line); border-radius:6px; padding:3px 9px; background:var(--panel); white-space:nowrap; }
    .badge { font-size:12px; font-weight:700; text-transform:uppercase; letter-spacing:0; }
    .service { color:var(--service); }
    .user-generated { color:var(--custom); }
    .internal-db { color:var(--internal); }
    .runtime-config { color:var(--runtime); }
    table { width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line); border-radius:8px; overflow:hidden; }
    th, td { padding:12px; border-bottom:1px solid var(--line); vertical-align:top; text-align:left; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; letter-spacing:0; background:#fbfcfd; }
    tr:last-child td { border-bottom:0; }
    code { display:inline-block; max-width:100%; padding:2px 5px; border-radius:5px; background:var(--code); overflow-wrap:anywhere; font-family:ui-monospace, "SFMono-Regular", Consolas, monospace; font-size:12px; }
    .methods { display:flex; flex-wrap:wrap; gap:5px; }
    .method { background:#17202a; color:#fff; border-radius:5px; padding:2px 6px; font-size:12px; font-weight:700; }
    .muted { color:var(--muted); font-size:12px; margin-top:4px; }
    @media (max-width:760px) {
      header, main { width:min(100% - 20px, 1180px); }
      table, tbody, tr, td { display:block; width:100%; }
      thead { display:none; }
      tr { border-bottom:1px solid var(--line); }
      td { border-bottom:0; padding:8px 10px; }
      td::before { display:block; margin-bottom:3px; color:var(--muted); font-size:11px; font-weight:700; text-transform:uppercase; content:attr(data-label); }
    }
  </style>
</head>
<body>
  <header>
    <h1>${escapeHtml(docs.service)} API docs</h1>
    <p>Generated from route declarations in source. Standard routes: <code>/docs/api</code>, <code>/api/docs</code>, <code>/api/docs.json</code>.</p>
    <div class="summary">
      <span>${docs.routeCount} routes</span>
      <span>${escapeHtml(docs.language)}</span>
      <span>${docs.routeTypeCounts.service ?? 0} service</span>
      <span>${docs.routeTypeCounts['user-generated'] ?? 0} user-generated</span>${optionalSummaryRows ? `\n${optionalSummaryRows}` : ''}
    </div>
  </header>
  <main>
    <table>
      <thead><tr><th>Type</th><th>Methods</th><th>Path</th><th>Purpose</th><th>Handlers</th><th>Auth</th></tr></thead>
      <tbody>
${rows}
      </tbody>
    </table>
  </main>
</body>
</html>
`;
}

function renderDocsIndexHtml(services) {
  const totalRoutes = services.reduce((sum, service) => sum + service.routeCount, 0);
  const serviceRows = services
    .map((service) => {
      const publicJson = service.generated[0];
      const publicHtml = service.generated[1];
      return `<tr>
  <td data-label="Service"><code>${escapeHtml(service.service)}</code></td>
  <td data-label="Language">${escapeHtml(service.language)}</td>
  <td data-label="Routes">${service.routeCount}</td>
  <td data-label="Public docs"><code>${escapeHtml(publicHtml)}</code></td>
  <td data-label="Public OpenAPI"><code>${escapeHtml(publicJson)}</code></td>
</tr>`;
    })
    .join('\n');
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>dd runtime API docs</title>
  <style>
    :root { color-scheme: light; --bg:#f7f8fa; --panel:#fff; --ink:#17202a; --muted:#5b6672; --line:#d8dee6; --code:#eef2f6; }
    * { box-sizing:border-box; }
    body { margin:0; background:var(--bg); color:var(--ink); font:14px/1.5 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }
    header,main { width:min(1180px,calc(100% - 32px)); margin:0 auto; }
    header { padding:28px 0 18px; }
    h1 { margin:0 0 6px; font-size:30px; line-height:1.15; }
    p { margin:0; color:var(--muted); }
    .summary { display:flex; flex-wrap:wrap; gap:10px; margin-top:18px; }
    .summary span { display:inline-flex; align-items:center; min-height:26px; border:1px solid var(--line); border-radius:6px; padding:3px 9px; background:var(--panel); }
    table { width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line); border-radius:8px; overflow:hidden; }
    th,td { padding:11px 12px; border-bottom:1px solid var(--line); vertical-align:top; text-align:left; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; background:#fbfcfd; }
    tr:last-child td { border-bottom:0; }
    code { display:inline-block; max-width:100%; padding:2px 5px; border-radius:5px; background:var(--code); overflow-wrap:anywhere; font-family:ui-monospace,"SFMono-Regular",Consolas,monospace; font-size:12px; }
    @media (max-width:760px) {
      header,main { width:min(100% - 20px,1180px); }
      table,tbody,tr,td { display:block; width:100%; }
      thead { display:none; }
      tr { border-bottom:1px solid var(--line); }
      td { border-bottom:0; padding:8px 10px; }
      td::before { display:block; margin-bottom:3px; color:var(--muted); font-size:11px; font-weight:700; text-transform:uppercase; content:attr(data-label); }
    }
  </style>
</head>
<body>
  <header>
    <h1>dd runtime API docs</h1>
    <p>Public-only fleet index. Internal contracts are unserved build artifacts used for private SDK generation and parity checks.</p>
    <div class="summary">
      <span>${services.length} services</span>
      <span>${totalRoutes} registered routes</span>
      <span>central JSON <code>/api-docs.json</code></span>
    </div>
  </header>
  <main>
    <table>
      <thead><tr><th>Service</th><th>Language</th><th>Registered routes</th><th>Public docs</th><th>Public OpenAPI</th></tr></thead>
      <tbody>
${serviceRows}
      </tbody>
    </table>
  </main>
</body>
</html>
`;
}

function gleamString(value) {
  return JSON.stringify(value);
}

function gleamApiDocsModule(docs, openapi) {
  return `// Generated by remote/tools/generate-api-docs.mjs. Do not edit by hand.
import gleam/bytes_tree
import gleam/http/response
import mist

const api_docs_html = ${gleamString(renderDocsHtml(docs))}

const api_docs_json = ${gleamString(`${JSON.stringify(openapi, null, 2)}\n`)}

pub fn html() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/html; charset=utf-8")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(api_docs_html)))
}

pub fn json() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "application/json; charset=utf-8")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(api_docs_json)))
}
`;
}

async function writeOrCheck(path, content) {
  if (checkOnly) {
    let existing = null;
    try {
      existing = await readUtf8(path);
    } catch {
      throw new Error(`missing generated API docs file: ${relative(repoRoot, path)}`);
    }
    if (existing !== content) {
      throw new Error(`stale generated API docs file: ${relative(repoRoot, path)}. Run node remote/tools/generate-api-docs.mjs`);
    }
    return;
  }
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

function canonicalGeneratedArtifacts(publicOpenapiPath) {
  if (
    typeof publicOpenapiPath !== 'string' ||
    !publicOpenapiPath.endsWith('.json') ||
    publicOpenapiPath.endsWith('.internal.json') ||
    publicOpenapiPath.endsWith('.metadata.json')
  ) {
    throw new Error(`invalid public OpenAPI artifact path: ${publicOpenapiPath}`);
  }
  return [
    publicOpenapiPath,
    publicOpenapiPath.replace(/\.json$/, '.html'),
    publicOpenapiPath.replace(/\.json$/, '.internal.json'),
    publicOpenapiPath.replace(/\.json$/, '.metadata.json'),
  ];
}

function normalizeIndexedServiceArtifacts(service) {
  const canonical = service.generated?.find(
    (path) =>
      typeof path === 'string' &&
      path.endsWith('.json') &&
      !path.endsWith('.internal.json') &&
      !path.endsWith('.metadata.json'),
  );
  if (!canonical) {
    throw new Error(`${service.service ?? 'unknown service'} has no public OpenAPI JSON artifact`);
  }
  return { ...service, generated: canonicalGeneratedArtifacts(canonical) };
}

async function main() {
  const services = [...await discoverRustServices(), ...await discoverExtraServices()]
    .filter((service) => service.routes.length > 0)
    .filter((service) => !serviceFilter || service.service === serviceFilter)
    .sort((left, right) => left.service.localeCompare(right.service));
  if (serviceFilter && services.length === 0) {
    throw new Error(`unknown or route-less API docs service: ${serviceFilter}`);
  }
  const index = [];
  const indexItems = [];
  for (const service of services) {
    assertStandardDocsRoutes(service);
    const docs = buildDocs(service);
    const internalOpenapi = buildOpenApi(docs);
    const publicOpenapi = buildPublicOpenApi(internalOpenapi);
    const publicDocs = buildPublicDocs(docs);
    const outputBase = service.outputName ?? 'api-docs';
    const generatedDir = join(service.deploymentDir, 'generated');
    const publicJson = `${JSON.stringify(publicOpenapi, null, 2)}\n`;
    const internalJson = `${JSON.stringify(internalOpenapi, null, 2)}\n`;
    const metadataJson = `${JSON.stringify(docs, null, 2)}\n`;
    const html = renderDocsHtml(publicDocs);
    const publicOpenapiPath = relative(
      repoRoot,
      join(generatedDir, `${outputBase}.json`),
    ).split(sep).join('/');
    const generated = canonicalGeneratedArtifacts(publicOpenapiPath);
    await writeOrCheck(join(generatedDir, `${outputBase}.json`), publicJson);
    await writeOrCheck(join(generatedDir, `${outputBase}.internal.json`), internalJson);
    await writeOrCheck(join(generatedDir, `${outputBase}.metadata.json`), metadataJson);
    await writeOrCheck(join(generatedDir, `${outputBase}.html`), html);
    if (service.language === 'gleam' && outputBase === 'api-docs' && service.moduleDir) {
      await writeOrCheck(
        join(service.moduleDir, 'api_docs.gleam'),
        gleamApiDocsModule(publicDocs, publicOpenapi),
      );
    }
    index.push({
      service: service.service,
      language: service.language,
      routeCount: docs.routeCount,
      routeTypeCounts: docs.routeTypeCounts,
      generated,
    });
    indexItems.push({ docs: publicDocs, generated });
  }
  const indexPayload = {
    ok: true,
    generatedBy: 'remote/tools/generate-api-docs.mjs',
    centralDocsRoutes: CENTRAL_DOCS_ROUTES,
    standardDocsRoutes: STANDARD_DOCS_ROUTES,
    services: index,
  };
  if (!serviceFilter) {
    const centralIndexJson = resolve(
      repoRoot,
      'remote/deployments/generated-api-docs-index.json',
    );
    const centralIndexHtml = resolve(
      repoRoot,
      'remote/deployments/generated-api-docs-index.html',
    );
    const unavailableServices = await unavailableIndexedGitlinkServices(centralIndexJson);
    if (unavailableServices.length > 0) {
      for (const path of [centralIndexJson, centralIndexHtml]) {
        if (!existsSync(path)) {
          throw new Error(
            `missing central API docs index during partial checkout: ${relative(repoRoot, path)}`,
          );
        }
      }

      const unavailable = new Set(unavailableServices);
      const currentPayload = JSON.parse(await readUtf8(centralIndexJson));
      const currentByService = new Map(
        (currentPayload.services ?? []).map((service) => [service.service, service]),
      );
      const availableByService = new Map(index.map((service) => [service.service, service]));
      const serviceNames = [...new Set([...currentByService.keys(), ...availableByService.keys()])].sort();
      const mergedServices = serviceNames.map((serviceName) => {
        const availableService = availableByService.get(serviceName);
        if (availableService) {
          return normalizeIndexedServiceArtifacts(availableService);
        }
        const preservedService = currentByService.get(serviceName);
        if (!preservedService || !unavailable.has(serviceName)) {
          throw new Error(
            `central API docs index contains non-gitlink service that is no longer discoverable: ${serviceName}`,
          );
        }
        return normalizeIndexedServiceArtifacts(preservedService);
      });
      const mergedPayload = { ...indexPayload, services: mergedServices };
      await writeOrCheck(
        centralIndexJson,
        `${JSON.stringify(mergedPayload, null, 2)}\n`,
      );
      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(mergedServices));
      console.log(
        `updated central API docs JSON index while preserving HTML route details for ${unavailableServices.length} uninitialized gitlink service(s): ${unavailableServices.join(', ')}`,
      );
    } else {
      const normalizedPayload = {
        ...indexPayload,
        services: indexPayload.services.map(normalizeIndexedServiceArtifacts),
      };
      await writeOrCheck(
        centralIndexJson,
        `${JSON.stringify(normalizedPayload, null, 2)}\n`,
      );
      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(normalizedPayload.services));
    }
  }
  console.log(`${checkOnly ? 'checked' : 'generated'} API docs for ${services.length} service(s)`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
