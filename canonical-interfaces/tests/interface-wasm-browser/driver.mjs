import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

const MAX_DISCOVERY_BYTES = 64 * 1024;
const MAX_RESULT_TEXT_BYTES = 16 * 1024;

function usage() {
  throw new Error(
    "usage: node driver.mjs <debug-origin> <target-url> <dom-output> <result-output>",
  );
}

function assertLoopbackHttp(value, label) {
  const url = new URL(value);
  assert.equal(url.protocol, "http:", `${label} must use http`);
  assert.equal(url.hostname, "127.0.0.1", `${label} must use IPv4 loopback`);
  assert.equal(url.username, "", `${label} must not contain credentials`);
  assert.equal(url.password, "", `${label} must not contain credentials`);
  assert.ok(url.port, `${label} must use an explicit port`);
  return url;
}

async function readBoundedText(response, limit) {
  const declared = response.headers.get("content-length");
  if (declared !== null && Number(declared) > limit) {
    throw new Error(`Chromium discovery response exceeds ${limit} bytes`);
  }
  if (!response.body) {
    throw new Error("Chromium discovery response had no body");
  }

  const chunks = [];
  let length = 0;
  for await (const chunk of response.body) {
    length += chunk.byteLength;
    if (length > limit) {
      throw new Error(`Chromium discovery response exceeds ${limit} bytes`);
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function discoverBrowser(debugOrigin) {
  const discoveryUrl = new URL("/json/version", debugOrigin);
  const response = await fetch(discoveryUrl, {
    redirect: "error",
    headers: { accept: "application/json" },
  });
  assert.equal(response.ok, true, `Chromium discovery returned HTTP ${response.status}`);

  const document = JSON.parse(await readBoundedText(response, MAX_DISCOVERY_BYTES));
  assert.equal(typeof document.webSocketDebuggerUrl, "string");
  const socketUrl = new URL(document.webSocketDebuggerUrl);
  assert.equal(socketUrl.protocol, "ws:", "Chromium debugger must use ws");
  assert.equal(socketUrl.hostname, debugOrigin.hostname, "debugger escaped loopback");
  assert.equal(socketUrl.port, debugOrigin.port, "debugger changed ports");
  assert.equal(socketUrl.username, "", "debugger URL must not contain credentials");
  assert.equal(socketUrl.password, "", "debugger URL must not contain credentials");
  return socketUrl;
}

class DevToolsClient {
  constructor(socketUrl) {
    this.socket = new WebSocket(socketUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.waiters = new Set();

    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) {
          pending.reject(new Error(`${message.error.code}: ${message.error.message}`));
        } else {
          pending.resolve(message.result ?? {});
        }
        return;
      }

      for (const waiter of this.waiters) {
        if (
          waiter.method === message.method &&
          (waiter.sessionId === undefined || waiter.sessionId === message.sessionId)
        ) {
          this.waiters.delete(waiter);
          waiter.resolve(message.params ?? {});
          break;
        }
      }
    });

    this.socket.addEventListener("close", () => {
      const error = new Error("Chromium DevTools connection closed unexpectedly");
      for (const pending of this.pending.values()) pending.reject(error);
      this.pending.clear();
      for (const waiter of this.waiters) waiter.reject(error);
      this.waiters.clear();
    });
  }

  async open() {
    if (this.socket.readyState === WebSocket.OPEN) return;
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener(
        "error",
        () => reject(new Error("Chromium DevTools connection failed")),
        { once: true },
      );
    });
  }

  send(method, params = {}, sessionId) {
    const id = this.nextId++;
    const message = { id, method, params };
    if (sessionId !== undefined) message.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify(message));
    });
  }

  waitFor(method, sessionId) {
    return new Promise((resolve, reject) => {
      this.waiters.add({ method, sessionId, resolve, reject });
    });
  }

  close() {
    if (this.socket.readyState < WebSocket.CLOSING) this.socket.close();
  }
}

function valueFromEvaluation(evaluation, label) {
  if (evaluation.exceptionDetails) {
    const description = evaluation.exceptionDetails.exception?.description ??
      evaluation.exceptionDetails.text ??
      "unknown JavaScript exception";
    throw new Error(`${label}: ${description}`);
  }
  return evaluation.result?.value;
}

async function main() {
  const [debugOriginValue, targetUrlValue, domOutput, resultOutput, ...extra] =
    process.argv.slice(2);
  if (!debugOriginValue || !targetUrlValue || !domOutput || !resultOutput || extra.length) {
    usage();
  }

  const debugOrigin = assertLoopbackHttp(debugOriginValue, "debug origin");
  const targetUrl = assertLoopbackHttp(targetUrlValue, "target URL");
  const socketUrl = await discoverBrowser(debugOrigin);
  const client = new DevToolsClient(socketUrl);
  await client.open();

  let targetId;
  try {
    ({ targetId } = await client.send("Target.createTarget", { url: "about:blank" }));
    const { sessionId } = await client.send("Target.attachToTarget", {
      targetId,
      flatten: true,
    });

    await client.send("Page.enable", {}, sessionId);
    await client.send("Runtime.enable", {}, sessionId);
    const loaded = client.waitFor("Page.loadEventFired", sessionId);
    const navigation = await client.send(
      "Page.navigate",
      { url: targetUrl.href },
      sessionId,
    );
    assert.equal(navigation.errorText, undefined, navigation.errorText);
    await loaded;

    const completion = await client.send(
      "Runtime.evaluate",
      {
        expression: `(() => new Promise((resolve) => {
          const result = document.querySelector("#result");
          if (!result) {
            resolve({ status: "fail", stage: "driver", text: "missing #result sentinel" });
            return;
          }
          const read = () => ({
            status: result.dataset.status ?? "",
            stage: result.dataset.stage ?? "",
            text: result.textContent ?? "",
          });
          const finish = () => {
            const state = read();
            if (state.status === "pass" || state.status === "fail") {
              observer.disconnect();
              resolve(state);
            }
          };
          const observer = new MutationObserver(finish);
          observer.observe(result, {
            attributes: true,
            childList: true,
            characterData: true,
            subtree: true,
          });
          finish();
        }))()`,
        awaitPromise: true,
        returnByValue: true,
      },
      sessionId,
    );
    const result = valueFromEvaluation(completion, "browser contract evaluation");
    assert.ok(result && typeof result === "object", "browser contract returned no result");
    assert.ok(
      Buffer.byteLength(String(result.text ?? ""), "utf8") <= MAX_RESULT_TEXT_BYTES,
      "browser contract result text exceeded its evidence bound",
    );

    const domEvaluation = await client.send(
      "Runtime.evaluate",
      {
        expression: "document.documentElement.outerHTML",
        returnByValue: true,
      },
      sessionId,
    );
    const dom = valueFromEvaluation(domEvaluation, "DOM capture");
    assert.equal(typeof dom, "string", "Chromium did not return a DOM snapshot");

    await writeFile(domOutput, `${dom}\n`, "utf8");
    await writeFile(resultOutput, `${JSON.stringify(result, null, 2)}\n`, "utf8");

    if (result.status !== "pass") {
      throw new Error(
        `browser contract reported ${JSON.stringify(result.status)} at ${JSON.stringify(result.stage)}: ${result.text}`,
      );
    }
    console.log(`pass: ${result.text}`);
  } finally {
    if (targetId) {
      await client.send("Target.closeTarget", { targetId }).catch(() => {});
    }
    client.close();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  process.exitCode = 1;
});
