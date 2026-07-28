#!/usr/bin/env python3
"""Make executable-contract provenance text framework-neutral.

The fleet generator consumes native contracts from Rust/Axum, Node/Fastify,
Gleam/Mist, and future server stacks. Its generated operation notes must
therefore describe the strong route-registration invariant without claiming a
specific framework.
"""

from pathlib import Path


GENERATOR = Path("remote/tools/generate-api-docs.mjs")

FRAMEWORK_SPECIFIC = """        notes:
          document['x-dd-language'] === 'node'
            ? 'Executable OpenAPI contract collected from the same typed handler registration as the runtime Fastify router.'
            : 'Executable OpenAPI contract collected from the same typed handler registration as the runtime Axum router.',"""

FRAMEWORK_NEUTRAL = """        notes:
          'Executable OpenAPI contract collected from the same typed route registration used by the runtime dispatcher.',"""


def main() -> None:
    source = GENERATOR.read_text(encoding="utf-8")
    count = source.count(FRAMEWORK_SPECIFIC)
    if count != 1:
        raise SystemExit(
            "expected exactly one framework-specific executable-contract notes "
            f"block, found {count}"
        )
    updated = source.replace(FRAMEWORK_SPECIFIC, FRAMEWORK_NEUTRAL, 1)
    if "runtime Axum router" in updated or "runtime Fastify router" in updated:
        raise SystemExit("framework-specific executable-contract wording remains")
    GENERATOR.write_text(updated, encoding="utf-8")


if __name__ == "__main__":
    main()
