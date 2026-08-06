#!/usr/bin/env node
// Compose the pure schema generator with package-owned adapters that require
// lifecycle or language-specific glue outside the generic emitter.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build as buildAdapters, loadTypes } from "./generate.mjs";
import { emitQuoteDart } from "./generate-quote-dart.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const genDir = path.join(root, "generated");
const wasmManifest = "rust-wasm/Cargo.toml";
const generatedLibBlock = `[lib]\ncrate-type = ["cdylib", "rlib"]\n`;
const packagedLibBlock = `[lib]\npath = "../../src/rust-wasm-entry.rs"\ncrate-type = ["cdylib", "rlib"]\n`;

export function build() {
  const files = buildAdapters();
  const manifest = files[wasmManifest];
  if (typeof manifest !== "string" || !manifest.includes(generatedLibBlock)) {
    throw new Error("generated Rust/WASM manifest no longer has the expected [lib] block");
  }
  if (manifest.includes("rust-wasm-entry.rs")) {
    throw new Error("generated Rust/WASM manifest already owns the package entrypoint");
  }
  files[wasmManifest] = manifest.replace(generatedLibBlock, packagedLibBlock);
  Object.assign(files, emitQuoteDart(loadTypes()));
  return files;
}

export { loadTypes };

function main() {
  const check = process.argv.includes("--check");
  let files;
  try {
    files = build();
  } catch (error) {
    console.error(`error: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(2);
  }

  let drift = 0;
  for (const [relativePath, content] of Object.entries(files)) {
    const absolutePath = path.join(genDir, relativePath);
    if (check) {
      const current = fs.existsSync(absolutePath)
        ? fs.readFileSync(absolutePath, "utf8")
        : null;
      if (current !== content) {
        console.error(`drift: ${relativePath}`);
        if (process.env.GITHUB_ACTIONS === "true") {
          console.error(`::error file=generated/${relativePath}::generated adapter is out of date; run npm run generate`);
        }
        drift += 1;
      }
    } else {
      fs.mkdirSync(path.dirname(absolutePath), { recursive: true });
      fs.writeFileSync(absolutePath, content);
      console.log(`wrote ${relativePath}`);
    }
  }

  if (check && drift > 0) {
    console.error(`${drift} file(s) out of date — run: npm run generate`);
    process.exit(1);
  }
  if (check) console.log("generated files up to date");
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) main();
