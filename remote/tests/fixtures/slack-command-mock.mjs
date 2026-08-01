import http from 'node:http';

const port = Number.parseInt(process.env.SLACK_MOCK_PORT ?? '8170', 10);
const state = {
  historyCalls: 0,
  views: [],
  messages: [],
};

const historyMessages = [
  { user: 'U6', ts: '1006.000001', text: 'message-6' },
  { user: 'U5', ts: '1005.000001', text: 'message-5' },
  { bot_id: 'B1', ts: '1004.500001', text: 'ignore-bot' },
  { user: 'U4', ts: '1004.000001', text: 'message-4' },
  { user: 'U3', ts: '1003.000001', text: 'message-3' },
  { user: 'U2', ts: '1002.000001', text: 'message-2' },
  { user: 'U1', ts: '1001.000001', text: 'message-1' },
];

function json(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(body),
    'cache-control': 'no-store',
  });
  response.end(body);
}

async function readJson(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 1024 * 1024) {
      throw new Error('request body exceeds test limit');
    }
    chunks.push(chunk);
  }
  const text = Buffer.concat(chunks).toString('utf8');
  return text ? JSON.parse(text) : {};
}

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function dashboard() {
  const latestView = state.views.at(-1)?.view ?? {};
  const blocks = Array.isArray(latestView.blocks) ? latestView.blocks : [];
  const blockIds = blocks.map((block) => block.block_id).filter(Boolean);
  const contextBlock = blocks.find((block) => block.block_id === 'context_messages');
  const writeBlock = blocks.find((block) => block.block_id === 'write_scope');
  const messages = state.messages
    .map((message, index) => `<li data-testid="status-${index}"><pre>${escapeHtml(message.text ?? '')}</pre></li>`)
    .join('');

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>ORESoftware Slack command integration dashboard</title>
</head>
<body>
  <h1>ORESoftware Slack command integration dashboard</h1>
  <dl>
    <dt>Modal count</dt><dd data-testid="modal-count">${state.views.length}</dd>
    <dt>Status count</dt><dd data-testid="message-count">${state.messages.length}</dd>
    <dt>History calls</dt><dd data-testid="history-count">${state.historyCalls}</dd>
    <dt>Modal callback</dt><dd data-testid="modal-callback">${escapeHtml(latestView.callback_id ?? '')}</dd>
    <dt>Modal blocks</dt><dd data-testid="modal-blocks">${escapeHtml(blockIds.join(','))}</dd>
    <dt>Context default</dt><dd data-testid="context-default">${escapeHtml(contextBlock?.element?.initial_option?.value ?? '')}</dd>
    <dt>Write default</dt><dd data-testid="write-default">${escapeHtml(writeBlock?.element?.initial_option?.value ?? '')}</dd>
  </dl>
  <ol data-testid="statuses">${messages}</ol>
</body>
</html>`;
}

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? '/', `http://${request.headers.host ?? `127.0.0.1:${port}`}`);

    if (request.method === 'GET' && url.pathname === '/healthz') {
      return json(response, 200, { ok: true });
    }
    if (request.method === 'GET' && url.pathname === '/api/conversations.history') {
      state.historyCalls += 1;
      return json(response, 200, { ok: true, messages: historyMessages });
    }
    if (request.method === 'GET' && url.pathname === '/api/usergroups.list') {
      return json(response, 200, { ok: true, usergroups: [] });
    }
    if (request.method === 'POST' && url.pathname === '/api/views.open') {
      state.views.push(await readJson(request));
      return json(response, 200, { ok: true });
    }
    if (request.method === 'POST' && url.pathname === '/api/chat.postMessage') {
      state.messages.push(await readJson(request));
      return json(response, 200, {
        ok: true,
        ts: `${2000 + state.messages.length}.000001`,
      });
    }
    if (request.method === 'GET' && url.pathname === '/admin/state') {
      return json(response, 200, state);
    }
    if (request.method === 'POST' && url.pathname === '/admin/reset') {
      state.historyCalls = 0;
      state.views.length = 0;
      state.messages.length = 0;
      return json(response, 200, { ok: true });
    }
    if (request.method === 'GET' && url.pathname === '/') {
      const body = dashboard();
      response.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        'content-length': Buffer.byteLength(body),
        'cache-control': 'no-store',
      });
      return response.end(body);
    }

    return json(response, 404, { ok: false, error: 'not_found' });
  } catch (error) {
    return json(response, 500, {
      ok: false,
      error: error instanceof Error ? error.message : 'mock_failure',
    });
  }
});

server.listen(port, '127.0.0.1', () => {
  console.log(`Slack API mock listening on http://127.0.0.1:${port}`);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
