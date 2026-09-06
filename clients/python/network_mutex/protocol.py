"""Wire protocol for ``dd-rust-network-mutex`` (Python mirror of ``src/protocol.rs``).

The broker speaks newline-delimited JSON with a camelCase ``type`` discriminator.
We mirror the Rust ``Request`` / ``Response`` tagged enums using :class:`enum.Enum`
discriminators plus typed builder functions, so a typo is a ``NameError`` at
import time rather than a silently-misrouted magic string (the failure mode the
upstream Node ``live-mutex`` library has with ``if (data.type === '...')``).

See ``../../PROTOCOL.md`` for the single source of truth.
"""

from __future__ import annotations

import enum
import json
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


class RequestType(str, enum.Enum):
    """Discriminator for client -> broker frames."""

    VERSION = "version"
    AUTH = "auth"
    LOCK = "lock"
    UNLOCK = "unlock"
    REGISTER_READ = "registerRead"
    REGISTER_WRITE = "registerWrite"
    END_READ = "endRead"
    END_WRITE = "endWrite"
    LOCK_INFO = "lockInfo"
    LS = "ls"
    HEARTBEAT = "heartbeat"


class ResponseType(str, enum.Enum):
    """Discriminator for broker -> client frames."""

    VERSION = "version"
    AUTH = "auth"
    LOCK = "lock"
    COMPOSITE_LOCK = "compositeLock"
    UNLOCK = "unlock"
    REGISTER_READ_RESULT = "registerReadResult"
    REGISTER_WRITE_RESULT = "registerWriteResult"
    END_READ_RESULT = "endReadResult"
    END_WRITE_RESULT = "endWriteResult"
    LOCK_INFO = "lockInfo"
    LS_RESULT = "lsResult"
    REELECTION = "reelection"
    ERROR = "error"
    OK = "ok"

    @classmethod
    def parse(cls, raw: str) -> "ResponseType":
        try:
            return cls(raw)
        except ValueError as exc:  # pragma: no cover - defensive
            raise ValueError(f"unknown response type from broker: {raw!r}") from exc


MAX_COMPOSITE_KEYS = 5
PROTOCOL_VERSION = "0.1.0"


def _frame(payload: Dict[str, Any]) -> bytes:
    """Serialize a request dict to one newline-delimited JSON frame.

    ``None`` values are stripped so the broker sees the same shape the Rust
    client produces (``skip_serializing_if = "Option::is_none"``).
    """

    compact = {k: v for k, v in payload.items() if v is not None}
    return (json.dumps(compact, separators=(",", ":")) + "\n").encode("utf-8")


def version_request(uuid: str, value: str = PROTOCOL_VERSION) -> bytes:
    return _frame({"type": RequestType.VERSION.value, "uuid": uuid, "value": value})


def auth_request(uuid: str, token: str) -> bytes:
    return _frame({"type": RequestType.AUTH.value, "uuid": uuid, "token": token})


def lock_request(
    uuid: str,
    *,
    key: Optional[str] = None,
    keys: Optional[List[str]] = None,
    pid: Optional[int] = None,
    ttl_ms: Optional[int] = None,
    max_holders: Optional[int] = None,
    force: bool = False,
    keep_locks_after_death: bool = False,
    wait: Optional[bool] = None,
) -> bytes:
    if (key is None) == (keys is None):
        raise ValueError("lock_request: pass exactly one of key= or keys=")
    if keys is not None and not (1 <= len(keys) <= MAX_COMPOSITE_KEYS):
        raise ValueError(
            f"composite key count must be 1..={MAX_COMPOSITE_KEYS}, got {len(keys)}"
        )
    return _frame(
        {
            "type": RequestType.LOCK.value,
            "uuid": uuid,
            "key": key,
            "keys": keys,
            "pid": pid,
            "ttl": ttl_ms,
            "max": max_holders,
            "force": force or None,
            "keepLocksAfterDeath": keep_locks_after_death or None,
            "wait": wait,
        }
    )


def unlock_request(
    uuid: str,
    *,
    key: Optional[str] = None,
    keys: Optional[List[str]] = None,
    lock_uuid: Optional[str] = None,
    force: bool = False,
) -> bytes:
    return _frame(
        {
            "type": RequestType.UNLOCK.value,
            "uuid": uuid,
            "key": key,
            "keys": keys,
            "lockUuid": lock_uuid,
            "force": force or None,
        }
    )


def register_read_request(uuid: str, key: str) -> bytes:
    return _frame({"type": RequestType.REGISTER_READ.value, "uuid": uuid, "key": key})


def register_write_request(uuid: str, key: str) -> bytes:
    return _frame({"type": RequestType.REGISTER_WRITE.value, "uuid": uuid, "key": key})


def end_read_request(uuid: str, key: str) -> bytes:
    return _frame({"type": RequestType.END_READ.value, "uuid": uuid, "key": key})


def end_write_request(uuid: str, key: str) -> bytes:
    return _frame({"type": RequestType.END_WRITE.value, "uuid": uuid, "key": key})


def lock_info_request(uuid: str, key: str) -> bytes:
    return _frame({"type": RequestType.LOCK_INFO.value, "uuid": uuid, "key": key})


def ls_request(uuid: str) -> bytes:
    return _frame({"type": RequestType.LS.value, "uuid": uuid})


def heartbeat_request(uuid: str) -> bytes:
    return _frame({"type": RequestType.HEARTBEAT.value, "uuid": uuid})


@dataclass
class Response:
    """Parsed broker frame. Optional fields are ``None`` when absent so callers
    can tell ``false``/``0`` apart from "not present" (mirrors the Go client's
    pointer fields)."""

    type: ResponseType
    uuid: str
    raw: Dict[str, Any] = field(repr=False, default_factory=dict)

    broker_version: Optional[str] = None
    ok: Optional[bool] = None
    error: Optional[str] = None

    key: Optional[str] = None
    keys: Optional[List[str]] = None
    acquired: Optional[bool] = None
    unlocked: Optional[bool] = None

    lock_request_count: Optional[int] = None
    lock_uuid: Optional[str] = None
    fencing_token: Optional[int] = None
    fencing_tokens: Optional[Dict[str, int]] = None
    readers_count: Optional[int] = None
    writer_flag: Optional[bool] = None
    granted: Optional[bool] = None
    is_locked: Optional[bool] = None
    lockholder_uuids: Optional[List[str]] = None

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Response":
        return cls(
            type=ResponseType.parse(data["type"]),
            uuid=data.get("uuid", ""),
            raw=data,
            broker_version=data.get("brokerVersion"),
            ok=data.get("ok"),
            error=data.get("error"),
            key=data.get("key"),
            keys=data.get("keys"),
            acquired=data.get("acquired"),
            unlocked=data.get("unlocked"),
            lock_request_count=data.get("lockRequestCount"),
            lock_uuid=data.get("lockUuid"),
            fencing_token=data.get("fencingToken"),
            fencing_tokens=data.get("fencingTokens"),
            readers_count=data.get("readersCount"),
            writer_flag=data.get("writerFlag"),
            granted=data.get("granted"),
            is_locked=data.get("isLocked"),
            lockholder_uuids=data.get("lockholderUuids"),
        )

    @classmethod
    def decode(cls, line: bytes) -> "Response":
        return cls.from_dict(json.loads(line))
