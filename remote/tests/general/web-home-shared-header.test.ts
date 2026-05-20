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

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('web-home-rs document pages render through Maud shells with the shared header', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.match(server, /use maud::\{html, Markup, PreEscaped, DOCTYPE\}/);
  assert.match(server, /fn shared_header\(active_page: &'static str\) -> Markup/);
  assert.match(server, /fn ui_document\(/);
  assert.match(server, /fn inline_ui_document\(/);
  assert.match(server, /fn home_document\(state: &AppState\) -> Html<String>/);

  assert.match(server, /home_document[\s\S]*\(shared_header\("home"\)\)/);
  assert.match(server, /fn ui_document\([\s\S]*\(shared_header\(active_page\)\)/);
  assert.match(server, /fn inline_ui_document\([\s\S]*\(shared_header\(active_page\)\)/);

  assert.match(server, /async fn agents_tasks_page\(\)[\s\S]*ui_document\([\s\S]*"tasks"/);
  assert.match(server, /async fn agents_threads_page\(\)[\s\S]*ui_document\([\s\S]*"threads"/);
  assert.match(server, /async fn lambda_functions_page\(\)[\s\S]*inline_ui_document\([\s\S]*"lambdas"/);
  assert.match(server, /async fn presence_test_page\(\)[\s\S]*inline_ui_document\([\s\S]*"presence"/);
  assert.match(server, /async fn wss_test_page\(\)[\s\S]*inline_ui_document\([\s\S]*"wss"/);

  assert.doesNotMatch(server, /static_page_with_shared_header/);
  assert.doesNotMatch(server, /const (?:LAMBDA_FUNCTIONS|PRESENCE_TEST|WSS_TEST)_HTML: &str/);
  assert.doesNotMatch(server, /<!doctype html>|<html lang="en">|<body>/);
});
