"""Fail-closed publisher for the five explicitly requested MCP repositories."""

from .model import (
    PublisherError,
    REPOSITORIES,
    RepositorySpec,
    bootstrap_files,
    validate_specs,
)
from .publisher import check, materialize_bootstrap, publish

__all__ = [
    "PublisherError",
    "REPOSITORIES",
    "RepositorySpec",
    "bootstrap_files",
    "check",
    "materialize_bootstrap",
    "publish",
    "validate_specs",
]
