import { DESKTOP_VIEWPORT, ROUTES, expect, test } from "./fixtures";

interface Anchor {
  href: string;
  text: string;
  rel: string;
  target: string;
}

async function anchors(page: import("@playwright/test").Page) {
  return page.$$eval("a[href]", (elements) =>
    elements.map((element) => ({
      href: (element as HTMLAnchorElement).href,
      text: (element.textContent ?? "").replace(/\s+/g, " ").trim(),
      rel: element.getAttribute("rel") ?? "",
      target: element.getAttribute("target") ?? "",
    })),
  ) as Promise<Anchor[]>;
}

for (const route of ROUTES) {
  test.describe(`${route} links`, () => {
    test("resolves every internal link and in-page anchor", async ({
      page,
      request,
      baseURL,
    }) => {
      await page.goto(route);
      const origin = new URL(baseURL!).origin;
      const found = await anchors(page);
      expect(
        found.length,
        `${route} rendered no links at all`,
      ).toBeGreaterThanOrEqual(2);

      const internal = found.filter((a) => new URL(a.href).origin === origin);
      expect(
        internal.length,
        `${route} rendered no internal links`,
      ).toBeGreaterThan(0);

      const broken: string[] = [];
      const checked = new Set<string>();

      for (const anchor of internal) {
        const url = new URL(anchor.href);

        if (url.hash) {
          // In-page anchors are the primary navigation on the homepage; a
          // missing target silently does nothing when clicked.
          const id = decodeURIComponent(url.hash.slice(1));
          const samePage = url.pathname === new URL(page.url()).pathname;
          if (samePage) {
            const exists = await page.evaluate(
              (elementId) => document.getElementById(elementId) !== null,
              id,
            );
            if (!exists) {
              broken.push(`${anchor.href} (no element with id "${id}")`);
            }
            continue;
          }
        }

        const target = `${url.origin}${url.pathname}`;
        if (checked.has(target)) continue;
        checked.add(target);

        const response = await request.get(target, { maxRedirects: 5 });
        if (!response.ok()) {
          broken.push(`${target} -> HTTP ${response.status()}`);
        }
      }

      expect(broken, `${route} has broken internal links`).toEqual([]);
    });

    test("isolates and secures every off-origin link", async ({
      page,
      baseURL,
    }) => {
      await page.goto(route);
      const origin = new URL(baseURL!).origin;
      const external = (await anchors(page)).filter((anchor) => {
        const url = new URL(anchor.href);
        return url.protocol.startsWith("http") && url.origin !== origin;
      });

      // /privacy/ and /account-deletion/ deliberately link nowhere off-origin
      // (mailto only), but the homepage and /support/ must, or the assertions
      // below would be silently vacuous.
      if (route === "/" || route === "/support/") {
        expect(
          external.length,
          `${route} rendered no external links to check`,
        ).toBeGreaterThan(0);
      }

      const violations: string[] = [];
      for (const anchor of external) {
        const url = new URL(anchor.href);
        if (url.protocol !== "https:") {
          violations.push(`${anchor.href} is not https`);
        }
        const tokens = new Set(anchor.rel.toLowerCase().split(/\s+/));
        if (!tokens.has("noopener")) {
          violations.push(`${anchor.href} is missing rel=noopener`);
        }
        if (!tokens.has("noreferrer")) {
          violations.push(`${anchor.href} is missing rel=noreferrer`);
        }
      }

      expect(violations, `${route} has unsafe external links`).toEqual([]);
    });

    test("never renders an active or protocol-relative URL", async ({
      page,
    }) => {
      await page.goto(route);

      const active = await page.$$eval(
        "[href], [src], [action]",
        (elements) =>
          elements
            .flatMap((element) =>
              ["href", "src", "action"].map((attribute) =>
                element.getAttribute(attribute),
              ),
            )
            .filter((value): value is string => typeof value === "string")
            .filter((value) =>
              /^(?:javascript|vbscript|data):/i.test(value.trim()) ||
              /^http:/i.test(value.trim()) ||
              value.trim().startsWith("//"),
            ),
      );

      expect(active, `${route} rendered an active or insecure URL`).toEqual([]);
    });
  });
}

test.describe("primary navigation and footer chrome", () => {
  test.use({ viewport: DESKTOP_VIEWPORT });

  test("the homepage nav jumps to every section it advertises", async ({
    page,
  }) => {
    await page.goto("/");

    const nav = page.locator('header.nav nav[aria-label="Primary"]');
    await expect(nav).toBeVisible();

    const sections = [
      ["Features", "features"],
      ["How it works", "how"],
      ["Privacy", "privacy"],
      ["Open source", "open-source"],
    ] as const;

    for (const [label, id] of sections) {
      const link = nav.getByRole("link", { name: label, exact: true });
      await expect(link).toBeVisible();
      await link.click();
      await expect(page).toHaveURL(new RegExp(`#${id}$`));

      const section = page.locator(`#${id}`);
      await expect(section).toBeInViewport();
    }

    // The primary CTA has to reach the download section.
    await page.getByRole("link", { name: "Get the app" }).click();
    await expect(page).toHaveURL(/#download$/);
    await expect(page.locator("#download")).toBeInViewport();
  });

  test("the footer exposes the compliance routes on the homepage", async ({
    page,
  }) => {
    await page.goto("/");
    const footer = page.locator("footer.footer");
    await expect(footer).toBeVisible();

    for (const [name, path] of [
      ["Privacy policy", "/privacy/"],
      ["Delete your account & data", "/account-deletion/"],
      ["Support", "/support/"],
    ] as const) {
      const link = footer.getByRole("link", { name, exact: true });
      await expect(link).toBeVisible();
      await expect(link).toHaveAttribute("href", path);
    }

    await expect(
      footer.getByRole("link", { name: "Responsible recording" }),
    ).toHaveAttribute("href", "/privacy/#recording-responsibly");
  });

  test("every route offers a way back to the homepage", async ({ page }) => {
    for (const route of ROUTES) {
      await page.goto(route);
      const home = page.locator('a[href="/"]').first();
      await expect(home, `${route} has no link home`).toBeVisible();
    }
  });

  test("the legal pages keep their back link and reach the homepage", async ({
    page,
  }) => {
    for (const route of ["/privacy/", "/account-deletion/", "/support/"]) {
      await page.goto(route);
      const back = page.locator(".legal-back a");
      await expect(back).toBeVisible();
      await back.click();
      await expect(page).toHaveURL("/");
      await expect(page.locator("header.nav")).toBeVisible();
    }
  });
});
