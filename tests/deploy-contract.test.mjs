import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import {
  publicEnvKeysForNamedStep,
  publicKeysFromDotenv,
  resolveBuildPath,
  resolveSameOriginBuildTarget,
} from "../scripts/deploy-contract.mjs";

const workflow = `
env:
  PUBLIC_TOP_LEVEL: ignored
jobs:
  verify:
    steps:
      - name: Other step
        env:
          PUBLIC_UNRELATED: ignored
        run: npm test
  build:
    steps:
      - name: Build with Astro
        uses: withastro/action@immutable
        env:
          # A comment inside the target map is allowed.
          PUBLIC_REAL_ONE: value
          PUBLIC_REAL_TWO: value
        with:
          node-version: 22
      - name: Later step
        env:
          PUBLIC_TOO_LATE: ignored
        run: echo done
`;

test("PUBLIC variables are scoped to the uniquely named Astro build step", () => {
  assert.deepEqual(
    [...publicEnvKeysForNamedStep(workflow, "Build with Astro")].sort(),
    ["PUBLIC_REAL_ONE", "PUBLIC_REAL_TWO"],
  );
});

test("an unrelated step cannot satisfy the Astro env contract", () => {
  const withoutRealOne = workflow.replace("          PUBLIC_REAL_ONE: value\n", "");
  const keys = publicEnvKeysForNamedStep(withoutRealOne, "Build with Astro");
  assert.equal(keys.has("PUBLIC_REAL_ONE"), false);
  assert.equal(keys.has("PUBLIC_UNRELATED"), false);
});

test("duplicate or missing named steps fail closed", () => {
  assert.throws(
    () => publicEnvKeysForNamedStep(`${workflow}\n      - name: Build with Astro\n        env:\n          PUBLIC_DUPLICATE: value\n`, "Build with Astro"),
    /exactly one workflow step/,
  );
  assert.throws(
    () => publicEnvKeysForNamedStep(workflow, "Missing step"),
    /exactly one workflow step/,
  );
});

test("missing or inline env maps fail closed", () => {
  assert.throws(
    () =>
      publicEnvKeysForNamedStep(
        "jobs:\n  build:\n    steps:\n      - name: Build with Astro\n        run: npm run build\n",
        "Build with Astro",
      ),
    /block-style env map/,
  );
  assert.throws(
    () =>
      publicEnvKeysForNamedStep(
        "jobs:\n  build:\n    steps:\n      - name: Build with Astro\n        env: { PUBLIC_INLINE: value }\n",
        "Build with Astro",
      ),
    /block-style env map/,
  );
});

test("dotenv documentation exposes only PUBLIC keys", () => {
  assert.deepEqual(
    [...publicKeysFromDotenv("PRIVATE=value\nPUBLIC_ONE=\nexport PUBLIC_TWO=value\n")].sort(),
    ["PUBLIC_ONE", "PUBLIC_TWO"],
  );
});

test("build paths stay within dist and preserve directory indexes", () => {
  const dist = path.resolve("dist-fixture");
  assert.equal(resolveBuildPath("/", dist), path.join(dist, "index.html"));
  assert.equal(resolveBuildPath("/privacy/", dist), path.join(dist, "privacy", "index.html"));
  assert.equal(resolveBuildPath("/og.svg", dist), path.join(dist, "og.svg"));
  assert.throws(() => resolveBuildPath("/%2e%2e%2fsecret", dist), /escapes dist/);
  assert.throws(() => resolveBuildPath("/%ZZ", dist), /valid percent-encoding/);
});

test("metadata targets require an exact origin and reject embedded credentials", () => {
  const dist = path.resolve("dist-fixture");
  assert.equal(
    resolveSameOriginBuildTarget(
      "https://sonusauris.app/og.svg?cache=1#card",
      "https://sonusauris.app",
      dist,
    ),
    path.join(dist, "og.svg"),
  );
  assert.equal(
    resolveSameOriginBuildTarget(
      "https://sonusauris.app.evil.invalid/og.svg",
      "https://sonusauris.app",
      dist,
    ),
    null,
  );
  assert.equal(resolveSameOriginBuildTarget("Product description", "https://sonusauris.app", dist), null);
  assert.throws(
    () =>
      resolveSameOriginBuildTarget(
        "https://user:secret@sonusauris.app/og.svg",
        "https://sonusauris.app",
        dist,
      ),
    /must not contain credentials/,
  );
});
