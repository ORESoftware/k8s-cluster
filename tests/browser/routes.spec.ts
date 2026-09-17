import { readdirSync, statSync } from "node:fs";
import { relative, resolve, sep } from "node:path";

import {
  DESKTOP_VIEWPORT,
  MOBILE_VIEWPORT,
  PAGE_TITLES,
  ROUTES,
  expect,
  test,
} from "./fixtures";

test("the covered route list matches every page the build emits", () => {
  const dist = resolve("dist");
  const walk = (directory: string): string[] =>
    readdirSync(directory).flatMap((entry) => {
      const path = resolve(directory, entry);
      return statSync(path).isDirectory() ? walk(path) : [path];
    });

  const built = walk(dist)
    .filter((path) => path.endsWith(".html"))
    .map(
      (path) =>
        `/${relative(dist, path).split(sep).join("/").replace(/index\.html$/, "")}`,
    )
    .sort();

  expect(
    built,
    "a route was added or removed without updating ROUTES in tests/browser/fixtures.ts",
  ).toEqual([...ROUTES].sort());
});

for (const route of ROUTES) {
  test.describe(`${route}`, () => {
    test("renders cleanly with per-page title and description metadata", async ({
      page,
    }) => {
      const response = await page.goto(route);
      expect(response?.status(), `${route} did not return 200`).toBe(200);

      await expect(page).toHaveTitle(PAGE_TITLES[route]);

      const description = page.locator('head meta[name="description"]');
      await expect(description).toHaveCount(1);
      const descriptionText = (await description.getAttribute("content")) ?? "";
      expect(descriptionText.length, `${route} description is too short`)
        .toBeGreaterThan(40);

      // Canonical + Open Graph identity, which store review and link unfurls
      // both depend on.
      await expect(page.locator('head link[rel="canonical"]')).toHaveCount(1);
      await expect(page.locator('head meta[property="og:title"]')).toHaveAttribute(
        "content",
        PAGE_TITLES[route],
      );

      // Exactly one h1, and it must actually be rendered (not hidden by CSS).
      const h1 = page.locator("h1");
      await expect(h1).toHaveCount(1);
      await expect(h1).toBeVisible();
      expect((await h1.innerText()).trim().length).toBeGreaterThan(0);
    });

    test("enforces the static-host content security policy in the markup", async ({
      page,
    }) => {
      await page.goto(route);

      // GitHub Pages does not serve `public/_headers`, so the policy has to be
      // in the document or it is not enforced at all.
      const csp = page.locator(
        'head meta[http-equiv="Content-Security-Policy"]',
      );
      await expect(csp).toHaveCount(1);
      const policy = (await csp.getAttribute("content")) ?? "";
      for (const directive of [
        "default-src 'self'",
        "script-src 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'self'",
        "frame-src 'none'",
      ]) {
        expect(policy, `${route} CSP is missing ${directive}`).toContain(
          directive,
        );
      }

      await expect(page.locator('head meta[name="referrer"]')).toHaveAttribute(
        "content",
        "strict-origin-when-cross-origin",
      );

      // The policy above is only honest if the page really has no scripts or
      // document-wide base URL override.
      await expect(page.locator("script")).toHaveCount(0);
      await expect(page.locator("base")).toHaveCount(0);
    });

    test("has no duplicate element ids", async ({ page }) => {
      await page.goto(route);

      const duplicates = await page.evaluate(() => {
        const seen = new Map<string, number>();
        for (const element of document.querySelectorAll("[id]")) {
          const id = element.id;
          seen.set(id, (seen.get(id) ?? 0) + 1);
        }
        return [...seen.entries()]
          .filter(([, count]) => count > 1)
          .map(([id, count]) => `${id} x${count}`);
      });

      expect(duplicates, `${route} has duplicate DOM ids`).toEqual([]);
    });

    test("gives every image and every meaningful graphic an accessible name", async ({
      page,
    }) => {
      await page.goto(route);

      const unlabelled = await page.evaluate(() => {
        const problems: string[] = [];

        for (const image of document.querySelectorAll("img")) {
          if (image.getAttribute("alt") === null) {
            problems.push(`img without alt: ${image.getAttribute("src")}`);
          }
        }

        // Inline SVG is this site's only imagery. Anything exposed to the
        // accessibility tree as an image needs a name; purely decorative art
        // must be hidden from it (directly or via an ancestor).
        const graphics = [...document.querySelectorAll('svg[role="img"], svg')];
        for (const svg of graphics) {
          if (svg.closest('[aria-hidden="true"]')) continue;
          const named =
            svg.getAttribute("aria-label") ??
            svg.getAttribute("aria-labelledby") ??
            svg.querySelector("title")?.textContent;
          if (!named) {
            const owner = svg.closest("a, button");
            if (owner?.getAttribute("aria-label")) continue;
            problems.push(
              `svg is neither hidden nor named: ${svg.getAttribute("class") ?? svg.outerHTML.slice(0, 60)}`,
            );
          }
        }

        return problems;
      });

      expect(unlabelled, `${route} has unlabelled graphics`).toEqual([]);
    });

    for (const [label, viewport] of [
      ["mobile", MOBILE_VIEWPORT],
      ["desktop", DESKTOP_VIEWPORT],
    ] as const) {
      test(`does not scroll horizontally at ${label} width (${viewport.width}px)`, async ({
        page,
      }) => {
        await page.setViewportSize(viewport);
        await page.goto(route);

        const overflow = await page.evaluate(() => {
          const root = document.documentElement;
          const limit = root.clientWidth;
          const offenders: string[] = [];
          for (const element of document.querySelectorAll("body *")) {
            const box = element.getBoundingClientRect();
            if (box.width === 0 && box.height === 0) continue;
            if (box.right > limit + 1 || box.left < -1) {
              const name =
                element.tagName.toLowerCase() +
                (element.getAttribute("class")
                  ? `.${element.getAttribute("class")!.split(/\s+/)[0]}`
                  : "");
              if (!offenders.includes(name)) offenders.push(name);
            }
          }
          return {
            scrollWidth: root.scrollWidth,
            clientWidth: limit,
            offenders: offenders.slice(0, 8),
          };
        });

        expect(
          overflow.scrollWidth,
          `${route} overflows at ${viewport.width}px; widest offenders: ${overflow.offenders.join(", ")}`,
        ).toBeLessThanOrEqual(overflow.clientWidth + 1);
      });
    }
  });
}
