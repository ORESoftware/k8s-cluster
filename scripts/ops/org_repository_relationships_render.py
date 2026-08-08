"""Human-readable rendering and managed README integration for relationship data."""

from __future__ import annotations

from typing import Any

from org_repository_relationships_model import JSON_PATH, MD_PATH, SCHEMA_PATH

BEGIN_MARKER = "<!-- BEGIN MANAGED REPOSITORY RELATIONSHIPS v1 -->"
END_MARKER = "<!-- END MANAGED REPOSITORY RELATIONSHIPS v1 -->"


def render_markdown(data: dict[str, Any]) -> str:
    lines = [
        f"# `{data['organization']}` repository relationships",
        "",
        "Generated from reviewed policy and the current **public** repository inventory.",
        "",
        f"- Public repositories declared: **{len(data['repositories'])}**",
        f"- Private repository names withheld: **{data['privacy']['private_repository_count']}**",
        f"- Relationship edges: **{len(data['relationships'])}**",
        "",
        "## Repository roles",
        "",
        "| Repository | Role | Lifecycle |",
        "|---|---|---|",
    ]
    lines.extend(
        f"| [`{repository['name']}`](https://github.com/{repository['full_name']}) | "
        f"`{repository['role']}` | `{repository['lifecycle']}` |"
        for repository in data["repositories"]
    )
    lines.extend(
        [
            "",
            "## Declared edges",
            "",
            "| From | Relationship | To | Status/basis |",
            "|---|---|---|---|",
        ]
    )
    lines.extend(
        f"| `{item['from']}` | `{item['kind']}` | `{item['to']}` | "
        f"`{item['status']}` / `{item['source_basis']}`: {item['rationale']} |"
        for item in data["relationships"]
    )
    lines.extend(
        [
            "",
            "## Composition, service, and observability contract",
            "",
            (
                "Git submodules compose editable source; Zed packages resolve "
                "packages/artifacts; dual-managed commits must match. Production "
                "deploys immutable image digests, not runtime source builds. "
                "Cross-service access uses APIs/SDKs/events rather than another "
                "service database. MCP uses the product API/SDK. Services emit "
                "OpenTelemetry traces, bounded metrics, and correlated structured logs."
            ),
            "",
            "## Privacy boundary",
            "",
            (
                "This public registry deliberately omits private repository names "
                "and edges; the count above makes the boundary explicit."
            ),
            "",
        ]
    )
    return "\n".join(lines)


def relationship_readme_block(organization: str) -> str:
    return f"""## Repository relationship registry

`{organization}` declares repository roles, dependency edges, cross-organization capabilities, deployment ownership, and the git-submodule/Zed-package contract:

- [Human-readable map]({MD_PATH})
- [Machine-readable manifest]({JSON_PATH})
- [JSON Schema]({SCHEMA_PATH})

The public registry withholds private repository names and edges.
"""


def merge_managed_block(existing: str | None, body: str) -> str:
    block = f"{BEGIN_MARKER}\n{body.rstrip()}\n{END_MARKER}"
    if not existing:
        return block + "\n"
    start = existing.find(BEGIN_MARKER)
    end = existing.find(END_MARKER)
    if start >= 0 or end >= 0:
        if start < 0 or end < 0 or end < start:
            raise ValueError("malformed relationship markers")
        prefix = existing[:start].rstrip()
        suffix = existing[end + len(END_MARKER) :]
        return (prefix + "\n\n" + block + suffix).rstrip() + "\n"
    return existing.rstrip() + "\n\n" + block + "\n"
