import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmod, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import net from "node:net";

const packageRoot = resolve(new URL("..", import.meta.url).pathname);
const threadId = "00000000-0000-4000-8000-000000000001";
const taskId = "00000000-0000-4000-8000-000000000101";
const userId = "00000000-0000-4000-8000-000000000002";
const serverSecret = "local-thread-smoke-secret-32chars";

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
printf '%s\\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"local claude smoke response"}]}}'
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

async function waitForFinishedTask(baseUrl) {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    const snapshot = await waitForJson(`${baseUrl}/tasks`, {
      headers: { "X-Server-Auth": serverSecret },
    });
    const task = snapshot.tasks?.find((entry) => entry.taskId === taskId);
    if (task?.finished) {
      return task;
    }
    await sleep(500);
  }
  throw new Error("remote-dev task did not finish in time");
}

function onceExit(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve();
  }
  return new Promise((resolveExit) => {
    child.once("exit", resolveExit);
  });
}

async function stopServer(child) {
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

const port = await findOpenPort();
const baseUrl = `http://127.0.0.1:${port}`;
const tempRoot = await mkdtemp(join(tmpdir(), "dd-dev-server-local-smoke-"));
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
    PORT: String(port),
    HOST: "127.0.0.1",
    WORKSPACE_REPO: repoDir,
    OUTPUTS_DIR: join(tempRoot, "outputs"),
    LOG_DIR: join(tempRoot, "logs"),
    REMOTE_DEV_THREAD_ID: threadId,
    THREAD_ID: threadId,
    USER_ID: userId,
    SERVER_AUTH_SECRET: serverSecret,
    ANTHROPIC_API_KEY: "local-smoke-key",
    SUPABASE_URL: "",
    NEXT_PUBLIC_SUPABASE_URL: "",
    SUPABASE_SERVICE_ROLE_KEY: "",
    AGENT_PROVIDER: "claude-cli",
    GH_PAT: "local-gh-token",
    DEFAULT_STORAGE_PROVIDER: "local",
    LOCAL_STORAGE_PUBLIC_BASE_URL: `${baseUrl}/artifacts`,
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

try {
  const health = await waitForJson(`${baseUrl}/healthz`, undefined);
  assert.equal(health.ok, true);
  assert.equal(health.pinnedThreadId, threadId);

  const dispatch = await fetch(`${baseUrl}/tasks`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Server-Auth": serverSecret,
    },
    body: JSON.stringify({
      taskId,
      threadId,
      userId,
      provider: "claude-cli",
      prompt: "Claude smoke test: write one sentence and finish.",
    }),
  });
  if (dispatch.status !== 200) {
    assert.fail(`dispatch returned ${dispatch.status}: ${await dispatch.text()}`);
  }
  const body = await dispatch.json();
  assert.equal(body.taskId, taskId);

  const finished = await waitForFinishedTask(baseUrl);
  assert.equal(finished.finished, true);
  assert.equal(finished.threadId, threadId);

  const status = await waitForJson(`${baseUrl}/status`, {
    headers: { "X-Server-Auth": serverSecret },
  });
  assert.equal(status.pinnedThreadId, threadId);
  assert.equal(status.totalTracked, 1);

  console.log("remote/dev-server-local thread smoke passed");
} catch (error) {
  console.error(output);
  throw error;
} finally {
  await stopServer(server);
}
