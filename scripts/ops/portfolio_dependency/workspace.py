"""Compatibility facade for checkout, discovery, profile selection, and mutation."""

from .checkout import GitWorkspace
from .discovery import (
    detect_profile,
    discover_edges,
    discover_flake_lock,
    discover_gitmodules,
    discover_zpkg,
    discover_zpkg_lock_only,
    enrich_zpkg_from_lock,
    gitlink_sha,
)
from .mutation import (
    apply_candidate,
    regenerate_zpkg_lock,
    replace_zpkg_git_pin,
    replace_zpkg_version,
    update_flake_lock,
)

__all__ = [
    "GitWorkspace",
    "apply_candidate",
    "detect_profile",
    "discover_edges",
    "discover_flake_lock",
    "discover_gitmodules",
    "discover_zpkg",
    "discover_zpkg_lock_only",
    "enrich_zpkg_from_lock",
    "gitlink_sha",
    "regenerate_zpkg_lock",
    "replace_zpkg_git_pin",
    "replace_zpkg_version",
    "update_flake_lock",
]
