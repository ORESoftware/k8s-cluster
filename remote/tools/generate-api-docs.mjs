#!/usr/bin/env node
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
  '/readyz',
  '/metrics',
  '/docs/api',
  '/api/docs',
  '/api/docs.json',
  '/favicon.ico',
]);

const RUST_DEPLOYMENT_ALLOWLIST = new Set([
  'agent-worker-broker-rs',
  'auth-server-rs',
  'bastion-rs',
  'build-server-rs',
  'container-pool-rs',
  'contract-service-rs',
  'des-simulator-rs',
  'formal-methods-service-rs',
  'formal-methods-server-rs',
  'mdp-optimizer-rs',
  'rest-api-rs',
  'trading-server-rs',
  'wal-gateway-rs',
  'webrtc-signaling-rs',
]);

const RUST_ROUTE_SOURCE_OVERRIDES = new Map([
  ['formal-methods-service-rs', 'src/routes/mod.rs'],
]);

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

async function pathExists(path) {
  return existsSync(path);
}

async function readUtf8(path) {
  return readFile(path, 'utf8');
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

function extractGleamRoutes(source, sourceFile) {
  const routes = [];
  for (const match of source.matchAll(/\/\/\/\/\s*(GET|POST|PATCH|DELETE|PUT|OPTIONS)\s+([^\s]+)(?:\s+(.*))?/g)) {
    routes.push({
      path: match[2].replace(/<([a-zA-Z0-9_]+)>/g, ':$1'),
      methods: [match[1]],
      handlers: [],
      sourceFile,
      purposeHint: match[3]?.trim() ?? '',
    });
  }
  for (const match of source.matchAll(/\b(Get|Post|Patch|Delete|Put|Options),\s*\[([^\]]*)\]/g)) {
    routes.push({
      path: gleamSegmentsToPath(match[2]),
      methods: [match[1].toUpperCase()],
      handlers: [],
      sourceFile,
    });
  }
  for (const match of source.matchAll(/^\s*\[([^\]]*)\]\s*->/gm)) {
    const path = gleamSegmentsToPath(match[1]);
    if (path !== '/') {
      routes.push({
        path,
        methods: ['GET', 'POST'],
        handlers: [],
        sourceFile,
        notes: 'Method is inferred from route body; inspect source for exact method guard.',
      });
    }
  }
  return mergeRoutes(routes);
}

function extractNodeRoutes(source, sourceFile) {
  const routes = [];
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

function classifyRoute(serviceName, route) {
  if (serviceName === 'rest-api-rs' && route.path.startsWith('/internal/db')) {
    return 'internal-db';
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
  if (route.path === '/healthz' || route.path.endsWith('/healthz')) {
    return 'Health check.';
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
  return 'Custom code-first route derived from the service router.';
}

function routeAuth(routeType, route) {
  if (routeType === 'internal-db') {
    return 'operator secret';
  }
  if (route.path.includes('/webhooks/')) {
    return 'webhook signature';
  }
  if (route.path === '/healthz' || route.path === '/readyz' || route.path === '/metrics' || route.path === '/') {
    return 'public';
  }
  if (route.path === '/docs/api' || route.path === '/api/docs' || route.path === '/api/docs.json') {
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

async function discoverRustServices() {
  const deploymentsDir = resolve(repoRoot, 'remote/deployments');
  const entries = await readdir(deploymentsDir, { withFileTypes: true });
  const services = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || !RUST_DEPLOYMENT_ALLOWLIST.has(entry.name)) {
      continue;
    }
    const main = join(
      deploymentsDir,
      entry.name,
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
      const dbRoutes = join(deploymentsDir, entry.name, 'src/db_routes.rs');
      if ((await pathExists(dbRoutes)) && source.includes('/internal/db')) {
        // Internal DB tooling is intentionally not part of the public REST
        // docs unless the main router exposes its private mount point.
        rawRoutes.push(...extractAxumRoutesFromSource(await readUtf8(dbRoutes), dbRoutes, '/internal/db'));
      }
    }
    services.push({
      service: entry.name,
      language: 'rust',
      deploymentDir: join(deploymentsDir, entry.name),
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
  ];
  const services = [];
  for (const spec of specs) {
    const file = resolve(repoRoot, spec.file);
    if (!(await pathExists(file))) {
      continue;
    }
    const rawRoutes = spec.parser(await readUtf8(file), file);
    services.push({
      service: spec.service,
      language: spec.language,
      deploymentDir: resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(file))),
      moduleDir: dirname(file),
      outputName: spec.outputName ?? 'api-docs',
      routes: normalizeRoutes(spec.service, rawRoutes),
    });
  }
  return services;
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
    standardDocsRoutes: ['/docs/api', '/api/docs', '/api/docs.json'],
    routes,
  };
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
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(docs.service)} API docs</title>
  <style>
    :root { color-scheme: light; --bg:#f7f8fa; --panel:#fff; --ink:#17202a; --muted:#5b6672; --line:#d8dee6; --code:#eef2f6; --service:#52687a; --custom:#1f6f5b; --internal:#8a5a12; }
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
      <span>${docs.routeTypeCounts['user-generated'] ?? 0} user-generated</span>
      ${docs.routeTypeCounts['internal-db'] ? `<span>${docs.routeTypeCounts['internal-db']} internal-db</span>` : ''}
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

function gleamString(value) {
  return JSON.stringify(value);
}

function gleamApiDocsModule(docs) {
  return `// Generated by remote/tools/generate-api-docs.mjs. Do not edit by hand.
import gleam/bytes_tree
import gleam/http/response
import mist

const api_docs_html = ${gleamString(renderDocsHtml(docs))}

const api_docs_json = ${gleamString(`${JSON.stringify(docs, null, 2)}\n`)}

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

async function main() {
  const services = [...await discoverRustServices(), ...await discoverExtraServices()]
    .filter((service) => service.routes.length > 0)
    .sort((left, right) => left.service.localeCompare(right.service));
  const index = [];
  for (const service of services) {
    const docs = buildDocs(service);
    const outputBase = service.outputName ?? 'api-docs';
    const generatedDir = join(service.deploymentDir, 'generated');
    const json = `${JSON.stringify(docs, null, 2)}\n`;
    const html = renderDocsHtml(docs);
    await writeOrCheck(join(generatedDir, `${outputBase}.json`), json);
    await writeOrCheck(join(generatedDir, `${outputBase}.html`), html);
    if (service.language === 'gleam' && outputBase === 'api-docs' && service.moduleDir) {
      await writeOrCheck(join(service.moduleDir, 'api_docs.gleam'), gleamApiDocsModule(docs));
    }
    index.push({
      service: service.service,
      language: service.language,
      routeCount: docs.routeCount,
      routeTypeCounts: docs.routeTypeCounts,
      generated: [
        relative(repoRoot, join(generatedDir, `${outputBase}.json`)).split(sep).join('/'),
        relative(repoRoot, join(generatedDir, `${outputBase}.html`)).split(sep).join('/'),
      ],
    });
  }
  await writeOrCheck(
    resolve(repoRoot, 'remote/deployments/generated-api-docs-index.json'),
    `${JSON.stringify({ ok: true, generatedBy: 'remote/tools/generate-api-docs.mjs', services: index }, null, 2)}\n`,
  );
  console.log(`${checkOnly ? 'checked' : 'generated'} API docs for ${services.length} service(s)`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
