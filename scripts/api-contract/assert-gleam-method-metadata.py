#!/usr/bin/env python3
"""Assert method-specific metadata survives native-to-fleet projection."""

import json
from pathlib import Path


GENERATED = Path("remote/deployments/gleamlang-presence-server/generated")
PATH = "/conv/{conv_id}/members/{user_id}"
EXPECTED = {
    "post": ("addConversationMember", "Add a conversation member"),
    "delete": ("removeConversationMember", "Remove a conversation member"),
}


def main() -> None:
    native = json.loads((GENERATED / "openapi.json").read_text(encoding="utf-8"))
    projected = json.loads(
        (GENERATED / "api-docs.internal.json").read_text(encoding="utf-8")
    )

    for method, (operation_id, summary) in EXPECTED.items():
        native_operation = native["paths"][PATH][method]
        projected_operation = projected["paths"][PATH][method]
        assert native_operation["operationId"] == operation_id
        assert native_operation["summary"] == summary
        assert projected_operation["summary"] == summary
        assert projected_operation["x-dd-handlers"] == [operation_id]
        assert projected_operation["x-dd-auth"] == native_operation["x-dd-auth"]
        assert projected_operation.get("security") == native_operation.get("security")

    assert (
        projected["paths"][PATH]["post"]["summary"]
        != projected["paths"][PATH]["delete"]["summary"]
    )


if __name__ == "__main__":
    main()
