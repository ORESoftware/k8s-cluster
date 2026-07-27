import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/web-home-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const sourceRoot = 'remote/deployments/web-home-rs/src';

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('web-home-rs modular source preserves shared shells, handlers, and route wiring', async () => {
  const [main, shared, home, handlers, agents, lambda] = await Promise.all([
    readRepoFile(`${sourceRoot}/main.rs`),
    readRepoFile(`${sourceRoot}/shared.rs`),
    readRepoFile(`${sourceRoot}/home.rs`),
    readRepoFile(`${sourceRoot}/handlers.rs`),
    readRepoFile(`${sourceRoot}/agents.rs`),
    readRepoFile(`${sourceRoot}/lambda.rs`),
  ]);

  for (const moduleName of ['agents', 'handlers', 'home', 'lambda', 'shared']) {
    assert.match(main, new RegExp(`mod ${moduleName};`), `main.rs must register ${moduleName}.rs`);
  }

  assert.match(shared, /use maud::\{html, Markup, PreEscaped, DOCTYPE\}/);
  assert.match(shared, /fn shared_header\(active_page: &'static str\) -> Markup/);
  assert.match(shared, /fn ui_document\(/);
  assert.match(shared, /fn inline_ui_document\(/);
  assert.match(home, /fn home_document\(state: &AppState\) -> Html<String>/);

  assert.match(home, /home_document[\s\S]*\(shared_header\("home"\)\)/);
  assert.match(shared, /fn ui_document\([\s\S]*\(shared_header\(active_page\)\)/);
  assert.match(shared, /fn inline_ui_document\([\s\S]*\(shared_header\(active_page\)\)/);

  assert.match(handlers, /async fn agents_tasks_page\(\)[\s\S]*ui_document\([\s\S]*"tasks"/);
  assert.match(handlers, /async fn agents_threads_page\(\)[\s\S]*ui_document\([\s\S]*"threads"/);
  assert.match(handlers, /async fn lambda_functions_page\(\)[\s\S]*inline_ui_document\([\s\S]*"lambdas"/);
  assert.match(handlers, /async fn presence_test_page\(\)[\s\S]*inline_ui_document\([\s\S]*"presence"/);
  assert.match(handlers, /async fn wss_test_page\(\)[\s\S]*inline_ui_document\([\s\S]*"wss"/);

  const normalizedMain = main.replace(/\s+/g, ' ');
  for (const [path, handler] of [
    ['/agents/tasks', 'agents_tasks_page'],
    ['/agents/threads', 'agents_threads_page'],
    ['/lambdas/functions', 'lambda_functions_page'],
    ['/presence-test', 'presence_test_page'],
    ['/wss-test', 'wss_test_page'],
  ] as const) {
    assert.ok(
      normalizedMain.includes(`.route("${path}", get(${handler}))`),
      `main.rs must route ${path} to ${handler}`,
    );
  }

  assert.match(agents, /pub\(crate\) const AGENTS_TASKS_CSS: &str/);
  assert.match(agents, /pub\(crate\) const AGENTS_TASKS_JS: &str/);
  assert.match(agents, /pub\(crate\) const AGENTS_THREADS_CSS: &str/);
  assert.match(agents, /pub\(crate\) const AGENTS_THREADS_JS: &str/);
  assert.match(lambda, /pub\(crate\) const LAMBDA_FUNCTIONS_CSS: &str/);
  assert.match(lambda, /pub\(crate\) const LAMBDA_FUNCTIONS_JS: &str/);

  const combined = [main, shared, home, handlers, agents, lambda].join('\n');
  assert.doesNotMatch(combined, /static_page_with_shared_header/);
  assert.doesNotMatch(combined, /const (?:LAMBDA_FUNCTIONS|PRESENCE_TEST|WSS_TEST)_HTML: &str/);
  assert.doesNotMatch(shared + home, /<!doctype html>|<html lang="en">|<body>/);
});
