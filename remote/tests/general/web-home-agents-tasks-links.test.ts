import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';
import { chromium } from 'playwright';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/web-home-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readWebHomeSource(): Promise<string> {
  return readFile(resolve(repoRoot, 'remote/deployments/web-home-rs/src/main.rs'), 'utf8');
}

function rawStringConst(source: string, name: string): string {
  const startMatch = new RegExp(`const ${name}: &str = r(#+)"`).exec(source);
  assert.ok(startMatch, `expected ${name} raw string constant`);
  const start = startMatch.index + startMatch[0].length;
  const endToken = `"${startMatch[1]};`;
  const end = source.indexOf(endToken, start);
  assert.notEqual(end, -1, `expected ${name} raw string terminator`);
  return source.slice(start, end);
}

test('agents tasks stream links safe URIs and requires modifier-click to open them', async () => {
  const server = await readWebHomeSource();
  const css = rawStringConst(server, 'AGENTS_TASKS_CSS');
  const tasksJs = rawStringConst(server, 'AGENTS_TASKS_JS');
  const helperPrefix = tasksJs.slice(0, tasksJs.indexOf('      const setThreadRuntimeState'));

  assert.match(css, /\.stream-link \{/);
  assert.match(tasksJs, /const LINKABLE_URI_PATTERN = /);
  assert.match(tasksJs, /const BLOCKED_URI_PROTOCOLS = new Set\(\["javascript:", "data:", "vbscript:", "blob:"\]\)/);
  assert.match(tasksJs, /anchor\.title = "Ctrl\+click or Cmd\+click to open"/);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(`
      <pre id="chat-stream">No active stream.</pre>
      <script>
        ${helperPrefix}
        window.__appendStreamLine = appendStreamLine;
      </script>
    `);

    const result = await page.evaluate(() => {
      window.__openedLink = null;
      window.open = (href, target, features) => {
        window.__openedLink = { href, target, features };
        return null;
      };
      window.__appendStreamLine('Draft PR created: https://github.com/ORESoftware/live-mutex/pull/114');
      window.__appendStreamLine('Docs: www.example.com/path). Email: mailto:ops@example.com javascript:alert(1)');
      window.__appendStreamLine('Event prefix should stay plain: sse:status: websocket connected');

      const anchors = [...document.querySelectorAll<HTMLAnchorElement>('#chat-stream a')];
      const normalClickPrevented = !anchors[0].dispatchEvent(new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
      }));
      const ctrlClickPrevented = !anchors[0].dispatchEvent(new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        ctrlKey: true,
      }));
      anchors[0].dispatchEvent(new MouseEvent('mousedown', {
        bubbles: true,
        cancelable: true,
        button: 0,
        ctrlKey: true,
      }));

      return {
        text: document.querySelector('#chat-stream')?.textContent,
        anchors: anchors.map((anchor) => ({
          href: anchor.getAttribute('href'),
          text: anchor.textContent,
          target: anchor.target,
          rel: anchor.rel,
          title: anchor.title,
        })),
        normalClickPrevented,
        ctrlClickPrevented,
        openedLink: window.__openedLink,
      };
    });

    assert.equal(result.text?.includes('javascript:alert(1)'), true);
    assert.equal(result.text?.includes('sse:status: websocket connected'), true);
    assert.deepEqual(result.anchors.map((anchor) => anchor.text), [
      'https://github.com/ORESoftware/live-mutex/pull/114',
      'www.example.com/path',
      'mailto:ops@example.com',
    ]);
    assert.deepEqual(result.anchors.map((anchor) => anchor.href), [
      'https://github.com/ORESoftware/live-mutex/pull/114',
      'https://www.example.com/path',
      'mailto:ops@example.com',
    ]);
    assert.equal(result.anchors.every((anchor) => anchor.target === '_blank'), true);
    assert.equal(result.anchors.every((anchor) => anchor.rel === 'noopener noreferrer'), true);
    assert.equal(result.anchors.every((anchor) => anchor.title === 'Ctrl+click or Cmd+click to open'), true);
    assert.equal(result.normalClickPrevented, true);
    assert.equal(result.ctrlClickPrevented, false);
    assert.deepEqual(result.openedLink, {
      href: 'https://github.com/ORESoftware/live-mutex/pull/114',
      target: '_blank',
      features: 'noopener,noreferrer',
    });
  } finally {
    await browser.close();
  }
});
