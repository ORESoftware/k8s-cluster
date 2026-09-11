import { spawnSync } from "node:child_process";

import { run as runLoadtest } from "./client.mjs";

function parseJsonArray(value, label) {
  if (!value) return [];
  let parsed;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error(`${label} is not valid JSON`);
  }
  if (!Array.isArray(parsed) || parsed.some((item) => typeof item !== "string")) {
    throw new Error(`${label} must be a JSON string array`);
  }
  return parsed;
}

export function run() {
  const args = process.argv.slice(2);
  const parsed = spawnSync("flags2env", ["__dd_gleamlang_ws_loadtest__", ...args], {
    cwd: process.cwd(),
    env: process.env,
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
  });

  if (parsed.error) {
    throw new Error(`flags2env failed to start: ${parsed.error.message}`);
  }
  if (parsed.status !== 0) {
    if (parsed.stdout) process.stdout.write(parsed.stdout);
    if (parsed.stderr) process.stderr.write(parsed.stderr);
    throw new Error(`flags2env exited with status ${parsed.status ?? "unknown"}`);
  }

  if (args.some((arg) => arg === "--help" || arg === "-h")) {
    process.stdout.write(parsed.stdout);
    return;
  }

  let resolved;
  try {
    resolved = JSON.parse(parsed.stdout);
  } catch {
    throw new Error("flags2env returned invalid JSON");
  }
  if (!resolved || typeof resolved !== "object" || Array.isArray(resolved)) {
    throw new Error("flags2env returned a non-object result");
  }

  const errors = parseJsonArray(resolved.FLAGS2ENV_PARSE_ERRORS, "FLAGS2ENV_PARSE_ERRORS");
  const unknown = parseJsonArray(
    resolved.FLAGS2ENV_UNKNOWN_OPTIONS,
    "FLAGS2ENV_UNKNOWN_OPTIONS",
  );
  if (errors.length > 0 || unknown.length > 0) {
    throw new Error(
      `flags2env rejected argv (${errors.length} parse error(s), ${unknown.length} unknown option(s))`,
    );
  }

  for (const [key, rawValue] of Object.entries(resolved)) {
    if (!/^[A-Z_][A-Z0-9_]*$/.test(key)) {
      throw new Error(`flags2env returned invalid environment key ${JSON.stringify(key)}`);
    }
    if (rawValue === null || typeof rawValue === "object") {
      throw new Error(`flags2env returned non-scalar value for ${key}`);
    }
    process.env[key] = String(rawValue);
  }

  // The historical JavaScript client still contains a compatibility parser.
  // Strip application argv before entering it so flags2env remains the only
  // active argv authority; all defaults/overrides are already materialized in
  // process.env above. The compatibility parser therefore cannot reinterpret
  // unknown or differently typed arguments.
  process.argv = process.argv.slice(0, 2);
  runLoadtest();
}
