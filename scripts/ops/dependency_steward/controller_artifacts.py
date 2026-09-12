"""Dependency graph, run report, and credential-free publish-plan artifacts."""

from __future__ import annotations

from .models import *
from .providers import *
from .runtime import *
from .scanners import *
from .operations import *

class ArtifactWritingMixin:
    def write_artifacts(self) -> None:
        self.artifacts.mkdir(parents=True, exist_ok=True)
        edges = sorted(self.edges, key=lambda item: (item.repository, item.key, item.kind))
        summaries = sorted(self.summaries, key=lambda item: item.repository)
        graph = {
            "contract": JOB_MARKER,
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "nodes": sorted(
                set(
                    [item.repository for item in edges]
                    + [canonical_github_repo(item.source_url) or item.name for item in edges]
                )
            ),
            "edges": [item.graph_dict() for item in edges],
        }
        tickets = sorted(
            self.ticket_intents, key=lambda item: (item.repository, item.category, item.marker)
        )
        pulls = sorted(
            self.pull_intents,
            key=lambda item: (item.repository, item.dependency.get("key", ""), item.target_version),
        )
        publish_plan = {
            "contract": JOB_MARKER,
            "phase": "analyze",
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "organizations": len(self.portfolio),
            "tickets": [dataclasses.asdict(item) for item in tickets],
            "pull_requests": [dataclasses.asdict(item) for item in pulls],
        }
        report = {
            "contract": JOB_MARKER,
            "apply": self.apply,
            "phase": "analyze" if not self.apply else "all",
            "organizations": len(self.portfolio),
            "repositories": len(summaries),
            "edges": len(edges),
            "pull_requests": sum(len(item.prs) for item in summaries),
            "linear_tickets": sum(len(item.tickets) for item in summaries),
            "closed_pull_requests": sum(len(item.closed_prs) for item in summaries),
            "provider_errors": self.provider_errors,
            "results": [dataclasses.asdict(item) for item in summaries],
        }
        (self.artifacts / "dependency-graph.json").write_text(
            json.dumps(graph, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        (self.artifacts / "dependency-graph.dot").write_text(
            graph_to_dot(edges), encoding="utf-8"
        )
        (self.artifacts / "publish-plan.json").write_text(
            json.dumps(publish_plan, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        (self.artifacts / "report.json").write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        lines = [
            "# Nightly dependency steward",
            "",
            f"- Organizations: **{len(self.portfolio)}**",
            f"- Repositories inventoried: **{len(summaries)}**",
            f"- Dependency edges: **{len(edges)}**",
            f"- PRs opened or updated: **{report['pull_requests']}**",
            f"- Linear tickets created or reused: **{report['linear_tickets']}**",
            f"- Obsolete managed PRs closed: **{report['closed_pull_requests']}**",
            "",
            "| Repository | Status | Edges | PRs | Tickets | Errors |",
            "|---|---|---:|---:|---:|---:|",
        ]
        for item in summaries:
            lines.append(
                f"| `{item.repository}` | {item.status} | {item.edges} | "
                f"{len(item.prs)} | {len(item.tickets)} | {len(item.errors)} |"
            )
        if self.provider_errors:
            lines.extend(["", "## Provider errors", ""])
            lines.extend(f"- {redact(error)}" for error in self.provider_errors)
        (self.artifacts / "report.md").write_text(
            "\n".join(lines) + "\n", encoding="utf-8"
        )
