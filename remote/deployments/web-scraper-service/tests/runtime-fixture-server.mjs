import { createServer } from 'node:http';

const host = process.env.SCRAPER_FIXTURE_HOST ?? '0.0.0.0';
const port = Number(process.env.SCRAPER_FIXTURE_PORT ?? '19090');

if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error(`invalid SCRAPER_FIXTURE_PORT: ${process.env.SCRAPER_FIXTURE_PORT ?? ''}`);
}

const INDEX_HTML = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>dd scraper runtime smoke</title>
  </head>
  <body>
    <main>
      <div id="static">static-ready</div>
      <a href="mailto:smoke@example.test">Email smoke</a>
      <a href="tel:+12125550147">Call smoke</a>
      <a href="/next.html">Next</a>
    </main>
    <script>
      setTimeout(() => {
        const node = document.createElement('div');
        node.id = 'dynamic';
        node.textContent = 'dynamic-ready';
        document.body.appendChild(node);
      }, 50);
    </script>
  </body>
</html>`;

const NEXT_HTML = '<!doctype html><html><body><div id="next">next-ready</div></body></html>';
const BLOCKED_HTML = '<!doctype html><html><body><div id="blocked">robots-should-block-this</div></body></html>';
const LARGE_HTML = `<!doctype html><html><body><div id="large">${'x'.repeat(96 * 1024)}</div></body></html>`;
const ROBOTS = 'User-agent: *\nAllow: /\nDisallow: /blocked.html\nCrawl-delay: 0\n';

function send(response, statusCode, contentType, body, extraHeaders = {}) {
  const bytes = Buffer.from(body);
  response.writeHead(statusCode, {
    'content-type': contentType,
    'content-length': String(bytes.byteLength),
    'cache-control': 'no-store',
    ...extraHeaders,
  });
  response.end(bytes);
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'fixture.invalid'}`);

  if (request.method !== 'GET') {
    send(response, 405, 'text/plain; charset=utf-8', 'method not allowed');
    return;
  }

  switch (url.pathname) {
    case '/healthz':
      send(response, 200, 'application/json; charset=utf-8', JSON.stringify({ ok: true }));
      return;
    case '/robots.txt':
      send(response, 200, 'text/plain; charset=utf-8', ROBOTS);
      return;
    case '/':
    case '/index.html':
      send(response, 200, 'text/html; charset=utf-8', INDEX_HTML);
      return;
    case '/next.html':
      send(response, 200, 'text/html; charset=utf-8', NEXT_HTML);
      return;
    case '/blocked.html':
      send(response, 200, 'text/html; charset=utf-8', BLOCKED_HTML);
      return;
    case '/large.html':
      send(response, 200, 'text/html; charset=utf-8', LARGE_HTML);
      return;
    case '/redirect':
      response.writeHead(302, {
        location: '/index.html',
        'content-length': '0',
        'cache-control': 'no-store',
      });
      response.end();
      return;
    case '/slow.html': {
      const requested = Number(url.searchParams.get('ms') ?? '1500');
      const delayMs = Number.isFinite(requested) ? Math.min(5_000, Math.max(0, requested)) : 1_500;
      await new Promise((resolve) => setTimeout(resolve, delayMs));
      send(
        response,
        200,
        'text/html; charset=utf-8',
        `<!doctype html><html><body><div id="slow">slow-ready-${delayMs}</div></body></html>`,
      );
      return;
    }
    case '/server-error':
      send(response, 500, 'text/plain; charset=utf-8', 'fixture upstream failure');
      return;
    case '/echo-headers':
      send(
        response,
        200,
        'application/json; charset=utf-8',
        JSON.stringify({
          userAgent: request.headers['user-agent'] ?? null,
          authorization: request.headers.authorization ?? null,
          cookie: request.headers.cookie ?? null,
          xServerAuth: request.headers['x-server-auth'] ?? null,
        }),
      );
      return;
    default:
      send(response, 404, 'text/plain; charset=utf-8', 'not found');
  }
});

server.listen(port, host, () => {
  process.stdout.write(`${JSON.stringify({ ok: true, host, port })}\n`);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    server.close(() => process.exit(0));
  });
}
