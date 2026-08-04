import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import {
  publicEnvForNamedStep,
  publicEnvKeysForNamedStep,
  publicKeysFromDotenv,
  resolveBuildPath,
  resolveSameOriginBuildTarget,
} from "../scripts/deploy-contract.mjs";

const productionStepName = "Build and verify the exact production-root static site";
const workflow = `
env:
  PUBLIC_TOP_LEVEL: ignored
jobs:
  build:
    steps:
      - name: Other step
        env:
          PUBLIC_UNRELATED: ignored
        run: npm test
      - name: ${productionStepName}
        run: npm run check
        env:
          # A comment inside the target map is allowed.
          PUBLIC_REAL_ONE: value-one
          PUBLIC_REAL_TWO: value-two
      - name: Later step
        env:
          PUBLIC_TOO_LATE: ignored
        run: echo done
`;

test("PUBLIC variables are scoped to the uniquely named production build step", () => {
  assert.deepEqual(
    [...publicEnvKeysForNamedStep(workflow, productionStepName)].sort(),
    ["PUBLIC_REAL_ONE", "PUBLIC_REAL_TWO"],
  );
  assert.deepEqual(
    [...publicEnvForNamedStep(workflow, productionStepName).entries()].sort(),
    [
      ["PUBLIC_REAL_ONE", "value-one"],
      ["PUBLIC_REAL_TWO", "value-two"],
    ],
  );
});

test("an unrelated step cannot satisfy the production env contract", () => {
  const targetBlock = [
    "          PUBLIC_REAL_ONE: value-one",
    "          PUBLIC_REAL_TWO: value-two",
    "      - name: Later step",
  ].join("\n");
  assert.ok(workflow.includes(targetBlock), "fixture must contain the intended production env block");
  const withoutRealOne = workflow.replace(
    targetBlock,
    ["          PUBLIC_REAL_TWO: value-two", "      - name: Later step"].join("\n"),
  );
  const keys = publicEnvKeysForNamedStep(withoutRealOne, productionStepName);
  assert.equal(keys.has("PUBLIC_REAL_ONE"), false);
  assert.equal(keys.has("PUBLIC_UNRELATED"), false);
});

test("duplicate or missing production steps fail closed", () => {
  assert.throws(
    () =>
      publicEnvKeysForNamedStep(
        `${workflow}\n      - name: ${productionStepName}\n        env:\n          PUBLIC_DUPLICATE: value\n`,
        productionStepName,
      ),
    /exactly one workflow step/,
  );
  assert.throws(
    () => publicEnvKeysForNamedStep(workflow, "Missing step"),
    /exactly one workflow step/,
  );
});

test("duplicate PUBLIC keys fail closed", () => {
  const duplicate = workflow.replace(
    "          PUBLIC_REAL_TWO: value-two\n      - name: Later step",
    "          PUBLIC_REAL_ONE: duplicate\n      - name: Later step",
  );
  assert.throws(
    () => publicEnvForNamedStep(duplicate, productionStepName),
    /duplicate environment key PUBLIC_REAL_ONE/,
  );
});

test("missing or inline env maps fail closed", () => {
  assert.throws(
    () =>
      publicEnvKeysForNamedStep(
        `jobs:\n  build:\n    steps:\n      - name: ${productionStepName}\n        run: npm run check\n`,
        productionStepName,
      ),
    /block-style env map/,
  );
  assert.throws(
    () =>
      publicEnvKeysForNamedStep(
        `jobs:\n  build:\n    steps:\n      - name: ${productionStepName}\n        env: { PUBLIC_INLINE: value }\n`,
        productionStepName,
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
