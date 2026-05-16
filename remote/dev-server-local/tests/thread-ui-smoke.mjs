import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmod, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import http from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import net from "node:net";
import { randomUUID } from "node:crypto";
import { chromium } from "playwright";

const packageRoot = resolve(new URL("..", import.meta.url).pathname);
const threadId = "00000000-0000-4000-8000-000000000001";
const userId = "00000000-0000-4000-8000-000000000002";
const serverSecret = "local-thread-ui-smoke-secret-32chars";

async function findOpenPort() {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      assert(address && typeof address === "object");
      const port = address.port;
      server.close(() => resolvePort(port));
    });
  });
}

async function writeExecutable(path, content) {
  await writeFile(path, content);
  await chmod(path, 0o755);
}

async function writeFakeToolchain(binDir) {
  await writeExecutable(
    join(binDir, "git"),
    `#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  status)
    exit 0
    ;;
  ls-remote)
    exit 0
    ;;
  push)
    echo "[fake-git] push $*"
    exit 0
    ;;
  *)
    echo "[fake-git] $*"
    exit 0
    ;;
esac
`,
  );

  await writeExecutable(
    join(binDir, "gh"),
    `#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "pr" && "$2" == "view" ]]; then
  exit 1
fi
if [[ "$1" == "pr" && "$2" == "create" ]]; then
  echo "https://github.com/ORESoftware/k8s-cluster/pull/1"
  exit 0
fi
echo "[fake-gh] $*"
exit 0
`,
  );

  await writeExecutable(
    join(binDir, "pnpm"),
    `#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "exec" && "$2" == "tsx" ]]; then
  shift 2
  exec "${process.execPath}" --import tsx "$@"
fi
echo "[fake-pnpm] $*"
exit 0
`,
  );

  await writeExecutable(
    join(binDir, "claude"),
    `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"local claude ui smoke response"}]}}'
exit 0
`,
  );
}

