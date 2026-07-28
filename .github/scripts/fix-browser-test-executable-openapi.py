#!/usr/bin/env python3
# Changes to this reviewed migration repair intentionally retrigger the cross-language SDK diagnostics.

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "remote/deployments/browser-test-server/src/server.ts"
CONTRACT = ROOT / "remote/deployments/browser-test-server/src/api-contract.ts"
PACKAGE = ROOT / "remote/deployments/browser-test-server/package.json"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


server = SERVER.read_text(encoding="utf-8")
repairs = {
    "const metrics = {const metrics = {": "const metrics = {",
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };\n": "export type { RunRequest, RunResult, Step };\n",
}
for old, new in repairs.items():
    count = server.count(old)
    if count > 1:
        raise SystemExit(f"ambiguous migration repair for {old!r}: {count} matches")
    if count == 1:
        server = server.replace(old, new, 1)

server = replace_once(
    server,
    "import swagger from '@fastify/swagger';\n",
    "import swagger, { type SwaggerOptions } from '@fastify/swagger';\n",
    "import SwaggerOptions at the plugin boundary",
)
server = replace_once(
    server,
    """import {
  registerAjvFormats,
  type TypeBoxTypeProvider,
  TypeBoxValidatorCompiler,
} from '@fastify/type-provider-typebox';
""",
    """import {
  Format,
  type TypeBoxTypeProvider,
  TypeBoxValidatorCompiler,
} from '@fastify/type-provider-typebox';
""",
    "replace nonexistent registerAjvFormats export",
)
server = replace_once(
    server,
    "registerAjvFormats();\n",
    """Format.Set('uri', (value) => {
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
});
Format.Set('date-time', (value) => !Number.isNaN(Date.parse(value)));
""",
    "register executable TypeBox formats",
)
server = replace_once(
    server,
    "await fastify.register(swagger, OPENAPI_SWAGGER_OPTIONS);\n",
    "await fastify.register(swagger, OPENAPI_SWAGGER_OPTIONS as unknown as SwaggerOptions);\n",
    "cast custom OpenAPI extensions only at registration",
)
server = replace_once(
    server,
    """fastify.setErrorHandler((error, request, reply) => {
  if (error.validation) {
    return reply.code(400).send({
      ok: false,
      error: 'request validation failed',
      details: error.validation,
    });
  }
  request.log.error({ err: error }, 'browser-test request failed');
  const statusCode =
    typeof error.statusCode === 'number' && error.statusCode >= 400
      ? error.statusCode
      : 500;
  return reply.code(statusCode).send({
    ok: false,
    error: statusCode >= 500 ? 'internal server error' : error.message,
  });
});
""",
    """fastify.setErrorHandler((error, request, reply) => {
  const typedError = error as Error & {
    statusCode?: number;
    validation?: unknown;
  };
  if (typedError.validation) {
    return reply.code(400).send({
      ok: false,
      error: 'request validation failed',
      details: typedError.validation,
    });
  }
  request.log.error({ err: error }, 'browser-test request failed');
  const statusCode =
    typeof typedError.statusCode === 'number' && typedError.statusCode >= 400
      ? typedError.statusCode
      : 500;
  return reply.code(statusCode).send({
    ok: false,
    error: statusCode >= 500 ? 'internal server error' : typedError.message,
  });
});
""",
    "narrow Fastify unknown errors safely",
)
server = replace_once(
    server,
    """function toolsDescriptor() {
  return {
    default: config.defaultTool,
""",
    """function toolsDescriptor() {
  return {
    defaultTool: config.defaultTool,
""",
    "avoid generated Rust Default enum collision in the tools response",
)
server = replace_once(
    server,
    """  void main().catch(async (error) => {
    fastify.log.error({ err: error }, 'dd-browser-test-server failed to start');
    process.exitCode = 1;
    await closeResources().catch(() => {});
  });
""",
    """  void main().catch(async (error) => {
    if (exportingOpenApi) {
      console.error(error instanceof Error ? (error.stack ?? error.message) : String(error));
    } else {
      fastify.log.error({ err: error }, 'dd-browser-test-server failed to start');
    }
    process.exitCode = 1;
    await closeResources().catch(() => {});
  });
""",
    "surface side-effect-free exporter failures on stderr",
)

for forbidden in (
    "const metrics = {const metrics = {",
    "registerAjvFormats",
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };",
    "    default: config.defaultTool,",
):
    if forbidden in server:
        raise SystemExit(f"migration repair did not remove {forbidden!r}")
SERVER.write_text(server, encoding="utf-8")

contract = CONTRACT.read_text(encoding="utf-8")
for schema_id, marker in (
    ("BrowserTool", "  $id: 'BrowserTool',\n"),
    ("BrowserScenarioStep", "    $id: 'BrowserScenarioStep',\n"),
    ("BrowserRunRequest", "    $id: 'BrowserRunRequest',\n"),
):
    contract = replace_once(
        contract,
        marker,
        "",
        f"keep inline {schema_id} schema anonymous",
    )
contract = replace_once(
    contract,
    "{ $id: 'BrowserRunResult', additionalProperties: false }",
    "{ additionalProperties: false }",
    "keep inline BrowserRunResult schema anonymous",
)
contract = replace_once(
    contract,
    "{ $id: 'BrowserApiError', additionalProperties: false }",
    "{ additionalProperties: false }",
    "keep inline BrowserApiError schema anonymous",
)
contract = replace_once(
    contract,
    """const ToolsDescriptorSchema = Type.Object(
  {
    default: ToolSchema,
""",
    """const ToolsDescriptorSchema = Type.Object(
  {
    defaultTool: ToolSchema,
""",
    "avoid generated Rust Default enum collision in the OpenAPI schema",
)
contract = replace_once(
    contract,
    """    ],
  },
};

function sortJson""",
    """    ],
  },
} as const;

function sortJson""",
    "preserve OpenAPI literal types without rejecting custom x-dd extensions",
)
if "    default: ToolSchema," in contract:
    raise SystemExit("tools response still exposes the generator-hostile default property")
CONTRACT.write_text(contract, encoding="utf-8")

package = json.loads(PACKAGE.read_text(encoding="utf-8"))
package["dependencies"].pop("ajv-formats", None)
package["dependencies"] = dict(sorted(package["dependencies"].items()))
PACKAGE.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")

print("repaired browser-test migration typing, schema identity, Rust model names, and marker boundaries")
