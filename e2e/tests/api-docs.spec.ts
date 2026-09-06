import { expect, test } from "@playwright/test";

const publicPaths = [
  "/",
  "/api/docs",
  "/api/docs.json",
  "/docs/api",
  "/healthz",
  "/openapi.json",
  "/readyz",
  "/v1/analyze",
  "/v1/speech-to-speech",
  "/v1/stt",
  "/v1/translate",
  "/v1/tts",
].sort();

const internalOnlyPaths = [
  "/internal/docs/api",
  "/internal/openapi.json",
  "/metrics",
  "/v1/history/syntheses",
  "/v1/history/transcriptions",
  "/v1/history/translations",
  "/v1/history/vapi-calls",
  "/vapi/call",
  "/vapi/call/{id}",
  "/vapi/webhook",
];

function operationIds(document: any): string[] {
  return Object.values(document.paths ?? {}).flatMap((pathItem: any) =>
    Object.values(pathItem)
      .map((operation: any) => operation?.operationId)
      .filter((value): value is string => typeof value === "string"),
  );
}

test("public Scalar and JSON aliases render the fail-closed speech contract", async ({
  page,
  request,
  baseURL,
}) => {
  const canonical = await request.get(`${baseURL}/openapi.json`);
  const alias = await request.get(`${baseURL}/api/docs.json`);
  expect(canonical.ok()).toBeTruthy();
  expect(alias.ok()).toBeTruthy();
  expect(await alias.body()).toEqual(await canonical.body());

  const document = await canonical.json();
  expect(document.openapi).toBe("3.1.0");
  expect(document["x-dd-contract-scope"]).toBe("public");
  expect(Object.keys(document.paths).sort()).toEqual(publicPaths);
  expect(document.components?.securitySchemes).toBeUndefined();
  for (const path of internalOnlyPaths) {
    expect(document.paths[path]).toBeUndefined();
  }

  const ids = operationIds(document);
  expect(ids.length).toBeGreaterThan(10);
  expect(new Set(ids).size).toBe(ids.length);

  await page.goto(`${baseURL}/api/docs`);
  await expect(page.locator("body")).toContainText("t2v-v2t API (public)");
  await page.goto(`${baseURL}/docs/api`);
  await expect(page.locator("body")).toContainText("t2v-v2t API (public)");
});

test("internal documentation fails closed and exposes the complete route family", async ({
  page,
  request,
  baseURL,
}) => {
  const unauthorized = await request.get(`${baseURL}/internal/openapi.json`);
  expect(unauthorized.status()).toBe(401);

  const wrong = await request.get(`${baseURL}/internal/openapi.json`, {
    headers: { Authorization: "Bearer wrong" },
  });
  expect(wrong.status()).toBe(401);

  const authorized = await request.get(`${baseURL}/internal/openapi.json`, {
    headers: { Authorization: "Bearer browser-server-secret" },
  });
  expect(authorized.ok()).toBeTruthy();
  const document = await authorized.json();
  expect(document["x-dd-contract-scope"]).toBe("internal");
  for (const path of [...publicPaths, ...internalOnlyPaths]) {
    expect(document.paths[path]).toBeDefined();
  }
  expect(document.components.securitySchemes.server_auth).toMatchObject({
    type: "http",
    scheme: "bearer",
  });
  expect(document.components.securitySchemes.vapi_secret).toMatchObject({
    type: "apiKey",
    in: "header",
    name: "x-vapi-secret",
  });
  expect(document.paths["/v1/history/transcriptions"].get.security).toEqual([
    { server_auth: [] },
  ]);
  expect(document.paths["/vapi/webhook"].post.security).toEqual([
    { vapi_secret: [] },
  ]);

  await page.setExtraHTTPHeaders({
    Authorization: "Bearer browser-server-secret",
  });
  await page.goto(`${baseURL}/internal/docs/api`);
  await expect(page.locator("body")).toContainText("t2v-v2t API (internal)");
});

test("runtime routes and documented authentication behavior stay aligned", async ({
  request,
  baseURL,
}) => {
  expect((await request.get(`${baseURL}/healthz`)).status()).toBe(200);
  expect((await request.get(`${baseURL}/readyz`)).status()).toBe(200);

  const historyUnauthorized = await request.get(
    `${baseURL}/v1/history/transcriptions`,
  );
  expect(historyUnauthorized.status()).toBe(401);
  const historyAuthorized = await request.get(
    `${baseURL}/v1/history/transcriptions`,
    { headers: { Authorization: "Bearer browser-server-secret" } },
  );
  expect(historyAuthorized.status()).toBe(200);
  expect(await historyAuthorized.json()).toMatchObject({ ok: true, total: 0 });

  const webhookUnauthorized = await request.post(`${baseURL}/vapi/webhook`, {
    data: { message: { type: "status-update", status: "ringing" } },
  });
  expect(webhookUnauthorized.status()).toBe(401);
  const webhookAuthorized = await request.post(`${baseURL}/vapi/webhook`, {
    headers: { "x-vapi-secret": "browser-vapi-secret" },
    data: { message: { type: "status-update", status: "ringing" } },
  });
  expect(webhookAuthorized.status()).toBe(200);
});
