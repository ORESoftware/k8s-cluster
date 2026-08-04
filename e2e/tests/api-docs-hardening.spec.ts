import { expect, test } from "@playwright/test";

function assertHeaders(
  headers: Record<string, string>,
  { privateRoute = false, html = false } = {},
) {
  expect(headers["cache-control"]).toContain("no-store");
  expect(headers.pragma).toContain("no-cache");
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  expect(headers["permissions-policy"]).toContain("camera=()");
  expect(headers["permissions-policy"]).toContain("microphone=()");
  expect(headers["permissions-policy"]).toContain("geolocation=()");
  const vary = (headers.vary ?? "").toLowerCase();
  if (privateRoute) expect(vary).toContain("authorization");
  else expect(vary).not.toContain("authorization");
  if (html) {
    expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
    expect(headers["content-security-policy"]).toContain("base-uri 'none'");
    expect(headers["content-security-policy"]).toContain("object-src 'none'");
    expect(headers["x-frame-options"]).toBe("DENY");
  }
}

test("public documentation responses are cache-isolated, non-sniffable, and non-frameable", async ({
  request,
  baseURL,
}) => {
  for (const path of ["/openapi.json", "/api/docs.json"]) {
    const response = await request.get(`${baseURL}${path}`);
    expect(response.status()).toBe(200);
    assertHeaders(response.headers());

    const head = await request.head(`${baseURL}${path}`);
    expect(head.status()).toBe(200);
    assertHeaders(head.headers());
    expect((await head.body()).length).toBe(0);
  }

  for (const path of ["/api/docs", "/docs/api"]) {
    const response = await request.get(`${baseURL}${path}`);
    expect(response.status()).toBe(200);
    assertHeaders(response.headers(), { html: true });

    const head = await request.head(`${baseURL}${path}`);
    expect(head.status()).toBe(200);
    assertHeaders(head.headers(), { html: true });
    expect((await head.body()).length).toBe(0);
  }
});

test("private docs and operator denials vary on Authorization and issue a bearer challenge", async ({
  request,
  baseURL,
}) => {
  for (const path of [
    "/internal/openapi.json",
    "/internal/docs/api",
    "/v1/history/translations",
  ]) {
    const denied = await request.get(`${baseURL}${path}`);
    expect(denied.status()).toBe(401);
    assertHeaders(denied.headers(), { privateRoute: true });
    expect(denied.headers()["www-authenticate"]).toBe(
      'Bearer realm="t2v-operator"',
    );
    expect(await denied.text()).not.toContain("browser-server-secret");
  }

  for (const path of ["/internal/openapi.json", "/internal/docs/api"]) {
    const authorized = await request.get(`${baseURL}${path}`, {
      headers: { Authorization: "Bearer browser-server-secret" },
    });
    expect(authorized.status()).toBe(200);
    assertHeaders(authorized.headers(), {
      privateRoute: true,
      html: path.endsWith("/api"),
    });
    expect(await authorized.text()).not.toContain("browser-server-secret");
  }
});

test("documentation routes reject unsupported methods without reflecting credentials", async ({
  request,
  baseURL,
}) => {
  for (const path of [
    "/openapi.json",
    "/api/docs.json",
    "/api/docs",
    "/docs/api",
    "/internal/openapi.json",
    "/internal/docs/api",
  ]) {
    const response = await request.post(`${baseURL}${path}`, {
      headers: { Authorization: "Bearer browser-server-secret" },
      data: { token: "browser-server-secret" },
    });
    expect(response.status()).toBe(405);
    assertHeaders(response.headers(), {
      privateRoute: path.startsWith("/internal/"),
    });
    expect(await response.text()).not.toContain("browser-server-secret");
  }
});
