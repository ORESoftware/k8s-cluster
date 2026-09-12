// Shared e2e harness: boots a real 3-relay onion overlay + SOCKS/UI client
// (the compiled Rust binary) plus a local origin HTTP server, on dynamic ports.
// Used by both the Playwright and Puppeteer suites.

import { spawn, spawnSync } from 'node:child_process';
import net from 'node:net';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const REPO_DIR = path.resolve(__dirname, '..', '..');
const BIN = path.join(REPO_DIR, 'target', 'release', 'tor-server');

export function getFreePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

export function waitPort(port, host = '127.0.0.1', timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const tryOnce = () => {
      const s = net.connect(port, host);
      s.once('connect', () => { s.destroy(); resolve(); });
      s.once('error', () => {
        s.destroy();
        if (Date.now() > deadline) reject(new Error(`port ${port} not up in ${timeoutMs}ms`));
        else setTimeout(tryOnce, 100);
      });
    };
    tryOnce();
  });
}

export async function httpJson(url) {
  const res = await fetch(url);
  return res.json();
}

function keygen(keyFile) {
  const out = spawnSync(BIN, ['keygen'], {
    env: { ...process.env, TOR_KEY_FILE: keyFile },
    encoding: 'utf8',
  });
  const m = /pubkey:\s*(\S+)/.exec(out.stdout || '');
  if (!m) throw new Error(`keygen failed for ${keyFile}: ${out.stdout} ${out.stderr}`);
  return m[1];
}

export function ensureBinary() {
  if (fs.existsSync(BIN)) return;
  const r = spawnSync('cargo', ['build', '--release'], { cwd: REPO_DIR, stdio: 'inherit' });
  if (r.status !== 0) throw new Error('cargo build --release failed');
}

// Start the full overlay. Returns handles + a hit counter for the origin so
// tests can prove requests actually reached it (cache/DNS were busted).
export async function startOverlay({ hops = 3, allowPrivate = true } = {}) {
  ensureBinary();
  const workdir = fs.mkdtempSync(path.join(os.tmpdir(), 'tor-e2e-'));
  const procs = [];

  const relays = [];
  for (const name of ['a', 'b', 'c']) {
    const keyFile = path.join(workdir, `relay-${name}.key`);
    const pub = keygen(keyFile);
    const port = await getFreePort();
    relays.push({ name: `relay-${name}`, port, keyFile, pub });
  }

  const dirToml = relays
    .map((r) => `[[relays]]\nname = "${r.name}"\naddr = "127.0.0.1:${r.port}"\npubkey = "${r.pub}"\n`)
    .join('\n');
  const dirFile = path.join(workdir, 'directory.toml');
  fs.writeFileSync(dirFile, dirToml);

  const logs = [];
  const spawnNode = (args, env, tag) => {
    const p = spawn(BIN, args, { env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
    p.stdout.on('data', (d) => logs.push(`[${tag}] ${d}`));
    p.stderr.on('data', (d) => logs.push(`[${tag}] ${d}`));
    procs.push(p);
    return p;
  };

  for (const r of relays) {
    spawnNode(['relay'], {
      TOR_LISTEN: `127.0.0.1:${r.port}`,
      TOR_KEY_FILE: r.keyFile,
      TOR_EXIT_ALLOW_PRIVATE: allowPrivate ? '1' : '',
      RUST_LOG: process.env.TOR_E2E_RUST_LOG || 'warn',
    }, r.name);
  }

  const socksPort = await getFreePort();
  const uiPort = await getFreePort();
  spawnNode(['client'], {
    TOR_SOCKS_LISTEN: `127.0.0.1:${socksPort}`,
    TOR_UI_LISTEN: `127.0.0.1:${uiPort}`,
    TOR_DIRECTORY: dirFile,
    TOR_HOPS: String(hops),
    TOR_DOCS_DIR: path.join(REPO_DIR, 'docs'),
    RUST_LOG: process.env.TOR_E2E_RUST_LOG || 'warn',
  }, 'client');

  // Local origin server: counts hits per path and forbids caching so we can
  // assert each navigation produced a fresh request that reached the origin.
  const hits = new Map();
  const origin = http.createServer((req, res) => {
    const u = new URL(req.url, 'http://origin');
    hits.set(u.pathname, (hits.get(u.pathname) || 0) + 1);
    const n = hits.get(u.pathname);
    res.setHeader('Cache-Control', 'no-store, no-cache, must-revalidate');
    res.setHeader('X-Origin-Hit', String(n));
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    res.end(
      `<!doctype html><html><head><title>onion-origin</title></head>` +
      `<body><h1 id="marker">onion-origin</h1>` +
      `<p id="hit">${n}</p><p id="path">${u.pathname}</p><p id="query">${u.search}</p>` +
      `<p id="ua">${(req.headers['user-agent'] || '').slice(0, 60)}</p></body></html>`
    );
  });
  const originPort = await getFreePort();
  await new Promise((r) => origin.listen(originPort, '127.0.0.1', r));

  for (const r of relays) await waitPort(r.port);
  await waitPort(socksPort);
  await waitPort(uiPort);

  return {
    socksPort,
    uiPort,
    originPort,
    relays,
    hitsOf: (p = '/') => hits.get(p) || 0,
    status: () => httpJson(`http://127.0.0.1:${uiPort}/api/status`),
    dumpLogs: () => logs.join(''),
    async stop() {
      for (const p of procs) { try { p.kill('SIGKILL'); } catch {} }
      await new Promise((r) => origin.close(r));
      try { fs.rmSync(workdir, { recursive: true, force: true }); } catch {}
    },
  };
}

// Chromium flags that (a) route ALL traffic — including loopback — through our
// SOCKS proxy (`<-loopback>` disables Chrome's implicit localhost bypass), and
// (b) disable on-disk caches so navigations are not served from cache.
export function proxyArgs(socksPort) {
  return [
    `--proxy-server=socks5://127.0.0.1:${socksPort}`,
    '--proxy-bypass-list=<-loopback>',
    '--no-sandbox',
    '--disk-cache-size=1',
    '--media-cache-size=1',
    '--dns-prefetch-disable',
    '--disable-features=NetworkServiceInProcess',
  ];
}

// A cache-busting URL: unique query per call so no HTTP cache can satisfy it.
export function bustUrl(base, i = 0) {
  const nonce = `${Date.now()}-${Math.random().toString(36).slice(2)}-${i}`;
  const sep = base.includes('?') ? '&' : '?';
  return `${base}${sep}cb=${nonce}`;
}
