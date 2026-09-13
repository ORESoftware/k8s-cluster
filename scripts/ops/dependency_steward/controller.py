"""Portfolio orchestration assembled from focused controller mixins."""

from __future__ import annotations

from .controller_repository import RepositoryControllerBase
from .controller_dependency import DependencyProcessingMixin
from .controller_artifacts import ArtifactWritingMixin


class StewardController(
    DependencyProcessingMixin, ArtifactWritingMixin, RepositoryControllerBase
):
    """Concrete controller with repository, dependency, and artifact behavior."""

