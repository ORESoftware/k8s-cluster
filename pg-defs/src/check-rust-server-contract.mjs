#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { validateRustServerContract } from "./rust-server-contract.mjs";

const pgDefsRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = path.resolve(pgDefsRoot, "..");

function outputPath(value) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    path.isAbsolute(value) ||
    value.split(/[\\/]/u).includes("..") ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    throw new Error("--report must be a safe repository-relative path");
  }
  const resolved = path.resolve(repositoryRoot, value);
  if (!resolved.startsWith(`${repositoryRoot}${path.sep}`)) {
    throw new Error("--report must remain inside the repository");
  }
  return resolved;
}

const contract = JSON.parse(
  await readFile(path.join(pgDefsRoot, "rust-server-contract.json"), "utf8"),
);
const files = {
  schemaSql: await readFile(path.join(pgDefsRoot, "schema", "schema.sql"), "utf8"),
  generatedManifest: await readFile(
    path.join(pgDefsRoot, "generated", "rust", "sea-orm", "Cargo.toml"),
    "utf8",
  ),
  generatedSource: await readFile(
    path.join(pgDefsRoot, "generated", "rust", "sea-orm", "src", "lib.rs"),
    "utf8",
  ),
  dpmScript: await readFile(path.join(pgDefsRoot, "scripts", "dpm.sh"), "utf8"),
};
const summary = validateRustServerContract(contract, files);
const report = {
  contract: "pg-defs/rust-server-contract.json",
  ...summary,
};

console.log(
  `Validated Rust server database contract: ${summary.applicationOrm}, ` +
    `${summary.generatedCrate}, ${summary.migrationTool} >= ${summary.minimumDpmVersion}, ` +
    `${summary.verificationGates} gates.`,
);

const reportIndex = process.argv.indexOf("--report");
if (reportIndex !== -1) {
  const requested = process.argv[reportIndex + 1];
  if (!requested) throw new Error("--report requires a path");
  const target = outputPath(requested);
  await writeFile(target, `${JSON.stringify(report, null, 2)}\n`, {
    flag: "wx",
    mode: 0o600,
  });
}
