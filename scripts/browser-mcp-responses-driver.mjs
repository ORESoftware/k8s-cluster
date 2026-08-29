#!/usr/bin/env node
// =============================================================================
// browser-mcp-responses-driver.mjs
//
// Drive dd-browser-mcp-rs from OpenAI's Responses API instead of a ChatGPT
// custom connector.
//
// WHY THIS EXISTS
// ---------------
// The ChatGPT app cannot reach this server usefully:
//   * ChatGPT custom connectors authenticate with OAuth 2.1 or not at all — they
//     cannot present a static bearer. Running the server anonymously to work
//     around that puts a write-capable browser driver on the public internet.
//   * ChatGPT write/modify MCP actions are gated to Business/Enterprise/Edu. On a
//     Pro account `browser_act` will not be invocable even once the connector
//     attaches, so the anonymous exposure buys nothing.
//
// The Responses API has neither limitation: its `mcp` tool accepts arbitrary
// headers, so the bearer works, and there is no plan gate on write tools. That
// means the endpoint stays bearer-gated (not public) AND you get the full tool
// set today. This is the supported path until a Business workspace + OAuth is
// actually worth building.
//
// USAGE
//   export OPENAI_API_KEY=sk-...
//   export BROWSER_MCP_BEARER="$(kubectl --context dd-ec2-runtime -n default \
//       get secret dd-browser-mcp-rs-secrets \
//       -o jsonpath='{.data.BROWSER_MCP_AUTH_SECRET}' | base64 -d)"
//
//   # 1. Prove the endpoint works, no OpenAI involved (no API key needed):
//   node scripts/browser-mcp-responses-driver.mjs --verify
//
//   # 2. Drive it with a model:
//   node scripts/browser-mcp-responses-driver.mjs \
//     "Open https://www.irs.gov, report the page title and URL. Do not type into
//      or submit any form."
//
// FLAGS
//   --verify         direct MCP initialize + tools/list against the endpoint; exits.
//   --model <id>     model to use (default: $OPENAI_MODEL or gpt-5).
//   --url <url>      MCP endpoint (default: $BROWSER_MCP_URL or the EC2 gateway).
//   --auto-approve   do not require approval for tool calls. OFF by default: the
//                    server's tools are write-capable and can submit forms.
// =============================================================================

const DEFAULT_URL = process.env.BROWSER_MCP_URL || 'https://98.90.186.114/browser-mcp';
const argv = process.argv.slice(2);

function flag(name, fallback = null) {
  const i = argv.indexOf(name);
  if (i === -1) return fallback;
  const v = argv[i + 1];
  if (v === undefined || v.startsWith('--')) return fallback;
  argv.splice(i, 2);
  return v;
}
function has(name) {
  const i = argv.indexOf(name);
  if (i === -1) return false;
  argv.splice(i, 1);
  return true;
}

const verify = has('--verify');
const autoApprove = has('--auto-approve');
const model = flag('--model', process.env.OPENAI_MODEL || 'gpt-5');
const url = flag('--url', DEFAULT_URL);
const bearer = process.env.BROWSER_MCP_BEARER || '';
const prompt = argv.join(' ').trim();

function die(msg, code = 1) {
  console.error(`error: ${msg}`);
  process.exit(code);
}

// The endpoint is bearer-gated on purpose. Without it every call is a 401, which
// surfaces as an opaque MCP failure inside the Responses API — so fail early here
// with an actionable message instead.
if (!bearer) {
  die(
    'BROWSER_MCP_BEARER is not set.\n' +
      "  export BROWSER_MCP_BEARER=\"$(kubectl --context dd-ec2-runtime -n default \\\n" +
      "      get secret dd-browser-mcp-rs-secrets \\\n" +
      "      -o jsonpath='{.data.BROWSER_MCP_AUTH_SECRET}' | base64 -d)\"",
  );
}

const mcpHeaders = {
  'Content-Type': 'application/json',
  // MCP Streamable HTTP: clients advertise both, servers may answer with either.
  Accept: 'application/json, text/event-stream',
  Authorization: `Bearer ${bearer}`,
};

async function rpc(method, params, id = 1) {
  const res = await fetch(url, {
    method: 'POST',
    headers: mcpHeaders,
    body: JSON.stringify({ jsonrpc: '2.0', id, method, ...(params ? { params } : {}) }),
  });
  const text = await res.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    body = text;
  }
  return { status: res.status, body };
}

// --- --verify: talk to the MCP server directly, no OpenAI in the loop ---------
// Worth doing first whenever something looks broken: it separates "the endpoint
// is wrong" from "the model/API is wrong", which are otherwise easy to conflate.
if (verify) {
  console.log(`endpoint: ${url}`);

  const init = await rpc('initialize', {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'browser-mcp-responses-driver', version: '1.0' },
  });
  console.log(`  initialize  -> http ${init.status}`);
  if (init.status === 401) {
    die('401 unauthorized — the bearer is wrong, or the pod is running with a different secret.');
  }
  if (init.status !== 200) die(`unexpected status ${init.status}: ${JSON.stringify(init.body).slice(0, 300)}`);

  const list = await rpc('tools/list', null, 2);
  const tools = list.body?.result?.tools ?? [];
  console.log(`  tools/list  -> http ${list.status}  tools: ${tools.map((t) => t.name).join(', ') || '(none)'}`);

  // Surface which tools can mutate state — the reason approval defaults to on.
  for (const t of tools) {
    const a = t.annotations ?? {};
    const kind = a.readOnlyHint ? 'read-only' : 'WRITE-CAPABLE';
    console.log(`      - ${t.name.padEnd(18)} ${kind}`);
  }
  process.exit(0);
}

if (!prompt) die('no prompt given. Pass one as arguments, or use --verify.');
if (!process.env.OPENAI_API_KEY) die('OPENAI_API_KEY is not set.');

// --- drive the model with the MCP server attached ----------------------------
const payload = {
  model,
  input: prompt,
  tools: [
    {
      type: 'mcp',
      server_label: 'browser-mcp',
      server_url: url,
      // The whole point: the Responses API forwards custom headers, so the
      // endpoint can stay bearer-gated rather than public.
      headers: { Authorization: `Bearer ${bearer}` },
      require_approval: autoApprove ? 'never' : 'always',
    },
  ],
};

const res = await fetch('https://api.openai.com/v1/responses', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${process.env.OPENAI_API_KEY}`,
  },
  body: JSON.stringify(payload),
});

const data = await res.json();
if (!res.ok) {
  console.error(`OpenAI API error ${res.status}:`);
  console.error(JSON.stringify(data, null, 2).slice(0, 2000));
  process.exit(1);
}

for (const item of data.output ?? []) {
  if (item.type === 'mcp_list_tools') {
    console.log(`[tools discovered] ${(item.tools ?? []).map((t) => t.name).join(', ')}`);
  } else if (item.type === 'mcp_approval_request') {
    console.log(`[APPROVAL REQUIRED] ${item.name}`);
    console.log(`  arguments: ${JSON.stringify(item.arguments).slice(0, 600)}`);
    console.log('  Re-run with --auto-approve to allow, or approve programmatically.');
  } else if (item.type === 'mcp_call') {
    console.log(`[tool call] ${item.name}`);
    if (item.error) console.log(`  error: ${JSON.stringify(item.error).slice(0, 400)}`);
    else console.log(`  output: ${String(item.output ?? '').slice(0, 800)}`);
  } else if (item.type === 'message') {
    for (const c of item.content ?? []) if (c.type === 'output_text') console.log(`\n${c.text}`);
  }
}
