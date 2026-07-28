#!/usr/bin/env python3

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

for forbidden in (
    "const metrics = {const metrics = {",
    "registerAjvFormats",
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };",
):
    if forbidden in server:
        raise SystemExit(f"migration repair did not remove {forbidden!r}")
SERVER.write_text(server, encoding="utf-8")

contract = CONTRACT.read_text(encoding="utf-8")
contract = replace_once(
    contract,
    """import { Type, type Static } from '@fastify/type-provider-typebox';
import type { FastifySchema } from 'fastify';
""",
    """import type { SwaggerOptions } from '@fastify/swagger';
import { Type, type Static } from '@fastify/type-provider-typebox';
import type { FastifySchema } from 'fastify';
""",
    "import SwaggerOptions",
)
contract = replace_once(
    contract,
    "export const OPENAPI_SWAGGER_OPTIONS = {\n",
    "export const OPENAPI_SWAGGER_OPTIONS: SwaggerOptions = {\n",
    "contextually type Swagger options",
)
CONTRACT.write_text(contract, encoding="utf-8")

package = json.loads(PACKAGE.read_text(encoding="utf-8"))
package["dependencies"].pop("ajv-formats", None)
package["dependencies"] = dict(sorted(package["dependencies"].items()))
PACKAGE.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")

print("repaired browser-test migration typing and marker boundaries")
