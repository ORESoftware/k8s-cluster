#!/usr/bin/env python3
"""Deterministic seed templates for the bounded new-organization repository fleet."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from .cli_sync import cli_files, sync_files
from .clients import clients_files
from .governance import governance_files, interfaces_files
from .integration import e2e_files, mcp_files
from .operations import infra_files, monorepo_files
from .server import server_files

SEED_BUILDERS = {
    "governance": governance_files,
    "interfaces": interfaces_files,
    "clients": clients_files,
    "cli": cli_files,
    "sync": sync_files,
    "server": server_files,
    "e2e": e2e_files,
    "mcp": mcp_files,
    "infra": infra_files,
    "monorepo": monorepo_files,
}


def files_for_repository(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    seed = str(repo["seed"])
    try:
        builder = SEED_BUILDERS[seed]
    except KeyError as error:
        raise ValueError(f"unsupported seed type {seed!r}") from error
    files = builder(org, repo)
    if not files or any(not path or path.startswith("/") or ".." in path.split("/") for path in files):
        raise ValueError(f"invalid seed file map for {org['owner']}/{repo['name']}")
    if any("PLACEHOLDER" in content or "TODO" in content for content in files.values()):
        raise ValueError(f"placeholder content in {org['owner']}/{repo['name']}")
    return dict(sorted(files.items()))


__all__ = ["SEED_BUILDERS", "files_for_repository"]
