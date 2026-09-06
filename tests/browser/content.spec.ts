import { expect, test } from "./fixtures";

/**
 * Release-critical copy.
 *
 * `scripts/verify-build.mjs` already greps the generated HTML for some of this,
 * but a string can be present in the markup and still be invisible to a reader
 * (collapsed container, `display:none`, zero-height overflow). These assertions
 * require the claim to be *rendered and visible* in a real browser, because the
 * privacy, consent, and store-readiness statements below are what the app-store
 * reviews and the privacy promises on the homepage are judged against.
 */

const HOMEPAGE_CLAIMS = [
  // Consent gating — recording never starts on its own.
  "After you tap Start or arm a schedule",
  // Encryption and zero-knowledge storage.
  "Encrypted on-device",
  "Zero-knowledge cloud",
  "Your backups are ciphertext to us",
  // No monetisation of user audio.
  "No ads, no resale",
  "We don't sell, mine, or train on your audio",
  // Deletion.
  "Yours to delete",
  // Open source.
  "The mobile app is open source",
  // Legal disclaimer that must not quietly become a stronger claim.
  "Record responsibly.",
  "it does not certify authenticity or admissibility",
  // Store-readiness honesty about download buttons.
  "become active only after a reviewed production release is available",
];

const PRIVACY_PAGE_CLAIMS = [
  "We do not sell your data.",
  "We do not use your data for advertising.",
  "We do not have access to the contents of your encrypted recordings.",
  "We do not record unless you start capture or explicitly arm a recording schedule.",
  "never later than the approved 100-hour ceiling",
  "is outside Sonus Auris automatic retention",
  "Exporting does not delay deletion of the app-private copy",
  "not directed to children under 13",
];

const DELETION_PAGE_CLAIMS = [
  "Settings → Delete account",
  "What gets deleted vs. retained",
  "within 30 days",
  "a web/email request cannot remotely erase a device",
];

test("the homepage keeps every privacy and product claim visible", async ({
  page,
}) => {
  await page.goto("/");

  for (const claim of HOMEPAGE_CLAIMS) {
    await expect(
      page.getByText(claim, { exact: false }).first(),
      `homepage no longer shows: ${claim}`,
    ).toBeVisible();
  }

  // The trust badges are the compressed version of the same promises.
  for (const badge of [
    "End-to-end encrypted",
    "Privacy-first by design",
    "Open-source app",
  ]) {
    await expect(page.getByText(badge).first()).toBeVisible();
  }
});

test("the privacy policy keeps its publisher identity and commitments visible", async ({
  page,
}) => {
  await page.goto("/privacy/");

  for (const claim of PRIVACY_PAGE_CLAIMS) {
    await expect(
      page.getByText(claim, { exact: false }).first(),
      `privacy policy no longer shows: ${claim}`,
    ).toBeVisible();
  }

  // Store review rejects a policy without a reachable publisher and contact.
  await expect(page.getByText("ORESoftware").first()).toBeVisible();
  const contact = page.locator('a[href^="mailto:"]').first();
  await expect(contact).toBeVisible();
  await expect(contact).toHaveAttribute("href", /^mailto:.+@.+\..+/);

  // The deletion pathway must stay linked from the policy.
  await expect(
    page.getByRole("link", { name: "Account & data deletion" }),
  ).toHaveAttribute("href", "/account-deletion/");

  // Deep link used by the footer and by in-app links.
  await expect(page.locator("#recording-responsibly")).toBeVisible();
});

test("the account deletion page keeps its store-required instructions visible", async ({
  page,
}) => {
  await page.goto("/account-deletion/");

  for (const claim of DELETION_PAGE_CLAIMS) {
    await expect(
      page.getByText(claim, { exact: false }).first(),
      `account deletion page no longer shows: ${claim}`,
    ).toBeVisible();
  }

  const mailto = page.locator('a[href^="mailto:"]').first();
  await expect(mailto).toHaveAttribute("href", /subject=Delete/i);
});

test("configured downloads render as usable, isolated links", async ({
  page,
}) => {
  await page.goto("/");

  // The suite builds with download fixtures configured (see
  // playwright.config.ts), so this is the production-shaped rendering path.
  const download = page.locator("#download");
  await expect(download).toBeVisible();

  const installers = download.locator("a.desktop-button");
  await expect(installers.first()).toBeVisible();
  const count = await installers.count();
  expect(count, "no desktop installer links rendered").toBeGreaterThan(0);

  for (let index = 0; index < count; index += 1) {
    const link = installers.nth(index);
    const href = (await link.getAttribute("href")) ?? "";
    expect(href, "installer link is not https").toMatch(/^https:\/\//);
    await expect(
      link,
      `installer ${href} does not isolate its opener`,
    ).toHaveAttribute("rel", /(?=.*noopener)(?=.*noreferrer)/);
  }

  // Checksums are how a tester verifies an unsigned build; the link must exist.
  const checksums = download.locator("a.checksum-link");
  expect(await checksums.count()).toBeGreaterThan(0);
  await expect(checksums.first()).toHaveAttribute("href", /\/SHA256SUMS$/);
  await expect(checksums.first()).toHaveAttribute(
    "rel",
    /(?=.*noopener)(?=.*noreferrer)/,
  );

  // Store badges become real links once configured.
  const appStore = page
    .getByRole("link", { name: "Download on the App Store" })
    .first();
  await expect(appStore).toHaveAttribute("href", /^https:\/\//);
});
