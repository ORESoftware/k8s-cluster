// Architecture characterization for the Rust service refactor. These tests
// deliberately inspect source boundaries: behavior tests alone cannot detect
// business logic quietly accumulating back in binary entrypoints.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const root = path.resolve(import.meta.dirname, "..");

const services = {
  "fiducia-operations-control-plane": 120,
  "fiducia-ai-agent-control-plane": 15,
  "fiducia-auth.rs": 900,
  "fiducia-load-balance.rs": 625,
  "fiducia-lambda-service.rs": 125,
  "fiducia-ai-agent-bridge.rs": 10,
  "fiducia-messaging.rs": 175,
  "fiducia-customer.rs": 6500,
  "fiducia-brain.rs": 500,
  "fiducia-admin.rs": 4000,
  "fiducia-mcp-server.rs": 50,
  "fiducia-node-sidecar.rs": 925,
  "fiducia-memory.rs": 10,
  "fiducia-node.rs": 600,
  "fiducia-ai-agent-manager.rs": 125,
};

const libraryBackedServices = [
  "fiducia-operations-control-plane",
  "fiducia-ai-agent-control-plane",
  "fiducia-lambda-service.rs",
  "fiducia-ai-agent-bridge.rs",
  "fiducia-messaging.rs",
  "fiducia-mcp-server.rs",
  "fiducia-memory.rs",
  "fiducia-ai-agent-manager.rs",
];

const seaOrmServices = [
  "fiducia-operations-control-plane",
  "fiducia-ai-agent-control-plane",
  "fiducia-ai-agent-bridge.rs",
  "fiducia-messaging.rs",
  "fiducia-customer.rs",
  "fiducia-admin.rs",
  "fiducia-memory.rs",
];

// Customer still imports the removed ../fiducia-payments.rs path, so Cargo
// cannot resolve or validate its telemetry upgrade in this refactor pass.
const telemetryGuardServices = Object.keys(services).filter(
  (service) => service !== "fiducia-customer.rs",
);

function appPath(service, relative = "") {
  return path.join(root, "apps", service, relative);
}

function read(service, relative) {
  return readFileSync(appPath(service, relative), "utf8");
}

function sourceFiles(service) {
  return execFileSync("git", ["-C", appPath(service), "ls-files", "src"], {
    encoding: "utf8",
  })
    .trim()
    .split(/\r?\n/)
    .filter((file) => file.endsWith(".rs"));
}

function lineCount(text) {
  return text.split(/\r?\n/).length - (text.endsWith("\n") ? 1 : 0);
}

test("Rust services keep orchestration split across source modules", () => {
  for (const [service, mainBudget] of Object.entries(services)) {
    const files = sourceFiles(service);
    const main = read(service, "src/main.rs");
    const totalLines = files.reduce((total, file) => total + lineCount(read(service, file)), 0);
    const mainLines = lineCount(main);

    assert.ok(files.length >= 5, `${service} must keep at least four modules beside main.rs`);
    assert.ok(mainLines <= mainBudget, `${service}/src/main.rs has ${mainLines} lines; budget is ${mainBudget}`);
    assert.ok(
      mainLines / totalLines < 0.75,
      `${service}/src/main.rs owns ${(100 * mainLines / totalLines).toFixed(1)}% of Rust source`,
    );
  }
});

test("library-backed services expose modules through lib.rs and retain thin binaries", () => {
  for (const service of libraryBackedServices) {
    const library = read(service, "src/lib.rs");
    const main = read(service, "src/main.rs");
    const publicModules = library.match(/^pub mod [a-zA-Z0-9_]+;/gm) ?? [];

    assert.ok(publicModules.length >= 4, `${service}/src/lib.rs must expose extracted modules`);
    assert.ok(lineCount(main) <= 200, `${service}/src/main.rs must remain an orchestration-only adapter`);
    assert.doesNotMatch(
      main,
      /["'`]\s*(?:SELECT|INSERT|UPDATE|DELETE)\b/i,
      `${service} binary contains an inline SQL statement`,
    );
  }
});

test("extracted modules retain local behavior tests", () => {
  for (const service of Object.keys(services)) {
    const testedModules = sourceFiles(service).filter((file) => {
      if (file === "src/main.rs") {
        return false;
      }
      return /#\[cfg\(test\)\]/.test(read(service, file));
    });

    assert.ok(
      testedModules.length >= 3,
      `${service} has only ${testedModules.length} test-bearing extracted modules`,
    );
  }
});

test("database services use SeaORM without a direct SQLx dependency", () => {
  for (const service of Object.keys(services)) {
    const manifest = read(service, "Cargo.toml");
    assert.doesNotMatch(manifest, /^sqlx\s*=/m, `${service} must not depend directly on SQLx`);
  }

  for (const service of seaOrmServices) {
    assert.match(read(service, "Cargo.toml"), /^sea-orm\s*=/m, `${service} must use SeaORM`);
  }
});

test("telemetry consumers keep the OTLP guard alive for the process lifetime", () => {
  for (const service of telemetryGuardServices) {
    const manifest = read(service, "Cargo.toml");
    if (!/^fiducia-telemetry\s*=/m.test(manifest)) {
      continue;
    }

    assert.doesNotMatch(manifest, /fiducia-telemetry[^\n]*tag\s*=\s*"v0\.1\.0"/);
    const initializers = sourceFiles(service).flatMap((file) =>
      read(service, file)
        .split(/\r?\n/)
        .filter(
          (line) =>
            !line.trimStart().startsWith("//") && /fiducia_telemetry::init\s*\(/.test(line),
        )
        .map((line) => [file, line]),
    );
    assert.ok(initializers.length > 0, `${service} must initialize shared telemetry`);
    for (const [file, line] of initializers) {
      assert.match(
        line,
        /let\s+_telemetry\s*=\s*fiducia_telemetry::init/,
        `${service}/${file} drops its telemetry guard immediately`,
      );
    }
  }

  const telemetry = read("fiducia-telemetry.rs", "src/lib.rs");
  assert.match(telemetry, /collector.*Loki/is);
  assert.match(telemetry, /collector.*Prometheus/is);
  assert.match(telemetry, /build_tracer_provider/);
  assert.match(telemetry, /build_meter_provider/);
});

test.todo("customer telemetry guard after the removed fiducia-payments path is migrated");
