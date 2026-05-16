import assert from "node:assert/strict";

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? "http://54.91.17.58").replace(/\/+$/, "");
const serverSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? "";

function authHeaders() {
  return serverSecret ? { "X-Server-Auth": serverSecret } : {};
}

async function text(res) {
  return await res.text();
}

async function json(res) {
  return await res.json();
}

console.log(`[cli-smoke] target=${baseUrl}`);

{
  const res = await fetch(`${baseUrl}/`, { redirect: "manual" });
  assert.equal(res.status, 302, `GET / expected 302, got ${res.status}`);
  assert.equal(res.headers.get("location"), "/home");
  console.log("[cli-smoke] GET / -> 302 /home");
}

{
  const res = await fetch(`${baseUrl}/home`);
  assert.equal(res.status, 200, `GET /home expected 200, got ${res.status}`);
  const body = await text(res);
  assert.match(body, /dd-dev-server/i);
  console.log("[cli-smoke] GET /home -> 200 + homepage html");
}

{
  const res = await fetch(`${baseUrl}/healthz`);
  assert.equal(res.status, 200, `GET /healthz expected 200, got ${res.status}`);
  const body = await json(res);
  assert.equal(body.ok, true);
  assert.equal(typeof body.serverInstanceId, "string");
  console.log(
    `[cli-smoke] GET /healthz -> ok=true instance=${body.serverInstanceId} pinnedThreadId=${body.pinnedThreadId ?? "null"}`,
  );
}

{
  const res = await fetch(`${baseUrl}/tasks`);
  assert.equal(res.status, 401, `GET /tasks (unauth) expected 401, got ${res.status}`);
  console.log("[cli-smoke] GET /tasks (unauth) -> 401");
}

if (serverSecret) {
  const statusRes = await fetch(`${baseUrl}/status`, { headers: authHeaders() });
  assert.equal(statusRes.status, 200, `GET /status (auth) expected 200, got ${statusRes.status}`);
  const statusBody = await json(statusRes);
  assert.equal(typeof statusBody.pinnedThreadId, "string");
  console.log(
    `[cli-smoke] GET /status (auth) -> 200 pinnedThreadId=${statusBody.pinnedThreadId} tracked=${statusBody.totalTracked}`,
  );

  const tasksRes = await fetch(`${baseUrl}/tasks`, { headers: authHeaders() });
  assert.equal(tasksRes.status, 200, `GET /tasks (auth) expected 200, got ${tasksRes.status}`);
  const tasksBody = await json(tasksRes);
  assert.ok(Array.isArray(tasksBody.tasks), "expected tasks array");
  console.log(`[cli-smoke] GET /tasks (auth) -> 200 tasks=${tasksBody.tasks.length}`);
} else {
  console.log("[cli-smoke] REMOTE_DEV_SERVER_SECRET not set; skipped authenticated endpoint checks");
}

console.log("[cli-smoke] PASS");
