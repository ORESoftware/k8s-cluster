import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

/**
 * Capture browser-level failures that assertion-only tests otherwise miss.
 * Playwright and Puppeteer expose compatible console/pageerror event shapes.
 */
export function observeBrowserErrors(page) {
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error") {
      errors.push(`console: ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => {
    errors.push(`pageerror: ${error.message}`);
  });
  return errors;
}

/**
 * Persist synthetic browser evidence even when the product journey fails.
 * Capture errors are recorded separately and never mask the original test
 * failure during cleanup.
 */
export async function captureBrowserEvidence(framework, page, browserErrors) {
  const directory = path.join(process.cwd(), "artifacts", "browser", framework);
  await mkdir(directory, { recursive: true });

  const tasks = [
    writeFile(
      path.join(directory, "browser-errors.json"),
      `${JSON.stringify(browserErrors, null, 2)}\n`,
      "utf8",
    ),
  ];

  if (page) {
    tasks.push(
      page.screenshot({
        path: path.join(directory, "customer-journey.png"),
        fullPage: true,
      }),
      page
        .content()
        .then((content) =>
          writeFile(path.join(directory, "page.html"), content, "utf8"),
        ),
    );
  }

  const results = await Promise.allSettled(tasks);
  const failures = results
    .filter((result) => result.status === "rejected")
    .map((result) => String(result.reason?.stack ?? result.reason));

  if (failures.length > 0) {
    await writeFile(
      path.join(directory, "capture-errors.json"),
      `${JSON.stringify(failures, null, 2)}\n`,
      "utf8",
    ).catch(() => {});
  }
}

export function assertNoBrowserErrors(browserErrors) {
  assert.deepEqual(
    browserErrors,
    [],
    `browser emitted errors:\n${browserErrors.join("\n")}`,
  );
}
