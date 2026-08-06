const result = document.querySelector("#result");
const originalFetch = globalThis.fetch.bind(globalThis);
const fetches = [];

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEqual(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function progress(stage) {
  result.dataset.stage = stage;
  result.textContent = `running: ${stage}`;
}

async function sameOriginFetch(input, init) {
  const url = new URL(input instanceof Request ? input.url : input, location.href);
  assertEqual(url.origin, location.origin, `Fetch escaped the browser-contract origin (${url.href})`);
  fetches.push(url.href);
  return originalFetch(input, init);
}

async function run() {
  globalThis.fetch = sameOriginFetch;

  progress("import generated bindings");
  const moduleUrl = new URL(
    "../../generated/rust-wasm/pkg/canonical_interfaces_wasm.js",
    import.meta.url,
  );
  assertEqual(moduleUrl.origin, location.origin, "generated module origin");
  const bindings = await import(moduleUrl.href);
  assert(typeof bindings.default === "function", "generated package must export its initializer");

  progress("fetch reviewed WebAssembly bytes");
  const wasmUrl = new URL(
    "../../generated/rust-wasm/pkg/canonical_interfaces_wasm_bg.wasm",
    import.meta.url,
  );
  const wasmResponse = await fetch(wasmUrl);
  assert(wasmResponse.ok, `generated WebAssembly returned HTTP ${wasmResponse.status}`);
  const wasmBytes = await wasmResponse.arrayBuffer();
  assert(wasmBytes.byteLength > 0, "generated WebAssembly must not be empty");

  progress("initialize reviewed WebAssembly bytes");
  const initialized = await bindings.default(wasmBytes);
  assert(initialized && typeof initialized === "object", "generated WebAssembly did not initialize");

  progress("inspect runtime export boundary");
  const runtimeExports = Object.keys(bindings).sort();
  const forbiddenRuntimeName = /(fetch|request|submit|mutat|write|delete|probe|execute|storage|cookie|websocket|eventsource|xmlhttp|beacon)/i;
  assert(
    runtimeExports.every((name) => !forbiddenRuntimeName.test(name)),
    `declaration-only package exposed a forbidden runtime API: ${runtimeExports.join(", ")}`,
  );

  progress("fetch and inspect generated declarations");
  const declarationUrl = new URL(
    "../../generated/rust-wasm/pkg/canonical_interfaces_wasm.d.ts",
    import.meta.url,
  );
  const declarationResponse = await fetch(declarationUrl);
  assert(declarationResponse.ok, `generated declarations returned HTTP ${declarationResponse.status}`);
  const declarations = await declarationResponse.text();
  for (const requiredType of ["HealthStatus", "MutationRequest", "AuditEngagement"]) {
    assert(
      declarations.includes(requiredType),
      `generated declarations omitted representative type ${requiredType}`,
    );
  }
  assert(
    !/:[\s]*(?:bigint|Map<)/.test(declarations),
    "generated declarations leaked bigint or Map into the browser contract",
  );
  assert(
    !/:[\s]*Value(?:[;[\]|]|$)/m.test(declarations),
    "generated declarations contain an unresolved Value type",
  );

  progress("verify same-origin resource closure");
  const resourceUrls = performance
    .getEntriesByType("resource")
    .map((entry) => entry.name)
    .filter((name) => /^https?:/.test(name));
  for (const resourceUrl of resourceUrls) {
    assertEqual(new URL(resourceUrl).origin, location.origin, `resource escaped origin (${resourceUrl})`);
  }
  assert(fetches.includes(wasmUrl.href), "browser contract did not fetch the reviewed WebAssembly");
  assert(fetches.includes(declarationUrl.href), "browser contract did not fetch the reviewed declarations");

  result.dataset.status = "pass";
  result.dataset.stage = "complete";
  result.textContent = `pass: initialized ${wasmBytes.byteLength} bytes; runtime exports ${runtimeExports.join(", ") || "none"}`;
}

try {
  await run();
} catch (error) {
  result.dataset.status = "fail";
  result.textContent = error instanceof Error ? error.stack ?? error.message : String(error);
  console.error(error);
} finally {
  globalThis.fetch = originalFetch;
}
