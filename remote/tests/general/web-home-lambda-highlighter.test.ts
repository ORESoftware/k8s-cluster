import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

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
  const match = source.match(new RegExp(`const ${name}: &str = r###"([\\s\\S]*?)"###;`));
  assert.ok(match, `expected ${name} raw string constant`);
  return match[1];
}

function highlightWith(lambdaJs: string, source: string, language: string): string {
  const prefix = lambdaJs.slice(0, lambdaJs.indexOf('\nfunction syncCodeScroll()'));
  const context = {
    URLSearchParams,
    language,
    location: { search: '' },
    result: '',
    source,
  };

  vm.runInNewContext(`${prefix}\nresult = highlightCode(source, language);`, context);
  return String(context.result);
}

test('lambda function code highlighter stretches over the editor cell', async () => {
  const server = await readWebHomeSource();
  const css = rawStringConst(server, 'LAMBDA_FUNCTIONS_CSS');
  const editorRule = css.match(/\.code-highlight,\n\.code-editor textarea \{([\s\S]*?)\n\}/)?.[1] ?? '';
  const highlightRule = css.match(/\.code-highlight \{([\s\S]*?)\n\}/)?.[1] ?? '';
  const tokenRule = css.match(/\.code-highlight span \{([\s\S]*?)\n\}/)?.[1] ?? '';

  assert.match(editorRule, /width:\s*100%;/);
  assert.match(editorRule, /min-width:\s*0;/);
  assert.match(editorRule, /max-width:\s*100%;/);
  assert.match(editorRule, /box-sizing:\s*border-box;/);
  assert.match(editorRule, /overflow-wrap:\s*normal;/);
  assert.match(editorRule, /word-break:\s*normal;/);
  assert.match(highlightRule, /overflow:\s*hidden;/);
  assert.match(tokenRule, /display:\s*inline;/);
  assert.match(tokenRule, /margin:\s*0;/);
  assert.match(css, /label > span \{/);
  assert.doesNotMatch(css, /label span \{/);
});

test('lambda function code highlighter uses language-specific comments', async () => {
  const server = await readWebHomeSource();
  const lambdaJs = rawStringConst(server, 'LAMBDA_FUNCTIONS_JS');

  const rust = highlightWith(lambdaJs, '#[derive(Debug)]\nfn main() {}\n// ok', 'rust');
  assert.doesNotMatch(rust, /<span class="tok-comment">#\[derive/);
  assert.match(rust, /<span class="tok-punct">#\[<\/span>/);
  assert.match(rust, /<span class="tok-comment">\/\/ ok<\/span>/);

  const python = highlightWith(lambdaJs, 'result = 4 // 2\n# comment', 'python3');
  assert.doesNotMatch(python, /<span class="tok-comment">\/\/ 2<\/span>/);
  assert.match(python, /<span class="tok-comment"># comment<\/span>/);
});

test('lambda function code highlighter covers every process profile language', async () => {
  const server = await readWebHomeSource();
  const lambdaJs = rawStringConst(server, 'LAMBDA_FUNCTIONS_JS');
  const samples: Array<[string, string, RegExp, RegExp]> = [
    ['nodejs', 'async function handler() {\n// ok\nreturn true;\n}', /tok-keyword">async/, /tok-comment">\/\/ ok/],
    ['python3', 'def handler(request, context):\n# ok\n    return True', /tok-keyword">def/, /tok-comment"># ok/],
    ['ruby', 'def handler(request, context)\n# ok\n  return true\nend', /tok-keyword">def/, /tok-comment"># ok/],
    ['bash', 'handler() {\n# ok\nreturn 0\n}', /tok-keyword">return/, /tok-comment"># ok/],
    ['golang', 'func Handler() {\n// ok\nreturn\n}', /tok-keyword">func/, /tok-comment">\/\/ ok/],
    ['dart', 'dynamic handler() {\n// ok\nreturn true;\n}', /tok-keyword">dynamic/, /tok-comment">\/\/ ok/],
    ['erlang', 'handle(_Request, _Context) ->\n% ok\ncase true of true -> ok end.', /tok-keyword">case/, /tok-comment">% ok/],
    ['elixir', 'defmodule Handler do\n# ok\n  def handle(), do: true\nend', /tok-keyword">defmodule/, /tok-comment"># ok/],
    ['java', 'public final class Handler {\n// ok\nreturn;\n}', /tok-keyword">public/, /tok-comment">\/\/ ok/],
    ['rust', 'fn main() {\n// ok\nreturn;\n}', /tok-keyword">fn/, /tok-comment">\/\/ ok/],
    ['gleamlang', 'pub fn handler() {\n// ok\ntodo\n}', /tok-keyword">pub/, /tok-comment">\/\/ ok/],
  ];

  for (const [language, source, keywordPattern, commentPattern] of samples) {
    const html = highlightWith(lambdaJs, source, language);
    assert.match(html, keywordPattern, `${language} keyword should be highlighted`);
    assert.match(html, commentPattern, `${language} comment should be highlighted`);
  }
});