async function waitForJson(url, init, timeoutMs = 20_000) {
  const started = Date.now();
  let lastError;
  while (Date.now() - started < timeoutMs) {
    try {
      const response = await fetch(url, init);
      if (response.ok) {
        return await response.json();
      }
      lastError = new Error(`${url} returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await sleep(300);
  }
  throw lastError ?? new Error(`timed out waiting for ${url}`);
}

function onceExit(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve();
  }
  return new Promise((resolveExit) => {
    child.once("exit", resolveExit);
  });
}

async function stopChild(child) {
  const gracefulExit = onceExit(child);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
  }
  const stopped = await Promise.race([
    gracefulExit.then(() => true),
    sleep(5_000).then(() => false),
  ]);
  if (stopped) {
    return;
  }

  const forcedExit = onceExit(child);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
  }
  await Promise.race([forcedExit, sleep(5_000)]);
}

async function readRequestBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function createUiServer(apiBaseUrl) {
  const html = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>Remote Dev UI Smoke</title>
    <style>
      body { font-family: system-ui, sans-serif; margin: 32px; max-width: 720px; }
      label, textarea, button { display: block; width: 100%; }
      textarea { min-height: 96px; margin: 8px 0 12px; }
      button { width: auto; padding: 8px 14px; }
      li { margin: 8px 0; }
    </style>
  </head>
  <body>
    <h1>Remote Dev Thread</h1>
    <label for="prompt">Claude instructions</label>
    <textarea id="prompt">Claude UI smoke test: write one sentence and finish.</textarea>
    <button id="start" type="button">Start thread task</button>
    <ol id="tasks"></ol>
    <script>
      const threadId = "${threadId}";
      const userId = "${userId}";
      const tasks = new Map();

      async function refresh() {
        const response = await fetch("/api/tasks");
        const snapshot = await response.json();
        for (const task of snapshot.tasks ?? []) {
          const row = tasks.get(task.taskId);
          if (!row) continue;
          row.dataset.taskStatus = task.finished ? "finished" : "running";
          row.textContent = task.finished
            ? task.taskId + " finished on " + task.threadId
            : task.taskId + " running on " + task.threadId;
        }
      }

      document.getElementById("start").addEventListener("click", async () => {
        const prompt = document.getElementById("prompt").value;
        const response = await fetch("/api/tasks", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            taskId: crypto.randomUUID(),
            threadId,
            userId,
            provider: "claude-cli",
            prompt
          })
        });
        if (!response.ok) {
          throw new Error(await response.text());
        }
        const body = await response.json();
        const row = document.createElement("li");
        row.dataset.taskId = body.taskId;
        row.dataset.threadId = threadId;
        row.dataset.taskStatus = "queued";
        row.textContent = body.taskId + " queued";
        tasks.set(body.taskId, row);
        document.getElementById("tasks").append(row);
      });

      setInterval(refresh, 250);
    </script>
  </body>
</html>`;

  const server = http.createServer(async (request, response) => {
    try {
      if (request.method === "GET" && request.url === "/") {
        response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
        response.end(html);
        return;
      }

      if (request.method === "GET" && request.url === "/api/tasks") {
        const upstream = await fetch(`${apiBaseUrl}/tasks`, {
          headers: { "X-Server-Auth": serverSecret },
        });
        response.writeHead(upstream.status, {
          "Content-Type": upstream.headers.get("content-type") ?? "application/json",
        });
        response.end(await upstream.text());
        return;
      }

      if (request.method === "POST" && request.url === "/api/tasks") {
        const upstream = await fetch(`${apiBaseUrl}/tasks`, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Server-Auth": serverSecret,
          },
          body: await readRequestBody(request),
        });
        response.writeHead(upstream.status, {
          "Content-Type": upstream.headers.get("content-type") ?? "application/json",
        });
        response.end(await upstream.text());
        return;
      }

      response.writeHead(404, { "Content-Type": "text/plain" });
      response.end("not found");
    } catch (error) {
      response.writeHead(500, { "Content-Type": "text/plain" });
      response.end(error instanceof Error ? error.message : String(error));
    }
  });

  return server;
}

async function submitTaskViaUiProxy(uiBaseUrl, prompt) {
  const taskId = randomUUID();
  const response = await fetch(`${uiBaseUrl}/api/tasks`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      taskId,
      threadId,
      userId,
      provider: "claude-cli",
      prompt,
    }),
  });
  if (!response.ok) {
    throw new Error(`UI proxy dispatch returned ${response.status}: ${await response.text()}`);
  }
  return taskId;
}

async function waitForFinishedTaskViaUiProxy(uiBaseUrl, taskId) {
  const started = Date.now();
  while (Date.now() - started < 35_000) {
    const snapshot = await waitForJson(`${uiBaseUrl}/api/tasks`, undefined);
    const task = snapshot.tasks?.find((entry) => entry.taskId === taskId);
    if (task?.finished) {
      return task;
    }
    await sleep(250);
  }
  throw new Error(`UI proxy task ${taskId} did not finish in time`);
}

const apiPort = await findOpenPort();
const uiPort = await findOpenPort();
const apiBaseUrl = `http://127.0.0.1:${apiPort}`;
const uiBaseUrl = `http://127.0.0.1:${uiPort}`;
const tempRoot = await mkdtemp(join(tmpdir(), "dd-dev-server-local-ui-smoke-"));
const binDir = join(tempRoot, "bin");
const repoDir = join(tempRoot, "repo");
await mkdir(binDir, { recursive: true });
await mkdir(join(repoDir, ".git"), { recursive: true });
await writeFakeToolchain(binDir);

