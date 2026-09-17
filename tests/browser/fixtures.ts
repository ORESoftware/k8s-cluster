import { expect, test as base } from "@playwright/test";

/**
 * Every public route the site builds. `routes.spec.ts` cross-checks this list
 * against the generated `dist/` tree, so a new page cannot be added without
 * also being covered here.
 */
export const ROUTES = [
  "/",
  "/privacy/",
  "/account-deletion/",
  "/support/",
] as const;

export type Route = (typeof ROUTES)[number];

export const PAGE_TITLES: Record<Route, string> = {
  "/": "Sonus Auris — A Dashcam for Audio",
  "/privacy/": "Privacy Policy — Sonus Auris",
  "/account-deletion/": "Delete your account and data — Sonus Auris",
  "/support/": "Support — Sonus Auris",
};

export const MOBILE_VIEWPORT = { width: 390, height: 844 };
export const DESKTOP_VIEWPORT = { width: 1440, height: 900 };

/**
 * Auto-applied fixture that fails any test whose page logged a console error,
 * threw an uncaught exception, failed a request, or reached off-origin.
 *
 * The last one is the point of the exercise as much as the first three: this
 * site is meant to ship zero third-party resources (self-hosted font, inline
 * SVG, no analytics), and a `<meta>` CSP of `default-src 'self'` only stays
 * true if nothing ever tries to leave the origin.
 */
export const test = base.extend<{ pageProblems: string[] }>({
  pageProblems: [
    async ({ page, baseURL }, use) => {
      const problems: string[] = [];
      const origin = new URL(baseURL ?? "http://127.0.0.1:4321").origin;

      page.on("console", (message) => {
        if (message.type() === "error") {
          problems.push(`console.error: ${message.text()}`);
        }
      });
      page.on("pageerror", (error) => {
        problems.push(`pageerror: ${error.message}`);
      });
      page.on("requestfailed", (request) => {
        problems.push(
          `requestfailed: ${request.url()} (${request.failure()?.errorText})`,
        );
      });
      page.on("request", (request) => {
        const url = request.url();
        if (!url.startsWith("http")) return;
        if (new URL(url).origin !== origin) {
          problems.push(`off-origin request: ${url}`);
        }
      });

      await use(problems);

      expect(
        problems,
        "page logged errors, failed requests, or requested a third-party origin",
      ).toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
