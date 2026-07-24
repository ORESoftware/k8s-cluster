pub(crate) const LAMBDA_FUNCTIONS_CSS: &str = r###":root {
  color-scheme: dark;
  --bg: #0d1117;
  --panel: #151b23;
  --panel-2: #101722;
  --field: #0f1620;
  --line: rgba(148, 163, 184, 0.28);
  --text: #eef2f6;
  --muted: #a8b3c1;
  --accent: #5eead4;
  --accent-2: #facc15;
  --danger: #fb7185;
  --ok: #86efac;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--text);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
button, input, select, textarea {
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--field);
  color: var(--text);
  font: inherit;
}
button {
  min-height: 34px;
  padding: 7px 11px;
  cursor: pointer;
}
button:hover { border-color: rgba(94, 234, 212, 0.62); }
button.primary {
  border-color: rgba(94, 234, 212, 0.65);
  background: rgba(20, 83, 45, 0.32);
  color: #dcfce7;
}
button.warn { border-color: rgba(250, 204, 21, 0.55); color: #fef9c3; }
input, select {
  min-height: 34px;
  width: 100%;
  padding: 7px 9px;
}
textarea {
  width: 100%;
  min-height: 120px;
  padding: 10px;
  resize: vertical;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.45;
}
.code-editor {
  display: grid;
  width: 100%;
  min-height: 220px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: #090f16;
  overflow: hidden;
}
.code-editor.field-invalid {
  border-color: rgba(251, 113, 133, 0.72) !important;
  box-shadow: 0 0 0 1px rgba(251, 113, 133, 0.16);
}
.code-highlight,
.code-editor textarea {
  grid-area: 1 / 1;
  justify-self: stretch;
  align-self: stretch;
  width: 100%;
  min-height: 220px;
  min-width: 0;
  max-width: 100%;
  box-sizing: border-box;
  margin: 0;
  padding: 10px;
  border: 0;
  border-radius: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.45;
  tab-size: 2;
  white-space: pre;
  overflow-wrap: normal;
  word-break: normal;
  overflow: auto;
}
.code-highlight {
  pointer-events: none;
  color: #d7fbf4;
  overflow: hidden;
}
.code-highlight span {
  display: inline;
  margin: 0;
}
.code-editor textarea {
  position: relative;
  z-index: 1;
  background: transparent;
  color: transparent;
  caret-color: var(--text);
  resize: vertical;
  -webkit-text-fill-color: transparent;
}
.code-editor textarea::selection {
  background: rgba(94, 234, 212, 0.24);
  -webkit-text-fill-color: transparent;
}
.tok-keyword { color: #93c5fd; }
.tok-string { color: #86efac; }
.tok-number { color: #facc15; }
.tok-comment { color: #7dd3fc; opacity: 0.66; }
.tok-punct { color: #c4b5fd; }
.app {
  min-height: 100vh;
  display: grid;
  grid-template-columns: minmax(280px, 360px) minmax(0, 1fr);
}
.sidebar {
  border-right: 1px solid var(--line);
  background: #111821;
  padding: 18px;
  min-width: 0;
}
.main {
  min-width: 0;
  padding: 22px;
}
.topbar, .row, .actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.topbar { justify-content: space-between; margin-bottom: 16px; }
h1 { margin: 0; font-size: 24px; }
h2 { margin: 0; font-size: 16px; }
h3 { margin: 0; font-size: 14px; }
p { margin: 0; color: var(--muted); line-height: 1.45; }
.muted { color: var(--muted); }
.panel {
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  padding: 14px;
}
.grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}
.wide { grid-column: 1 / -1; }
label > span {
  display: block;
  color: var(--muted);
  font-size: 12px;
  margin-bottom: 5px;
}
.check-row {
  min-height: 34px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 19px;
}
.check-row input { width: auto; min-height: auto; }
.check-row span { margin: 0; }
.pill {
  display: inline-flex;
  align-items: center;
  border: 1px solid rgba(94, 234, 212, 0.35);
  border-radius: 999px;
  padding: 3px 8px;
  color: var(--accent);
  font-size: 12px;
  white-space: nowrap;
}
.pill.warn { border-color: rgba(250, 204, 21, 0.4); color: var(--accent-2); }
.pill.bad { border-color: rgba(251, 113, 133, 0.42); color: var(--danger); }
.field-invalid {
  border-color: rgba(251, 113, 133, 0.72) !important;
  box-shadow: 0 0 0 1px rgba(251, 113, 133, 0.16);
}
.field-hint {
  margin-top: 5px;
  color: var(--danger);
  font-size: 12px;
}
.function-list {
  display: grid;
  gap: 8px;
  margin-top: 14px;
}
details {
  border: 1px solid rgba(148, 163, 184, 0.2);
  border-radius: 8px;
  background: var(--panel-2);
  overflow: hidden;
}
details[open] { border-color: rgba(94, 234, 212, 0.46); }
summary {
  min-height: 52px;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  padding: 12px;
  cursor: pointer;
}
summary::marker { color: var(--accent); }
.summary-title {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.summary-meta {
  display: flex;
  gap: 8px;
  color: var(--muted);
  font-size: 12px;
  flex-wrap: wrap;
  margin-top: 5px;
}
.details-body {
  border-top: 1px solid rgba(148, 163, 184, 0.18);
  padding: 12px;
  display: grid;
  gap: 10px;
}
.output {
  min-height: 170px;
  max-height: 420px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  border: 1px solid rgba(148, 163, 184, 0.2);
  border-radius: 8px;
  background: #090f16;
  padding: 12px;
  color: #d7fbf4;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.45;
}
@media (max-width: 980px) {
  .app { grid-template-columns: 1fr; }
  .sidebar { border-right: 0; border-bottom: 1px solid var(--line); }
  .grid { grid-template-columns: 1fr; }
}
"###;

pub(crate) const LAMBDA_FUNCTIONS_BODY: &str = r###"<div class="app">
  <aside class="sidebar">
    <div class="topbar">
      <div>
        <h1>Lambda functions</h1>
        <p id="snapshot-meta">loading functions</p>
      </div>
      <button id="refresh" type="button" title="Refresh">Refresh</button>
    </div>
    <input id="search" autocomplete="off" placeholder="Search functions" />
    <div class="actions" style="margin-top: 10px">
      <button id="new-function" class="primary" type="button">New</button>
    </div>
    <div id="function-list" class="function-list" aria-live="polite"></div>
  </aside>
  <main class="main">
    <div class="topbar">
      <div>
        <h1 id="editor-title">New function</h1>
        <p id="editor-subtitle">draft</p>
      </div>
      <div class="row">
        <a href="/agents/threads">Agent threads</a>
        <a href="/home">Service directory</a>
      </div>
    </div>

    <section class="panel">
      <div class="grid">
        <label>
          <span>Slug</span>
          <input id="slug" autocomplete="off" spellcheck="false" />
        </label>
        <label>
          <span>Name</span>
          <input id="display-name" autocomplete="off" />
        </label>
        <label>
          <span>Status</span>
          <select id="status">
            <option value="draft">draft</option>
            <option value="active">active</option>
            <option value="paused">paused</option>
            <option value="archived">archived</option>
          </select>
        </label>
        <label>
          <span>Runtime</span>
            <select id="runtime">
              <option value="nodejs">nodejs</option>
              <option value="python3">python3</option>
              <option value="ruby">ruby</option>
              <option value="bash">bash</option>
              <option value="golang">golang</option>
              <option value="dart">dart</option>
              <option value="erlang">erlang</option>
              <option value="elixir">elixir</option>
              <option value="java">java</option>
              <option value="browser">browser (Playwright/Puppeteer)</option>
            </select>
          </label>
          <label>
            <span>Process profile</span>
            <select id="process-profile">
              <option value="nodejs">nodejs process</option>
              <option value="python3">python3 process</option>
              <option value="ruby">ruby process</option>
              <option value="bash">bash process</option>
              <option value="golang">golang process</option>
              <option value="dart">dart process</option>
              <option value="erlang">erlang process</option>
              <option value="elixir">elixir process</option>
              <option value="java">java process</option>
              <option value="browser">browser automation</option>
              <option value="rust">rust process</option>
              <option value="gleamlang">gleamlang process</option>
            </select>
          </label>
        <label>
          <span>Container runner</span>
          <select id="container-runner">
            <option value="containerd-ctr">containerd / ctr</option>
            <option value="containerd-nerdctl">containerd / nerdctl</option>
            <option value="docker">docker</option>
          </select>
        </label>
        <label>
          <span>Base image</span>
          <select id="base-image"></select>
        </label>
        <label class="check-row">
          <input id="containerized" type="checkbox" />
          <span>Containerize</span>
        </label>
        <label>
          <span>Reuse key</span>
          <input id="reuse-key" autocomplete="off" spellcheck="false" />
        </label>
        <label>
          <span>Idle timeout seconds</span>
          <input id="idle-timeout" type="number" min="1" max="3600" />
        </label>
        <label>
          <span>Max run ms</span>
          <input id="max-run" type="number" min="1000" max="300000" step="500" />
        </label>
        <label>
          <span>Entry command</span>
          <input id="entry-command" autocomplete="off" readonly spellcheck="false" />
        </label>
        <label>
          <span>Container image</span>
          <input id="container-image" autocomplete="off" readonly spellcheck="false" />
        </label>
        <label>
          <span>Build status</span>
          <input id="container-build-status" autocomplete="off" readonly spellcheck="false" />
        </label>
        <label class="wide">
          <span>Description</span>
          <textarea id="description" style="min-height: 74px; font-family: inherit"></textarea>
        </label>
        <label class="wide">
          <span>Function body</span>
          <div id="function-body-editor" class="code-editor">
            <pre id="function-body-highlight" class="code-highlight" aria-hidden="true"></pre>
            <textarea id="function-body" spellcheck="false"></textarea>
          </div>
        </label>
        <label>
          <span>Labels JSON</span>
          <textarea id="labels-json" spellcheck="false"></textarea>
        </label>
        <label>
          <span>Meta JSON</span>
          <textarea id="meta-json" spellcheck="false"></textarea>
        </label>
      </div>
      <div class="actions" style="margin-top: 10px">
        <button id="check" type="button">Check</button>
        <button id="save" class="primary" type="button">Save</button>
        <button id="reset" type="button">Reset</button>
        <span id="save-state" class="pill warn">idle</span>
      </div>
    </section>

    <section class="panel" style="margin-top: 14px">
      <div class="topbar">
        <h2>Run</h2>
        <span id="run-state" class="pill warn">idle</span>
      </div>
      <label>
        <span>Request JSON</span>
        <textarea id="request-json" spellcheck="false"></textarea>
      </label>
      <div class="actions" style="margin-top: 10px">
        <button id="run" class="primary" type="button">Run</button>
        <code id="invoke-route">/lambdas/invoke/:function-id</code>
      </div>
      <pre id="output" class="output"></pre>
    </section>
  </main>
</div>"###;

pub(crate) const LAMBDA_FUNCTIONS_JS: &str = r###"const $ = (id) => document.getElementById(id);
const entryCommands = {
  nodejs: "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 node --permission --allow-net child-runtimes/js-function-runner.mjs",
  python3: "env -i PATH=\"$PATH\" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py",
  ruby: "env -i PATH=\"$PATH\" ruby child-runtimes/ruby-function-runner.rb",
  bash: "env -i PATH=\"$PATH\" NODE_NO_WARNINGS=1 node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs",
  golang: "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"golang\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs",
  dart: "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"dart\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs",
  erlang: "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"erlang\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs",
  elixir: "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"elixir\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs",
  java: "env -i PATH=\"$PATH\" LAMBDA_TARGET_RUNTIME=\"java\" NODE_NO_WARNINGS=1 node child-runtimes/polyglot-function-runner.mjs",
  browser: "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 node child-runtimes/browser-function-runner.mjs",
};
const processProfiles = {
  nodejs: {
    runtime: "nodejs",
    poolSlug: "nodejs",
    baseImages: [
      "docker.io/library/dd-lambda-nodejs-runtime:dev",
      "docker.io/library/dd-container-pool-nodejs-runtime:dev",
      "docker.io/library/node:25-alpine",
    ],
  },
  python3: {
    runtime: "python3",
    poolSlug: "python3",
    baseImages: [
      "docker.io/library/dd-lambda-python3-runtime:dev",
      "docker.io/library/dd-container-pool-python3-runtime:dev",
      "docker.io/library/python:3.12-alpine",
    ],
    },
    ruby: {
      runtime: "ruby",
      poolSlug: "ruby",
      baseImages: [
        "docker.io/library/dd-lambda-ruby-runtime:dev",
        "docker.io/library/ruby:3.3-alpine",
      ],
    },
    bash: {
      runtime: "bash",
      poolSlug: "bash",
      baseImages: [
        "docker.io/library/dd-lambda-bash-runtime:dev",
        "docker.io/library/bash:5.3-alpine",
      ],
    },
    golang: {
      runtime: "golang",
      poolSlug: "golang",
      baseImages: [
        "docker.io/library/dd-lambda-golang-runtime:dev",
        "docker.io/library/dd-container-pool-golang-runtime:dev",
        "docker.io/library/golang:1.25-alpine",
      ],
    },
    dart: {
      runtime: "dart",
      poolSlug: "dart",
      baseImages: [
        "docker.io/library/dd-lambda-dart-runtime:dev",
        "docker.io/library/dart:stable",
      ],
    },
    erlang: {
      runtime: "erlang",
      poolSlug: "erlang",
      baseImages: [
        "docker.io/library/dd-lambda-erlang-runtime:dev",
        "docker.io/library/erlang:28-alpine",
      ],
    },
    elixir: {
      runtime: "elixir",
      poolSlug: "elixir",
      baseImages: [
        "docker.io/library/dd-lambda-elixir-runtime:dev",
        "docker.io/library/elixir:1.18-alpine",
      ],
    },
    java: {
      runtime: "java",
      poolSlug: "java",
      baseImages: [
        "docker.io/library/dd-lambda-java-runtime:dev",
        "docker.io/library/eclipse-temurin:21-jdk-alpine",
      ],
    },
    browser: {
      runtime: "browser",
      poolSlug: "browser",
      baseImages: [
        "docker.io/library/dd-lambda-browser-runtime:dev",
      ],
    },
    rust: {
      runtime: "nodejs",
      poolSlug: "rust",
    requiresContainerPool: true,
    baseImages: [
      "docker.io/library/dd-container-pool-rust-runtime:dev",
      "docker.io/library/rust:1.90-bookworm",
      "docker.io/library/rust:1.90-alpine",
    ],
  },
  gleamlang: {
    runtime: "nodejs",
    poolSlug: "gleamlang",
    requiresContainerPool: true,
    baseImages: [
      "docker.io/library/dd-container-pool-gleamlang-runtime:dev",
      "ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine",
      "docker.io/library/erlang:27-alpine",
    ],
  },
};
const hostAllowedRuntimes = new Set(["nodejs"]);
const defaultCommand = entryCommands.nodejs;
const defaultContainerRunner = "containerd-ctr";
const state = {
  functions: [],
  selectedId: null,
  queryAutofillActive: false,
    editorDirty: false,
    bodyProfile: "nodejs",
    activeProfile: "nodejs",
    draftLoadToken: 0,
    draftSaveTimer: null,
  };
const queryParams = new URLSearchParams(location.search);
const autofillParamNames = [
  "slug", "name", "displayName", "title", "description", "status", "runtime",
  "processProfile", "profile", "process", "containerized", "container",
  "containerRunner", "runner", "baseImage", "image", "reuseKey",
  "idleTimeoutSeconds", "idleTimeout", "maxRunMs", "maxRun", "functionBody",
  "body", "code", "source", "request", "requestJson", "payload", "labels",
  "labelsJson", "meta", "metaData", "metaJson", "containerPoolTimeoutMs",
];
const codeKeywordSets = {
  nodejs: new Set([
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
    "delete", "do", "else", "export", "extends", "false", "finally", "for", "from",
    "function", "if", "import", "in", "instanceof", "let", "new", "null", "return",
    "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void",
    "while", "yield",
  ]),
  rust: new Set([
    "as", "async", "await", "break", "const", "continue", "crate", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
    "trait", "true", "type", "unsafe", "use", "where", "while",
  ]),
    golang: new Set([
      "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
      "for", "func", "go", "goto", "if", "import", "interface", "map", "nil", "package",
      "range", "return", "select", "struct", "switch", "type", "var",
    ]),
    dart: new Set([
      "abstract", "as", "async", "await", "base", "break", "case", "catch", "class", "const",
      "continue", "default", "deferred", "do", "dynamic", "else", "enum", "export", "extends",
      "extension", "external", "factory", "false", "final", "finally", "for", "Function",
      "if", "implements", "import", "in", "interface", "is", "late", "library", "mixin",
      "new", "null", "on", "operator", "part", "required", "return", "sealed", "static",
      "super", "switch", "sync", "this", "throw", "true", "try", "typedef", "var", "void",
      "when", "while", "with", "yield",
    ]),
    erlang: new Set([
      "after", "and", "andalso", "band", "begin", "bnot", "bor", "bsl", "bsr", "bxor",
      "case", "catch", "cond", "div", "end", "fun", "if", "let", "not", "of", "or",
      "orelse", "receive", "rem", "try", "when", "xor",
    ]),
    elixir: new Set([
      "after", "alias", "and", "case", "catch", "cond", "def", "defmodule", "defp", "do",
      "else", "end", "false", "fn", "for", "if", "import", "in", "nil", "not", "or",
      "quote", "raise", "receive", "require", "rescue", "super", "throw", "true", "try",
      "unless", "unquote", "use", "when",
    ]),
    java: new Set([
      "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class",
      "const", "continue", "default", "do", "double", "else", "enum", "extends", "false",
      "final", "finally", "float", "for", "goto", "if", "implements", "import", "instanceof",
      "int", "interface", "long", "native", "new", "null", "package", "private", "protected",
      "public", "return", "short", "static", "strictfp", "super", "switch", "synchronized",
      "this", "throw", "throws", "transient", "true", "try", "void", "volatile", "while",
    ]),
    gleamlang: new Set([
      "as", "assert", "case", "const", "echo", "else", "external", "fn", "if", "import",
      "let", "opaque", "panic", "pub", "todo", "type", "use",
  ]),
    python3: new Set([
      "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
      "elif", "else", "except", "False", "finally", "for", "from", "global", "if",
      "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
      "return", "True", "try", "while", "with", "yield",
    ]),
    ruby: new Set([
      "BEGIN", "END", "alias", "and", "begin", "break", "case", "class", "def", "defined?",
      "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next",
      "nil", "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then",
      "true", "undef", "unless", "until", "when", "while", "yield",
    ]),
    bash: new Set([
      "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if",
      "in", "return", "select", "then", "time", "until", "while",
    ]),
  };
const commentPatterns = {
    nodejs: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    rust: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    golang: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    dart: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    java: String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`,
    gleamlang: String.raw`\/\/[^\n]*`,
    erlang: String.raw`%[^\n]*`,
    elixir: String.raw`#[^\n]*`,
    python3: String.raw`#[^\n]*`,
    bash: String.raw`#[^\n]*`,
    ruby: String.raw`#[^\n]*`,
};

function queryParam(...names) {
  for (const name of names) {
    if (!queryParams.has(name)) continue;
    const value = queryParams.get(name);
    if (value !== null && value !== "") return value;
  }
  return null;
}

function queryHas(...names) {
  return names.some((name) => queryParams.has(name));
}

function queryBoolean(value, fallback = false) {
  if (value === null) return fallback;
  const normalized = String(value).trim().toLowerCase();
  if (["1", "true", "yes", "on"].includes(normalized)) return true;
  if (["0", "false", "no", "off"].includes(normalized)) return false;
  return fallback;
}

function jsonText(value, fallback) {
  if (value === null) return JSON.stringify(fallback, null, 2);
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

function jsonValue(value, fallback) {
  if (value === null) return fallback;
  try {
    return JSON.parse(value);
  } catch {
    return fallback;
  }
}

function selectValue(id, value) {
  if (value === null) return;
  const select = $(id);
  const option = Array.from(select.options).find((item) => item.value === value);
  if (option) select.value = value;
}

function ensureSelectValue(id, value) {
  if (value === null) return;
  const select = $(id);
  if (!Array.from(select.options).some((item) => item.value === value)) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    select.appendChild(option);
  }
  select.value = value;
}

  function normalizeRuntime(value) {
    if (value === "javascript" || value === "typescript" || value === "node") return "nodejs";
    if (value === "python") return "python3";
    if (value === "shell") return "bash";
    if (value === "go") return "golang";
    if (value === "erl") return "erlang";
    if (value === "ex") return "elixir";
    if (value === "jvm") return "java";
    if (["playwright", "puppeteer", "chromium", "headless", "scraper"].includes(value)) return "browser";
    return entryCommands[value] ? value : "nodejs";
  }

function normalizeProcessProfile(value) {
  const key = String(value || "").trim().toLowerCase();
    if (key === "gleam") return "gleamlang";
    if (key === "go") return "golang";
    if (key === "python") return "python3";
    if (key === "node") return "nodejs";
    if (key === "erl") return "erlang";
    if (key === "ex") return "elixir";
    if (key === "jvm") return "java";
    return processProfiles[key] ? key : "nodejs";
  }

function processProfileForRuntime(runtime) {
  const raw = String(runtime || "").trim().toLowerCase();
    if (raw === "go" || raw === "golang") return "golang";
    if (raw === "rust") return "rust";
    if (raw === "gleam" || raw === "gleamlang") return "gleamlang";
    const normalized = normalizeRuntime(runtime);
    if (processProfiles[normalized]) return normalized;
    return "nodejs";
  }

function deploymentMeta(metaData) {
  const value = metaData?.lambdaDeployment;
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function processProfileForFunction(fn) {
  const configured = deploymentMeta(fn?.metaData).processProfile;
  if (configured) return normalizeProcessProfile(configured);
  return processProfileForRuntime(fn?.runtime || "nodejs");
}

function selectedProcessProfile() {
  return processProfiles[normalizeProcessProfile($("process-profile").value)] || processProfiles.nodejs;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function codeLanguage() {
  const profile = normalizeProcessProfile($("process-profile").value);
  return codeKeywordSets[profile] ? profile : normalizeRuntime($("runtime").value);
}

function highlightCode(source, language) {
  const keywords = codeKeywordSets[language] || codeKeywordSets.nodejs;
  const commentPattern = commentPatterns[language] || commentPatterns.nodejs;
  const baseTokenPattern = [
    '`(?:\\\\[\\s\\S]|[^`\\\\])*`',
    '"(?:\\\\[\\s\\S]|[^"\\\\])*"',
    "'(?:\\\\[\\s\\S]|[^'\\\\])*'",
    "\\b\\d+(?:\\.\\d+)?\\b",
    "\\b[A-Za-z_][A-Za-z0-9_!?]*\\b",
    "[{}()[\\].,;:+\\-*/%=<>!&|^~#]+",
  ].join("|");
  const tokenPattern = new RegExp(`${commentPattern}|${baseTokenPattern}`, "g");
  const commentTokenPattern = new RegExp(`^(?:${commentPattern})$`);
  let html = "";
  let index = 0;
  for (const match of source.matchAll(tokenPattern)) {
    const token = match[0];
    html += escapeHtml(source.slice(index, match.index));
    let className = "";
    if (commentTokenPattern.test(token)) className = "tok-comment";
    else if (token.startsWith("\"") || token.startsWith("'") || token.startsWith("`")) className = "tok-string";
    else if (/^\d/.test(token)) className = "tok-number";
    else if (keywords.has(token)) className = "tok-keyword";
    else if (/^[{}()[\].,;:+\-*/%=<>!&|^~#]+$/.test(token)) className = "tok-punct";
    html += className
      ? `<span class="${className}">${escapeHtml(token)}</span>`
      : escapeHtml(token);
    index = match.index + token.length;
  }
  html += escapeHtml(source.slice(index));
  return html || "\n";
}

function syncCodeScroll() {
  const textarea = $("function-body");
  const highlight = $("function-body-highlight");
  if (!textarea || !highlight) return;
  highlight.scrollTop = textarea.scrollTop;
  highlight.scrollLeft = textarea.scrollLeft;
}

function updateCodeHighlight() {
  const textarea = $("function-body");
  const highlight = $("function-body-highlight");
  if (!textarea || !highlight) return;
  highlight.innerHTML = highlightCode(textarea.value, codeLanguage());
  highlight.dataset.language = codeLanguage();
  syncCodeScroll();
}

  function setFunctionBody(value) {
    $("function-body").value = value;
    updateCodeHighlight();
  }

  function draftFunctionKey(fn = selectedFunction()) {
    if (fn?.id) return `id:${fn.id}`;
    const slug = normalizeSlug($("slug")?.value || fn?.slug || "");
    return slug ? `slug:${slug}` : "new";
  }

  function draftStorageKey(profileName, functionKey = draftFunctionKey()) {
    return `dd-lambda-function-draft:v2:${functionKey}:${normalizeProcessProfile(profileName)}`;
  }

  function serviceWorkerRequest(message, timeoutMs = 1000) {
    if (!("serviceWorker" in navigator)) return Promise.resolve(null);
    return navigator.serviceWorker.ready.then((registration) => {
      const target = registration.active || navigator.serviceWorker.controller;
      if (!target) return null;
      return new Promise((resolve) => {
        const channel = new MessageChannel();
        const timer = setTimeout(() => resolve(null), timeoutMs);
        channel.port1.onmessage = (event) => {
          clearTimeout(timer);
          resolve(event.data || null);
        };
        target.postMessage(message, [channel.port2]);
      });
    }).catch(() => null);
  }

  function storeDraftInServiceWorker(key, record) {
    void serviceWorkerRequest({ type: "dd-lambda-draft-save", key, record }, 1000);
  }

  function loadLocalDraft(profileName, functionKey = draftFunctionKey()) {
    try {
      const raw = window.localStorage.getItem(draftStorageKey(profileName, functionKey));
      if (!raw) return null;
      const record = JSON.parse(raw);
      return record && typeof record.body === "string" ? record : null;
    } catch {
      return null;
    }
  }

  function persistLanguageDraft(profileName = state.activeProfile || normalizeProcessProfile($("process-profile").value)) {
    const normalizedProfile = normalizeProcessProfile(profileName);
    const key = draftStorageKey(normalizedProfile);
    const record = {
      schema: "dd.lambda.functionDraft.v2",
      functionKey: draftFunctionKey(),
      profile: normalizedProfile,
      runtime: normalizeRuntime((processProfiles[normalizedProfile] || processProfiles.nodejs).runtime),
      body: $("function-body").value,
      updatedAt: new Date().toISOString(),
    };
    try {
      window.localStorage.setItem(key, JSON.stringify(record));
    } catch {
      // localStorage can be unavailable in hardened browser contexts; the
      // service worker cache is the secondary same-origin draft store.
    }
    storeDraftInServiceWorker(key, record);
    return record;
  }

  function queueLanguageDraftPersist() {
    clearTimeout(state.draftSaveTimer);
    state.draftSaveTimer = setTimeout(() => persistLanguageDraft(), 250);
  }

  function restoreServiceWorkerDraft(profileName, functionKey, token) {
    const key = draftStorageKey(profileName, functionKey);
    void serviceWorkerRequest({ type: "dd-lambda-draft-load", key }, 1200).then((reply) => {
      const record = reply?.ok && reply.record && typeof reply.record.body === "string" ? reply.record : null;
      if (!record || token !== state.draftLoadToken) return;
      if (draftFunctionKey() !== functionKey) return;
      if (normalizeProcessProfile($("process-profile").value) !== normalizeProcessProfile(profileName)) return;
      if (state.editorDirty) return;
      setFunctionBody(record.body);
      state.bodyProfile = generatedDefaultProfile(record.body) || null;
    });
  }

  function bodyForProfile(profileName, fallback, functionKey = draftFunctionKey()) {
    const draft = loadLocalDraft(profileName, functionKey);
    if (draft?.body !== undefined) return draft.body;
    const token = ++state.draftLoadToken;
    restoreServiceWorkerDraft(profileName, functionKey, token);
    return fallback;
  }

  function registerLambdaServiceWorker() {
    if (!("serviceWorker" in navigator) || !window.isSecureContext) return;
    navigator.serviceWorker.register("/service-worker.js", { scope: "/" }).catch(() => {});
  }

function containerPoolFunctionBody(profileName) {
  const profile = processProfiles[normalizeProcessProfile(profileName)] || processProfiles.nodejs;
  return [
    "const payload = request.body ?? request;",
    `return await context.containerPool.dispatch("${profile.poolSlug}", payload, {`,
    "  path: \"/invoke\",",
    "  timeoutMs: Number(context.meta.metaData?.lambdaDeployment?.containerPoolTimeoutMs || 30000),",
    "});",
  ].join("\n");
}

  function defaultFunctionBody(runtimeOrProfile) {
    const profileName = processProfiles[runtimeOrProfile]
      ? runtimeOrProfile
      : processProfileForRuntime(runtimeOrProfile);
    switch (profileName) {
      case "python3":
        return [
          "def handler(request, context):",
          "    return { \"status\": 200, \"body\": { \"ok\": True, \"echo\": request.get(\"body\") } }",
          "",
          "result = handler(request, context)",
        ].join("\n");
      case "ruby":
        return [
          "def handler(request, context)",
          "  { status: 200, body: { ok: true, echo: request[\"body\"] } }",
          "end",
          "",
          "handler(request, context)",
        ].join("\n");
      case "bash":
        return [
          "handler() {",
          "  printf '%s\\n' '{\"status\":200,\"body\":{\"ok\":true}}'",
          "}",
          "",
          "handler",
        ].join("\n");
      case "golang":
        return [
          "package main",
          "",
          "func Handler(request map[string]any, context map[string]any) (any, error) {",
          "  return map[string]any{",
          "    \"status\": 200,",
          "    \"body\": map[string]any{",
          "      \"ok\": true,",
          "      \"echo\": request[\"body\"],",
          "    },",
          "  }, nil",
          "}",
        ].join("\n");
      case "dart":
        return [
          "dynamic handler(Map<String, dynamic> request, Map<String, dynamic> context) {",
          "  return {",
          "    \"status\": 200,",
          "    \"body\": {",
          "      \"ok\": true,",
          "      \"echo\": request[\"body\"],",
          "    },",
          "  };",
          "}",
        ].join("\n");
      case "erlang":
        return [
          "-module(handler).",
          "-export([handle/2]).",
          "-spec handle(binary(), binary()) -> binary().",
          "",
          "handle(_RequestJson, _ContextJson) ->",
          "  <<\"{\\\"status\\\":200,\\\"body\\\":{\\\"ok\\\":true}}\">>.",
        ].join("\n");
      case "elixir":
        return [
          "defmodule Handler do",
          "  @spec handle(binary(), binary()) :: binary()",
          "  def handle(_request_json, _context_json) do",
          "    ~s({\"status\":200,\"body\":{\"ok\":true}})",
          "  end",
          "end",
        ].join("\n");
      case "java":
        return [
          "public final class Handler {",
          "  public static String handle(String requestJson, String contextJson) throws Exception {",
          "    return \"{\\\"status\\\":200,\\\"body\\\":{\\\"ok\\\":true}}\";",
          "  }",
          "}",
        ].join("\n");
      case "browser":
        return [
          "const target = request.body?.url ?? request.url ?? \"https://example.com\";",
          "await context.page.goto(target);",
          "return { status: 200, body: { title: await context.page.title(), url: context.page.url() } };",
        ].join("\n");
      case "rust":
      case "gleamlang":
        return containerPoolFunctionBody(profileName);
      case "nodejs":
        return [
          "async function handler(request, context) {",
          "  return { status: 200, body: { ok: true, echo: request.body ?? null } };",
          "}",
          "",
          "return await handler(request, context);",
        ].join("\n");
      default:
        return defaultFunctionBody("nodejs");
    }
  }

function normalizedBody(value) {
  return String(value || "").trim().replace(/\r\n/g, "\n");
}

function generatedDefaultProfile(value) {
  const body = normalizedBody(value);
  if (!body) return "";
  for (const profileName of Object.keys(processProfiles)) {
    if (body === normalizedBody(defaultFunctionBody(profileName))) return profileName;
  }
  return "";
}

function shouldReplaceGeneratedBody(previousProfile) {
  const body = $("function-body").value;
  if (!body.trim()) return true;
  if (state.bodyProfile && normalizedBody(body) === normalizedBody(defaultFunctionBody(state.bodyProfile))) {
    return true;
  }
  if (previousProfile && normalizedBody(body) === normalizedBody(defaultFunctionBody(previousProfile))) {
    return true;
  }
  return Boolean(generatedDefaultProfile(body));
}

function markEditorDirty() {
  state.editorDirty = true;
}

  function markBodyDirty() {
    state.bodyProfile = generatedDefaultProfile($("function-body").value) || null;
    updateCodeHighlight();
    queueLanguageDraftPersist();
    markEditorDirty();
  }

function syncEntryCommand() {
  $("entry-command").value = entryCommands[normalizeRuntime($("runtime").value)] || defaultCommand;
}

function syncBaseImages(preferred = "") {
  const profile = selectedProcessProfile();
  const select = $("base-image");
  const current = preferred || select.value;
  select.textContent = "";
  const images = profile.baseImages || processProfiles.nodejs.baseImages;
  const selected = images.includes(current) ? current : images[0];
  for (const image of images) {
    const option = document.createElement("option");
    option.value = image;
    option.textContent = image;
    select.appendChild(option);
  }
  select.value = selected;
}

function syncContainerPolicy() {
  const requiresContainer = !hostAllowedRuntimes.has(normalizeRuntime($("runtime").value));
  $("containerized").disabled = requiresContainer;
  $("containerized").title = requiresContainer ? "This runtime requires container execution." : "";
  if (requiresContainer) $("containerized").checked = true;
}

  function syncProcessProfile(options = {}) {
    const profileName = normalizeProcessProfile($("process-profile").value);
    const profile = processProfiles[profileName] || processProfiles.nodejs;
    $("process-profile").value = profileName;
    $("runtime").value = profile.runtime;
    state.activeProfile = profileName;
    syncEntryCommand();
    syncContainerPolicy();
    syncBaseImages(options.baseImage || "");
    if (profile.requiresContainerPool) $("containerized").checked = false;
    if (options.restoreBody || options.replaceBody) {
      const functionKey = options.functionKey || draftFunctionKey();
      const fallback = options.bodyFallback ?? defaultFunctionBody(profileName);
      const body = bodyForProfile(profileName, fallback, functionKey);
      setFunctionBody(body);
      state.bodyProfile = generatedDefaultProfile(body) || null;
    }
    updateCodeHighlight();
  }

function deploymentMetaFromControls(existingMeta = {}) {
  const profileName = normalizeProcessProfile($("process-profile").value);
  const profile = processProfiles[profileName] || processProfiles.nodejs;
  const existingDeployment = deploymentMeta(existingMeta);
  const timeout = queryParam("containerPoolTimeoutMs");
  return {
    ...existingDeployment,
    ...(timeout ? { containerPoolTimeoutMs: Number(timeout) || timeout } : {}),
    processProfile: profileName,
    poolSlug: profile.poolSlug,
    runtime: profile.runtime,
    baseImage: $("base-image").value,
    containerRunner: $("container-runner").value || defaultContainerRunner,
  };
}

function applyQueryAutofill() {
  if (!autofillParamNames.some((name) => queryParams.has(name))) return;
  state.queryAutofillActive = true;
  const runtimeValue = queryParam("runtime");
  const profileValue = queryParam("processProfile", "profile", "process");
  const profileName = profileValue
    ? normalizeProcessProfile(profileValue)
    : runtimeValue
      ? processProfileForRuntime(runtimeValue)
      : normalizeProcessProfile($("process-profile").value);
  const bodyValue = queryParam("functionBody", "body", "code", "source");

  $("process-profile").value = profileName;
  syncProcessProfile({
    baseImage: queryParam("baseImage", "image") || "",
    replaceBody: bodyValue === null,
  });
  selectValue("container-runner", queryParam("containerRunner", "runner"));
  ensureSelectValue("base-image", queryParam("baseImage", "image"));

  const slug = queryParam("slug");
  if (slug !== null) $("slug").value = normalizeSlug(slug);
  const displayName = queryParam("displayName", "name", "title");
  if (displayName !== null) $("display-name").value = displayName;
  if (!$("display-name").value && $("slug").value) $("display-name").value = $("slug").value;
  const description = queryParam("description");
  if (description !== null) $("description").value = description;
  selectValue("status", queryParam("status"));
  const reuseKey = queryParam("reuseKey");
  if (reuseKey !== null) $("reuse-key").value = reuseKey;
  const idleTimeout = queryParam("idleTimeoutSeconds", "idleTimeout");
  if (idleTimeout !== null) $("idle-timeout").value = idleTimeout;
  const maxRun = queryParam("maxRunMs", "maxRun");
  if (maxRun !== null) $("max-run").value = maxRun;
  if (queryHas("containerized", "container")) {
    $("containerized").checked = queryBoolean(queryParam("containerized", "container"), $("containerized").checked);
  }
  syncContainerPolicy();
  if (bodyValue !== null) {
    setFunctionBody(bodyValue);
    state.bodyProfile = generatedDefaultProfile(bodyValue) || null;
  }

  const labels = queryParam("labels", "labelsJson");
  if (labels !== null) $("labels-json").value = jsonText(labels, []);
  const metaText = queryParam("metaData", "meta", "metaJson");
  const metaData = metaText === null ? parseJsonField("meta-json", {}) : jsonValue(metaText, {});
  metaData.lambdaDeployment = deploymentMetaFromControls(metaData);
  $("meta-json").value = JSON.stringify(metaData, null, 2);
  const request = queryParam("request", "requestJson", "payload");
  if (request !== null) $("request-json").value = jsonText(request, {});

  $("editor-title").textContent = $("display-name").value || "New function";
  $("editor-subtitle").textContent = $("slug").value || "query draft";
  $("invoke-route").textContent = "/lambdas/invoke/:function-id";
  state.editorDirty = true;
  setSaveState("query autofilled", "ok");
}

function normalizeSlug(value) {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 120);
}

function fmt(value) {
  if (!value) return "never";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function parseJsonField(id, fallback) {
  const value = $(id).value.trim();
  if (!value) return fallback;
  return JSON.parse(value);
}

function selectedFunction() {
  return state.functions.find((fn) => fn.id === state.selectedId) || null;
}

function functionPayload() {
  const profileName = normalizeProcessProfile($("process-profile").value);
  const profile = processProfiles[profileName] || processProfiles.nodejs;
  const metaData = parseJsonField("meta-json", {});
  metaData.lambdaDeployment = deploymentMetaFromControls(metaData);
  return {
    slug: normalizeSlug($("slug").value),
    displayName: $("display-name").value.trim(),
    description: $("description").value.trim(),
    runtime: normalizeRuntime(profile.runtime),
    entryCommand: entryCommands[normalizeRuntime(profile.runtime)] || defaultCommand,
    functionBody: $("function-body").value,
    reuseKey: $("reuse-key").value.trim() || null,
    idleTimeoutSeconds: Number($("idle-timeout").value || 300),
    maxRunMs: Number($("max-run").value || 30000),
    containerized: $("containerized").checked,
    status: $("status").value,
    labels: parseJsonField("labels-json", []),
    metaData,
  };
}

function clearFieldErrors() {
  for (const node of document.querySelectorAll(".field-invalid")) {
    node.classList.remove("field-invalid");
  }
  for (const node of document.querySelectorAll(".field-hint")) {
    node.remove();
  }
}

function setFieldError(id, message) {
  const field = $(id);
  if (!field) return;
  field.classList.add("field-invalid");
  const editor = field.closest(".code-editor");
  if (editor) editor.classList.add("field-invalid");
  const label = field.closest("label");
  if (!label || label.querySelector(".field-hint")) return;
  const hint = document.createElement("div");
  hint.className = "field-hint";
  hint.textContent = message;
  label.appendChild(hint);
}

function validationIssue(field, id, message) {
  return { field, id, message };
}

function validateDraft() {
  clearFieldErrors();
  const errors = [];
  let labels = [];
  let metaData = {};
  const slug = normalizeSlug($("slug").value);
  if (!slug) errors.push(validationIssue("Slug", "slug", "Slug is required."));
  if (slug && slug.length < 3) errors.push(validationIssue("Slug", "slug", "Slug must be at least 3 characters."));
  const displayName = $("display-name").value.trim();
  if (!displayName) errors.push(validationIssue("Name", "display-name", "Name is required."));
  const functionBody = $("function-body").value;
  if (!functionBody.trim()) errors.push(validationIssue("Function body", "function-body", "Function body is required."));
  try {
    labels = parseJsonField("labels-json", []);
    if (!Array.isArray(labels)) {
      errors.push(validationIssue("Labels JSON", "labels-json", "Labels JSON must be an array."));
    }
  } catch (error) {
    errors.push(validationIssue("Labels JSON", "labels-json", `Labels JSON is invalid: ${error.message}`));
  }
  try {
    metaData = parseJsonField("meta-json", {});
    if (!metaData || typeof metaData !== "object" || Array.isArray(metaData)) {
      errors.push(validationIssue("Meta JSON", "meta-json", "Meta JSON must be an object."));
    }
  } catch (error) {
    errors.push(validationIssue("Meta JSON", "meta-json", `Meta JSON is invalid: ${error.message}`));
  }
  for (const issue of errors) setFieldError(issue.id, issue.message);
  if (errors.length) return { errors, payload: null };
  const profileName = normalizeProcessProfile($("process-profile").value);
  const profile = processProfiles[profileName] || processProfiles.nodejs;
  metaData.lambdaDeployment = deploymentMetaFromControls(metaData);
  return {
    errors,
    payload: {
      slug,
      displayName,
      description: $("description").value.trim(),
      runtime: normalizeRuntime(profile.runtime),
      entryCommand: entryCommands[normalizeRuntime(profile.runtime)] || defaultCommand,
      functionBody,
      reuseKey: $("reuse-key").value.trim() || null,
      idleTimeoutSeconds: Number($("idle-timeout").value || 300),
      maxRunMs: Number($("max-run").value || 30000),
      containerized: $("containerized").checked,
      status: $("status").value,
      labels,
      metaData,
    },
  };
}

function renderIssues(title, issues) {
  $("output").textContent = JSON.stringify({ ok: false, title, issues }, null, 2);
}

async function backendSyntaxCheck(payload) {
  const response = await fetch("/lambdas/check", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await response.json().catch(() => ({ ok: false, error: `HTTP ${response.status}` }));
  if (response.status === 404 || response.status === 405) {
    return { ok: false, status: response.status, data: { ok: false, error: "Backend compile check route is not deployed yet." } };
  }
  return { ok: response.ok && data.ok !== false, status: response.status, data };
}

async function checkDraft() {
  const { errors, payload } = validateDraft();
  if (errors.length || !payload) {
    setSaveState(`${errors.length} field issue${errors.length === 1 ? "" : "s"}`, "bad");
    renderIssues("Fix required fields", errors);
    return { ok: false };
  }
  setSaveState("checking");
  try {
    const remote = await backendSyntaxCheck(payload);
    if (!remote.ok) {
      setFieldError("function-body", remote.data?.error || "Backend compile check failed.");
      setSaveState("backend check failed", "bad");
      $("output").textContent = JSON.stringify(remote.data, null, 2);
      return { ok: false };
    }
    setSaveState("backend check passed", "ok");
    $("output").textContent = JSON.stringify(remote.data || { ok: true }, null, 2);
    return { ok: true, payload };
  } catch (error) {
    const message = `Backend check unavailable: ${error instanceof Error ? error.message : String(error)}`;
    setFieldError("function-body", message);
    setSaveState("backend check unavailable", "bad");
    $("output").textContent = JSON.stringify({ ok: false, error: message }, null, 2);
    return { ok: false };
  }
}

function setSaveState(message, kind = "warn") {
  const node = $("save-state");
  node.textContent = message;
  node.className = kind === "bad" ? "pill bad" : kind === "ok" ? "pill" : "pill warn";
}

function setRunState(message, kind = "warn") {
  const node = $("run-state");
  node.textContent = message;
  node.className = kind === "bad" ? "pill bad" : kind === "ok" ? "pill" : "pill warn";
}

  function fillEditor(fn) {
    persistLanguageDraft();
    state.selectedId = fn?.id || null;
    $("editor-title").textContent = fn?.displayName || "New function";
    $("editor-subtitle").textContent = fn?.slug || "draft";
  $("slug").value = fn?.slug || "";
  $("display-name").value = fn?.displayName || "";
  $("status").value = fn?.status || "draft";
    const profileName = processProfileForFunction(fn);
    const functionKey = draftFunctionKey(fn);
    const lambdaDeployment = deploymentMeta(fn?.metaData);
    $("process-profile").value = profileName;
    state.activeProfile = profileName;
    $("runtime").value = normalizeRuntime(fn?.runtime || processProfiles[profileName]?.runtime || "nodejs");
  $("container-runner").value = lambdaDeployment.containerRunner || defaultContainerRunner;
  $("reuse-key").value = fn?.reuseKey || "";
  $("idle-timeout").value = fn?.idleTimeoutSeconds || 300;
  $("max-run").value = fn?.maxRunMs || 30000;
  syncEntryCommand();
  $("containerized").checked = Boolean(fn?.containerized);
  syncContainerPolicy();
  syncBaseImages(lambdaDeployment.baseImage || "");
    $("container-image").value = fn?.containerImage || "";
    $("container-build-status").value = fn?.containerBuildStatus || (fn?.containerized ? "pending" : "not_requested");
    $("description").value = fn?.description || "";
    const body = bodyForProfile(profileName, fn?.functionBody || defaultFunctionBody(profileName), functionKey);
    setFunctionBody(body);
    state.bodyProfile = generatedDefaultProfile(body) || null;
  $("labels-json").value = JSON.stringify(fn?.labels ?? [], null, 2);
  $("meta-json").value = JSON.stringify(fn?.metaData ?? {}, null, 2);
  $("request-json").value = JSON.stringify({ body: { ping: "pong" } }, null, 2);
  $("invoke-route").textContent = `/lambdas/invoke/${fn?.id || ":function-id"}`;
  $("output").textContent = "";
  setSaveState("idle");
  setRunState("idle");
  state.editorDirty = false;
  clearFieldErrors();
  updateCodeHighlight();
  renderFunctions();
}

function renderFunctions() {
  const list = $("function-list");
  list.textContent = "";
  const search = $("search").value.trim().toLowerCase();
  const functions = state.functions.filter((fn) => {
    const haystack = `${fn.id} ${fn.slug} ${fn.displayName} ${fn.description}`.toLowerCase();
    return !search || haystack.includes(search);
  });
  $("snapshot-meta").textContent = `${functions.length} of ${state.functions.length} functions`;
  if (!functions.length) {
    const empty = document.createElement("p");
    empty.className = "muted";
    empty.textContent = "No functions found.";
    list.appendChild(empty);
    return;
  }
  for (const fn of functions) {
    const details = document.createElement("details");
    details.open = fn.id === state.selectedId;
    const summary = document.createElement("summary");
    const left = document.createElement("span");
    const title = document.createElement("span");
    title.className = "summary-title";
    title.textContent = fn.displayName || fn.slug;
    const meta = document.createElement("span");
    meta.className = "summary-meta";
    const processProfile = processProfileForFunction(fn);
    const lambdaDeployment = deploymentMeta(fn.metaData);
    const mode = fn.containerized ? `container ${fn.containerBuildStatus || "pending"}` : "host";
    const runner = lambdaDeployment.containerRunner ? ` - ${lambdaDeployment.containerRunner}` : "";
    meta.textContent = `${fn.slug} - ${fn.id.slice(0, 8)} - ${processProfile} via ${normalizeRuntime(fn.runtime)} - ${mode}${runner} - updated ${fmt(fn.updatedAt)}`;
    left.append(title, meta);
    const status = document.createElement("span");
    status.className = fn.status === "active" ? "pill" : fn.status === "paused" ? "pill warn" : "pill bad";
    status.textContent = fn.status;
    summary.append(left, status);
    summary.addEventListener("click", () => {
      state.queryAutofillActive = false;
      fillEditor(fn);
    });
    const body = document.createElement("div");
    body.className = "details-body";
    const description = document.createElement("p");
    description.textContent = fn.description || "No description";
    const actions = document.createElement("div");
    actions.className = "actions";
    const edit = document.createElement("button");
    edit.type = "button";
    edit.textContent = "Edit";
    edit.addEventListener("click", () => {
      state.queryAutofillActive = false;
      fillEditor(fn);
    });
    const run = document.createElement("button");
    run.type = "button";
    run.className = "primary";
    run.textContent = "Run";
    run.addEventListener("click", () => {
      state.queryAutofillActive = false;
      fillEditor(fn);
      invokeSelected().catch((error) => {
        setRunState("failed", "bad");
        $("output").textContent = String(error);
      });
    });
    actions.append(edit, run);
    body.append(description, actions);
    details.append(summary, body);
    list.appendChild(details);
  }
}

async function load() {
  const response = await fetch("/api/lambdas/functions?limit=250", { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`GET /api/lambdas/functions failed: HTTP ${response.status}`);
  }
  const data = await response.json();
  state.functions = Array.isArray(data.functions) ? data.functions : [];
  if (state.selectedId) {
    const stillSelected = selectedFunction();
    if (stillSelected && !state.editorDirty) fillEditor(stillSelected);
  } else if (!state.editorDirty && !state.queryAutofillActive && state.functions.length) {
    fillEditor(state.functions[0]);
  } else if (!state.editorDirty && !state.queryAutofillActive) {
    fillEditor(null);
  }
  renderFunctions();
}

  async function save() {
    setSaveState("saving");
    persistLanguageDraft();
    const checked = await checkDraft();
  if (!checked.ok) {
    return;
  }
  const payload = checked.payload;
  const current = selectedFunction();
  const route = current ? `/api/lambdas/functions/${encodeURIComponent(current.id)}` : "/api/lambdas/functions";
  const response = await fetch(route, {
    method: current ? "PATCH" : "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok || !data.ok) {
    setSaveState("failed", "bad");
    $("output").textContent = JSON.stringify(data, null, 2);
    return;
  }
  setSaveState("saved", "ok");
  await load();
  const saved = state.functions.find((fn) => fn.id === data.function?.id);
  if (saved) {
    state.queryAutofillActive = false;
    fillEditor(saved);
  }
}

async function invokeSelected() {
  const current = selectedFunction();
  const functionId = current?.id;
  if (!functionId) {
    setRunState("save first", "bad");
    return;
  }
  const request = parseJsonField("request-json", {});
  $("invoke-route").textContent = `/lambdas/invoke/${functionId}`;
  setRunState("running");
  const response = await fetch(`/lambdas/invoke/${encodeURIComponent(functionId)}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
  const text = await response.text();
  setRunState(response.ok ? "complete" : "failed", response.ok ? "ok" : "bad");
  try {
    $("output").textContent = JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    $("output").textContent = text;
  }
}

$("refresh").addEventListener("click", () => load().catch((error) => setSaveState(String(error), "bad")));
$("new-function").addEventListener("click", () => {
  state.queryAutofillActive = false;
  registerLambdaServiceWorker();
  fillEditor(null);
});
$("search").addEventListener("input", renderFunctions);
  $("slug").addEventListener("input", () => {
    markEditorDirty();
    $("slug").value = normalizeSlug($("slug").value);
    queueLanguageDraftPersist();
    $("invoke-route").textContent = `/lambdas/invoke/${selectedFunction()?.id || ":function-id"}`;
  });
  $("runtime").addEventListener("change", () => {
    persistLanguageDraft(state.activeProfile || normalizeProcessProfile($("process-profile").value));
    $("process-profile").value = processProfileForRuntime($("runtime").value);
    syncProcessProfile({ restoreBody: true });
    markEditorDirty();
  });
  $("process-profile").addEventListener("change", () => {
    const previousProfile = state.activeProfile || generatedDefaultProfile($("function-body").value);
    persistLanguageDraft(previousProfile);
    syncProcessProfile({ restoreBody: true });
    markEditorDirty();
    setSaveState(`${normalizeProcessProfile($("process-profile").value)} draft restored`, "warn");
  });
for (const id of [
  "display-name", "status", "container-runner", "base-image", "containerized",
  "reuse-key", "idle-timeout", "max-run", "description", "labels-json", "meta-json",
  "request-json",
]) {
  $(id).addEventListener("input", markEditorDirty);
  $(id).addEventListener("change", markEditorDirty);
}
$("function-body").addEventListener("input", markBodyDirty);
$("function-body").addEventListener("scroll", syncCodeScroll);
$("check").addEventListener("click", () => checkDraft().catch((error) => {
  setSaveState("check failed", "bad");
  $("output").textContent = String(error);
}));
$("reset").addEventListener("click", () => {
  fillEditor(selectedFunction());
  if (state.queryAutofillActive && !selectedFunction()) applyQueryAutofill();
});
$("save").addEventListener("click", () => save().catch((error) => {
  setSaveState("failed", "bad");
  $("output").textContent = String(error);
}));
$("run").addEventListener("click", () => invokeSelected().catch((error) => {
  setRunState("failed", "bad");
  $("output").textContent = String(error);
}));

fillEditor(null);
applyQueryAutofill();
const handleLoadError = (error) => {
  setSaveState("load failed", "bad");
  $("snapshot-meta").textContent = String(error);
};
load().catch(handleLoadError);
  setInterval(() => load().catch(handleLoadError), 15000);
  "###;

