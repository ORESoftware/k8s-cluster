import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { validateRustServerConsumers } from "../../scripts/check-rust-server-consumers.mjs";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const inventory = JSON.parse(
  readFileSync(path.join(repositoryRoot, "pg-defs", "rust-server-consumers.json"), "utf8"),
);

test("stable generator CI validates the Rust-server consumer authority", () => {
  const summary = validateRustServerConsumers(inventory);
  assert.equal(summary.valid, true);
  assert.equal(
    summary.authorityCommit,
    "3c84cab532b27d328378f09fba5841f02644ae3b",
  );
  assert.deepEqual(summary.dpm, {
    repository: "declarative-migrations/declarative-postgres-migrate.rs",
    version: "0.3.2",
    linuxX8664Asset: "dpm-v0.3.2-x86_64-unknown-linux-gnu.tar.gz",
    linuxX8664Sha256: "4258755a946f6f3a49e33538889523e4736180624a186bddc90180994612d3aa",
    binary: "dpm",
  });
  assert.equal(summary.consumerCount, 3);
  assert.equal(summary.directSqlxTarget, 0);
  assert.equal(summary.rendererSpecificPersistence, false);
});
