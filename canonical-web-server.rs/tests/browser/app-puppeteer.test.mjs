import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startServer } from "./app-browser-harness.mjs";

// Browser e2e for the maud/htmx application shell served by the axum server.
// These drive the *unauthenticated* surface (login, protected-route redirect,
// 404) end to end through a real Chrome — complementing the tower `oneshot`
// router tests in tests/app.rs.

async function withBrowser(t) {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    executablePath: chromeExecutablePath(),
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());
  return { server, browser };
}

test("puppeteer: the login page renders the Supabase-backed sign-in form", async (t) => {
  const { server, browser } = await withBrowser(t);
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  const response = await page.goto(`${server.url}/login`, { waitUntil: "networkidle0" });
  assert.equal(response.status(), 200);
  assert.equal(await page.title(), "Sign in · canonical.cloud");

  assert.equal(await page.$eval("main h1", (el) => el.textContent?.trim()), "Sign in");
  // Supabase-delegated auth disclaimer.
  assert.match(await page.$eval("main", (el) => el.innerText), /Authentication is handled by Supabase/);

  // The form posts to /auth/login and is htmx-enhanced.
  const form = await page.$eval("form", (el) => ({
    method: el.getAttribute("method"),
    action: el.getAttribute("action"),
    hxPost: el.getAttribute("hx-post"),
  }));
  assert.equal(form.method, "post");
  assert.match(form.action, /\/auth\/login$/);
  assert.equal(form.hxPost, "/auth/login");

  // Email + password inputs with autofill hints, and a CSRF hidden field.
  assert.equal(await page.$eval('input[name="email"]', (el) => el.type), "email");
  assert.equal(await page.$eval('input[name="password"]', (el) => el.type), "password");
  assert.equal(
    await page.$eval('input[name="csrf"]', (el) => el.value.length > 0),
    true,
  );

  assert.deepEqual(pageErrors, []);
});

test("puppeteer: an unauthenticated visit to /app redirects to the login page", async (t) => {
  const { server, browser } = await withBrowser(t);
  const page = await browser.newPage();

  await page.goto(`${server.url}/app`, { waitUntil: "networkidle0" });
  assert.equal(new URL(page.url()).pathname, "/login");
  assert.equal(await page.$eval("main h1", (el) => el.textContent?.trim()), "Sign in");
});

test("puppeteer: an unknown application path renders the maud 404 page", async (t) => {
  const { server, browser } = await withBrowser(t);
  const page = await browser.newPage();

  const response = await page.goto(`${server.url}/app/does-not-exist`, {
    waitUntil: "networkidle0",
  });
  assert.equal(response.status(), 404);
  assert.equal(await page.title(), "Not found · canonical.cloud");
  assert.match(await page.$eval("main", (el) => el.innerText), /That application page does not exist/);
});