const server = spawn("pnpm", ["exec", "tsx", "src/server.ts"], {
  cwd: packageRoot,
  env: {
    ...process.env,
    PATH: `${binDir}:${process.env.PATH ?? ""}`,
    NODE_ENV: "test",
    PORT: String(apiPort),
    HOST: "127.0.0.1",
    WORKSPACE_REPO: repoDir,
    OUTPUTS_DIR: join(tempRoot, "outputs"),
    LOG_DIR: join(tempRoot, "logs"),
    REMOTE_DEV_THREAD_ID: threadId,
    THREAD_ID: threadId,
    USER_ID: userId,
    SERVER_AUTH_SECRET: serverSecret,
    ANTHROPIC_API_KEY: "local-ui-smoke-key",
    SUPABASE_URL: "",
    NEXT_PUBLIC_SUPABASE_URL: "",
    SUPABASE_SERVICE_ROLE_KEY: "",
    AGENT_PROVIDER: "claude-cli",
    GH_PAT: "local-gh-token",
    DEFAULT_STORAGE_PROVIDER: "local",
    LOCAL_STORAGE_PUBLIC_BASE_URL: `${apiBaseUrl}/artifacts`,
    HEARTBEAT_URL: "",
    EVENT_INGEST_URL: "",
  },
  stdio: ["ignore", "pipe", "pipe"],
});

let output = "";
server.stdout.on("data", (chunk) => {
  output += chunk.toString("utf8");
});
server.stderr.on("data", (chunk) => {
  output += chunk.toString("utf8");
});

const uiServer = createUiServer(apiBaseUrl);
let browser = null;

try {
  const health = await waitForJson(`${apiBaseUrl}/healthz`, undefined);
  assert.equal(health.ok, true);
  assert.equal(health.pinnedThreadId, threadId);

  await new Promise((resolveListen) => {
    uiServer.listen(uiPort, "127.0.0.1", resolveListen);
  });

  try {
    browser = await chromium.launch({ headless: true });
  } catch (error) {
    const message = error instanceof Error ? error.message.split("\n")[0] : String(error);
    process.stderr.write(
      `Playwright headless launch unavailable; using UI proxy fallback: ${message}\n`,
    );
  }

  if (browser) {
    const page = await browser.newPage();
    await page.goto(uiBaseUrl);
    await page.fill("#prompt", "Claude UI smoke test: first thread instruction.");
    await page.click("#start");
    await page.waitForFunction(
      () => document.querySelectorAll('[data-task-status="finished"]').length === 1,
      undefined,
      { timeout: 35_000 },
    );

    await page.fill("#prompt", "Claude UI smoke test: second queued instruction.");
    await page.click("#start");
    await page.waitForFunction(
      () => document.querySelectorAll('[data-task-status="finished"]').length === 2,
      undefined,
      { timeout: 35_000 },
    );

    const rows = await page.$$eval("#tasks li", (items) =>
      items.map((item) => ({
        taskId: item.getAttribute("data-task-id"),
        threadId: item.getAttribute("data-thread-id"),
        status: item.getAttribute("data-task-status"),
        text: item.textContent,
      })),
    );

    assert.equal(rows.length, 2);
    assert(rows.every((row) => row.threadId === threadId));
    assert(rows.every((row) => row.status === "finished"));
  } else {
    const firstTaskId = await submitTaskViaUiProxy(
      uiBaseUrl,
      "Claude UI smoke test: first thread instruction.",
    );
    const firstTask = await waitForFinishedTaskViaUiProxy(uiBaseUrl, firstTaskId);
    const secondTaskId = await submitTaskViaUiProxy(
      uiBaseUrl,
      "Claude UI smoke test: second queued instruction.",
    );
    const secondTask = await waitForFinishedTaskViaUiProxy(uiBaseUrl, secondTaskId);

    assert.equal(firstTask.threadId, threadId);
    assert.equal(secondTask.threadId, threadId);
    assert.equal(firstTask.finished, true);
    assert.equal(secondTask.finished, true);
  }

  console.log("remote/dev-server-local UI thread smoke passed");
} catch (error) {
  console.error(output);
  throw error;
} finally {
  if (browser) {
    await browser.close();
  }
  await new Promise((resolveClose) => {
    uiServer.close(resolveClose);
  });
  await stopChild(server);
}
