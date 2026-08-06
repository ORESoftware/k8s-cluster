#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { validateSeaOrmPolicy } from "./seaorm-policy.mjs";

const serviceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const remoteRoot = path.resolve(serviceRoot, "../..");
const sharedRoot = path.join(remoteRoot, "libs");

function read(relativePath) {
  return readFileSync(path.join(serviceRoot, relativePath), "utf8");
}

function git(args, cwd) {
  return execFileSync("git", ["-C", cwd, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout: 10_000,
    maxBuffer: 64 * 1024,
    env: {
      PATH: process.env.PATH,
      HOME: process.env.HOME,
      LANG: process.env.LANG ?? "C.UTF-8",
      LC_ALL: process.env.LC_ALL ?? "C.UTF-8",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0",
    },
  }).trim();
}

function sharedCommit() {
  const commit = git(["rev-parse", "HEAD"], sharedRoot);
  const expected = process.env.EXPECTED_SHARED_COMMIT?.trim();
  if (expected && expected !== commit) {
    throw new Error(`shared checkout mismatch: expected ${expected}, received ${commit}`);
  }
  return commit;
}

const summary = validateSeaOrmPolicy({
  manifest: read("Cargo.toml"),
  coordination: read("src/coordination.rs"),
  main: read("src/main.rs"),
  sharedContract: JSON.parse(
    readFileSync(path.join(sharedRoot, "pg-defs", "rust-server-contract.json"), "utf8"),
  ),
  sharedCommit: sharedCommit(),
});

console.log(JSON.stringify(summary));
