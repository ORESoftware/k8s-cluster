import { spawnSync } from "node:child_process";
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { extname, join, normalize, resolve } from "node:path";

import { expect, test } from "./fixtures";

/**
 * Regression test for the deployment-variable injection surface.
 *
 * Store listing and desktop download URLs come from repository/environment
 * variables, not from page source. Before hardening, whatever string those
 * variables held was written straight into an `href` — so a mistyped or hostile
 * value (`javascript:…`, plain `http://`) became an active or mixed-content URL
 * on a site that otherwise ships zero JavaScript, and `npm run check` never saw
 * it because CI builds without those variables set.
 *
 * `src/lib/external-url.ts` now requires an absolute https URL. This test
 * rebuilds the whole site with deliberately hostile values, serves that build,
 * and drives it in a real browser to prove the bad values are dropped and the
 * components fall back to their "coming soon" state instead.
 */

const PORT = Number(process.env.SONUS_AURIS_HOSTILE_PORT ?? 4399);
const ORIGIN = `http://127.0.0.1:${PORT}`;
// Deliberately outside Playwright's `outputDir`, which is wiped and repopulated
// while the run is in progress.
const OUT_DIR = resolve(".playwright-hostile-build");

const HOSTILE_ENV = {
  PUBLIC_APP_STORE_URL: "javascript:alert(document.domain)",
  PUBLIC_PLAY_STORE_URL: "http://play.google.example/insecure",
  PUBLIC_DOWNLOAD_BASE_URL: "javascript:void(0)",
  PUBLIC_RUST_DESKTOP_DOWNLOAD_BASE_URL: "javascript:alert('rust')",
  PUBLIC_FLUTTER_DESKTOP_DOWNLOAD_BASE_URL: "http://downloads.example.com/flutter",
  PUBLIC_DESKTOP_TESTERS_URL: "  javascript:alert('testers')  ",
};

const CONTENT_TYPES: Record<string, string> = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

let server: Server | undefined;

test.describe("hostile deployment configuration", () => {
  // Serial: these tests share one build and one server, so they must not be
  // split across workers (which would run the build concurrently into the same
  // directory and race).
  test.describe.configure({ mode: "serial" });
  test.use({ baseURL: ORIGIN });

  test.beforeAll(async () => {
    test.setTimeout(180_000);

    const build = spawnSync(
      process.platform === "win32" ? "npx.cmd" : "npx",
      ["astro", "build", "--outDir", OUT_DIR],
      {
        cwd: process.cwd(),
        encoding: "utf8",
        env: { ...process.env, ...HOSTILE_ENV },
      },
    );
    expect(
      build.status,
      `hostile-config build failed:\n${build.stdout ?? ""}\n${build.stderr ?? ""}`,
    ).toBe(0);
    expect(existsSync(join(OUT_DIR, "index.html"))).toBe(true);

    server = createServer((request, response) => {
      const requestPath = decodeURIComponent(
        new URL(request.url ?? "/", ORIGIN).pathname,
      );
      // Contain the served path inside OUT_DIR — this server only ever runs in
      // tests, but a traversal would make the test itself meaningless.
      const candidate = normalize(join(OUT_DIR, requestPath));
      if (!candidate.startsWith(OUT_DIR)) {
        response.writeHead(403).end();
        return;
      }

      const file =
        existsSync(candidate) && statSync(candidate).isDirectory()
          ? join(candidate, "index.html")
          : candidate;
      if (!existsSync(file) || !statSync(file).isFile()) {
        response.writeHead(404, { "Content-Type": "text/plain" }).end("404");
        return;
      }

      response.writeHead(200, {
        "Content-Type": CONTENT_TYPES[extname(file)] ?? "application/octet-stream",
      });
      createReadStream(file).pipe(response);
    });

    await new Promise<void>((resolveListen, rejectListen) => {
      server!.once("error", rejectListen);
      server!.listen(PORT, "127.0.0.1", resolveListen);
    });
  });

  test.afterAll(async () => {
    await new Promise<void>((done) => {
      if (!server) return done();
      server.close(() => done());
    });
    server = undefined;
  });

  test("drops javascript: and http: URLs supplied by deployment variables", async ({
    page,
  }) => {
    await page.goto("/");

    const dangerous = await page.$$eval("[href], [src], [action]", (elements) =>
      elements
        .flatMap((element) =>
          ["href", "src", "action"].map((attribute) => ({
            attribute,
            value: element.getAttribute(attribute),
          })),
        )
        .filter(
          (entry): entry is { attribute: string; value: string } =>
            typeof entry.value === "string",
        )
        .filter(
          (entry) =>
            /^\s*(?:javascript|vbscript|data):/i.test(entry.value) ||
            /^\s*http:/i.test(entry.value) ||
            entry.value.trim().startsWith("//"),
        )
        .map((entry) => `${entry.attribute}="${entry.value}"`),
    );

    expect(
      dangerous,
      "a hostile deployment variable reached a rendered URL attribute",
    ).toEqual([]);
  });

  test("falls back to the non-linked 'coming soon' state", async ({ page }) => {
    await page.goto("/");

    // Store badges: rendered as inert <span>, never as an anchor. They appear
    // in both the hero and the download section, so assert every instance.
    for (const store of ["App Store", "Google Play"]) {
      const badges = page
        .locator("span.store-badge.is-coming")
        .filter({ hasText: store });
      const count = await badges.count();
      expect(count, `no inert ${store} badge rendered`).toBeGreaterThan(0);
      for (let index = 0; index < count; index += 1) {
        await expect(badges.nth(index)).toBeVisible();
      }
    }
    await expect(page.locator("a.store-badge")).toHaveCount(0);

    // Desktop installers: no links, no checksum links, and the placeholder
    // instead of a tester-folder button.
    await expect(page.locator("a.desktop-button")).toHaveCount(0);
    await expect(page.locator("a.checksum-link")).toHaveCount(0);
    await expect(page.locator(".desktop-button.is-coming")).toBeVisible();
    await expect(
      page.locator(".desktop-button.is-coming"),
    ).toContainText("Downloads opening soon");
  });

  test("still renders the page and its security metadata intact", async ({
    page,
  }) => {
    // A hardened build must degrade, not break: the marketing copy, the CSP,
    // and the compliance links all have to survive the bad configuration.
    await page.goto("/");

    await expect(page.locator("h1")).toBeVisible();
    await expect(
      page.locator('head meta[http-equiv="Content-Security-Policy"]'),
    ).toHaveCount(1);
    await expect(page.locator("script")).toHaveCount(0);
    await expect(
      page.locator("footer.footer").getByRole("link", {
        name: "Privacy policy",
        exact: true,
      }),
    ).toBeVisible();
  });
});
