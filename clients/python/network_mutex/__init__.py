"""Python client for the ``dd-rust-network-mutex`` broker.

See ``../../PROTOCOL.md`` for the wire contract. Public surface mirrors the
Go/Dart/TS clients: connect, acquire / acquire_many / release, and the
try_acquire / try_acquire_many fail-fast helpers, plus the reader-writer helpers
acquire_read / acquire_write / release_read / release_write.
"""

from .client import (
    CompositeLockHandle,
    NetworkMutexClient,
    NetworkMutexError,
    SingleLockHandle,
)
from .protocol import RequestType, Response, ResponseType

__all__ = [
    "NetworkMutexClient",
    "NetworkMutexError",
    "SingleLockHandle",
    "CompositeLockHandle",
    "RequestType",
    "ResponseType",
    "Response",
]
