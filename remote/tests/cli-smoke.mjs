import assert from "node:assert/strict";
import http from "node:http";
import https from "node:https";

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? "https://54.91.17.58").replace(/\/+$/, "");
const serverSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? "";
const ec2GatewayHost = /^https?:\/\/54\.91\.17\.58(?::|\/|$)/.test(baseUrl);
const allowSelfSignedGatewayCert = ec2GatewayHost && process.env.REMOTE_DEV_STRICT_TLS !== "1";

function authHeaders() {
  return serverSecret ? { "X-Server-Auth": serverSecret } : {};
}

async function text(res) {
  return await res.text();
}

async function json(res) {
  return await res.json();
}

async function get(path, options = {}) {
  return await request(new URL(path, `${baseUrl}/`), options);
}

async function request(url, options = {}, redirectCount = 0) {
  const client = url.protocol === "https:" ? https : http;
  const reqOptions = {
    headers: options.headers ?? {},
    method: "GET",
    rejectUnauthorized: allowSelfSignedGatewayCert ? false : undefined,
  };

  return await new Promise((resolve, reject) => {
    const req = client.request(url, reqOptions, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", async () => {
        const status = res.statusCode ?? 0;
        const location = res.headers.location;
        if (
          options.redirect !== "manual" &&
          location &&
          [301, 302, 303, 307, 308].includes(status)
        ) {
          if (redirectCount >= 5) {
            reject(new Error(`too many redirects after ${url.toString()}`));
            return;
          }
          try {
            resolve(await request(new URL(location, url), options, redirectCount + 1));
          } catch (error) {
            reject(error);
          }
          return;
        }

        const body = Buffer.concat(chunks).toString("utf8");
        resolve({
          status,
          headers: {
            get(name) {
              const value = res.headers[name.toLowerCase()];
              return Array.isArray(value) ? value.join(", ") : value ?? null;
            },
          },
          async text() {
            return body;
          },
          async json() {
            return JSON.parse(body);
          },
        });
      });
    });
    req.on("error", reject);
    req.end();
  });
}

console.log(`[cli-smoke] target=${baseUrl}`);

{
  const res = await get("/", { redirect: "manual" });
  if (baseUrl.startsWith("http://")) {
    const expectedHttpsRoot = new URL("/", baseUrl.replace(/^http:/, "https:")).toString();
    assert.equal(res.status, 308, `GET / expected 308 HTTP->HTTPS redirect, got ${res.status}`);
    assert.equal(res.headers.get("location"), expectedHttpsRoot);
    console.log("[cli-smoke] GET / -> 308 HTTPS redirect");
  } else {
    assert.equal(res.status, 302, `GET / expected 302, got ${res.status}`);
    assert.equal(res.headers.get("location"), "/home");
    console.log("[cli-smoke] GET / -> 302 /home");
  }
}

{
  const res = await get("/home");
  assert.equal(res.status, 200, `GET /home expected 200, got ${res.status}`);
  const body = await text(res);
  assert.match(body, /dd-dev-server/i);
  console.log("[cli-smoke] GET /home -> 200 + homepage html");
}

{
  const res = await get("/healthz");
  assert.equal(res.status, 200, `GET /healthz expected 200, got ${res.status}`);
  const body = await json(res);
  assert.equal(body.ok, true);
  assert.equal(typeof body.serverInstanceId, "string");
  console.log(
    `[cli-smoke] GET /healthz -> ok=true instance=${body.serverInstanceId} pinnedThreadId=${body.pinnedThreadId ?? "null"}`,
  );
}

{
  const res = await get("/tasks");
  assert.equal(res.status, 401, `GET /tasks (unauth) expected 401, got ${res.status}`);
  console.log("[cli-smoke] GET /tasks (unauth) -> 401");
}

if (serverSecret) {
  const statusRes = await get("/status", { headers: authHeaders() });
  assert.equal(statusRes.status, 200, `GET /status (auth) expected 200, got ${statusRes.status}`);
  const statusBody = await json(statusRes);
  assert.equal(typeof statusBody.pinnedThreadId, "string");
  console.log(
    `[cli-smoke] GET /status (auth) -> 200 pinnedThreadId=${statusBody.pinnedThreadId} tracked=${statusBody.totalTracked}`,
  );

  const tasksRes = await get("/tasks", { headers: authHeaders() });
  assert.equal(tasksRes.status, 200, `GET /tasks (auth) expected 200, got ${tasksRes.status}`);
  const tasksBody = await json(tasksRes);
  assert.ok(Array.isArray(tasksBody.tasks), "expected tasks array");
  console.log(`[cli-smoke] GET /tasks (auth) -> 200 tasks=${tasksBody.tasks.length}`);
} else {
  console.log("[cli-smoke] REMOTE_DEV_SERVER_SECRET not set; skipped authenticated endpoint checks");
}

console.log("[cli-smoke] PASS");
