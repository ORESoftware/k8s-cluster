"""Multiplexed TCP/UDS client for ``dd-rust-network-mutex``.

One :class:`NetworkMutexClient` is safe for use from many threads: writes are
serialized under a lock and a single background reader thread fans responses
out to per-request queues keyed by the correlation ``uuid`` (mirrors the Go
client's ``map[string]chan Response``). A single request may receive more than
one frame (e.g. a queued notice followed by the actual grant), so callers that
block until granted drain the queue until the terminal frame arrives.
"""

from __future__ import annotations

import queue
import socket
import threading
import uuid as uuidlib
from dataclasses import dataclass
from typing import Dict, List, Optional, Union

from . import protocol
from .protocol import Response, ResponseType


class NetworkMutexError(RuntimeError):
    """Raised when the broker returns an error frame or an unexpected reply."""


@dataclass
class SingleLockHandle:
    key: str
    lock_uuid: str
    fencing_token: int


@dataclass
class CompositeLockHandle:
    keys: List[str]
    lock_uuid: str
    fencing_tokens: Dict[str, int]


Handle = Union[SingleLockHandle, CompositeLockHandle]


def _new_uuid() -> str:
    return str(uuidlib.uuid4())


class NetworkMutexClient:
    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock
        self._wlock = threading.Lock()
        self._inflight: Dict[str, "queue.Queue[Response]"] = {}
        self._inflight_lock = threading.Lock()
        self._closed = threading.Event()
        self._read_err: Optional[BaseException] = None
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    # -- construction ----------------------------------------------------

    @classmethod
    def connect(
        cls,
        host: str = "127.0.0.1",
        port: int = 6970,
        *,
        token: Optional[str] = None,
        connect_timeout: float = 5.0,
        unix_path: Optional[str] = None,
    ) -> "NetworkMutexClient":
        if unix_path is not None:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.settimeout(connect_timeout)
            sock.connect(unix_path)
        else:
            sock = socket.create_connection((host, port), timeout=connect_timeout)
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        sock.settimeout(None)
        client = cls(sock)
        if token:
            auth_uuid = _new_uuid()
            resp = client._roundtrip(protocol.auth_request(auth_uuid, token), auth_uuid)
            if resp.type is not ResponseType.AUTH or not resp.ok:
                client.close()
                raise NetworkMutexError(f"auth rejected: {resp.error}")
        return client

    # -- exclusive / composite locks ------------------------------------

    def acquire(
        self,
        key: str,
        *,
        ttl_ms: int = 0,
        max_holders: Optional[int] = None,
        timeout: Optional[float] = None,
        wait: bool = True,
    ) -> SingleLockHandle:
        if not wait:
            handle = self.try_acquire(key, ttl_ms=ttl_ms, max_holders=max_holders, timeout=timeout)
            if handle is None:
                raise NetworkMutexError(f"lock({key}) not acquired")
            return handle
        uuid = _new_uuid()
        resp = self._roundtrip_grant(
            protocol.lock_request(
                uuid,
                key=key,
                ttl_ms=ttl_ms or None,
                max_holders=max_holders,
                wait=True,
            ),
            uuid,
            timeout=timeout,
        )
        if resp.type is not ResponseType.LOCK or not resp.acquired or not resp.lock_uuid:
            raise NetworkMutexError(f"lock({key}) failed: {resp.raw}")
        return SingleLockHandle(key=key, lock_uuid=resp.lock_uuid, fencing_token=resp.fencing_token or 0)

    def try_acquire(
        self,
        key: str,
        *,
        ttl_ms: int = 0,
        max_holders: Optional[int] = None,
        timeout: Optional[float] = None,
    ) -> Optional[SingleLockHandle]:
        uuid = _new_uuid()
        resp = self._roundtrip(
            protocol.lock_request(
                uuid,
                key=key,
                ttl_ms=ttl_ms or None,
                max_holders=max_holders,
                wait=False,
            ),
            uuid,
            timeout=timeout,
        )
        if resp.type is ResponseType.ERROR:
            raise NetworkMutexError(f"try_acquire({key}) error: {resp.error}")
        if resp.type is not ResponseType.LOCK:
            raise NetworkMutexError(f"try_acquire({key}) unexpected: {resp.raw}")
        if not resp.acquired or not resp.lock_uuid:
            return None
        return SingleLockHandle(key=key, lock_uuid=resp.lock_uuid, fencing_token=resp.fencing_token or 0)

    def acquire_many(
        self,
        keys: List[str],
        *,
        ttl_ms: int = 0,
        timeout: Optional[float] = None,
        wait: bool = True,
    ) -> CompositeLockHandle:
        if not wait:
            handle = self.try_acquire_many(keys, ttl_ms=ttl_ms, timeout=timeout)
            if handle is None:
                raise NetworkMutexError(f"acquire_many({keys}) not acquired")
            return handle
        uuid = _new_uuid()
        resp = self._roundtrip_grant(
            protocol.lock_request(uuid, keys=keys, ttl_ms=ttl_ms or None, wait=True),
            uuid,
            timeout=timeout,
        )
        if resp.type is not ResponseType.COMPOSITE_LOCK or not resp.acquired or not resp.lock_uuid:
            raise NetworkMutexError(f"acquire_many({keys}) failed: {resp.raw}")
        return CompositeLockHandle(
            keys=keys, lock_uuid=resp.lock_uuid, fencing_tokens=resp.fencing_tokens or {}
        )

    def try_acquire_many(
        self,
        keys: List[str],
        *,
        ttl_ms: int = 0,
        timeout: Optional[float] = None,
    ) -> Optional[CompositeLockHandle]:
        uuid = _new_uuid()
        resp = self._roundtrip(
            protocol.lock_request(uuid, keys=keys, ttl_ms=ttl_ms or None, wait=False),
            uuid,
            timeout=timeout,
        )
        if resp.type is ResponseType.ERROR:
            raise NetworkMutexError(f"try_acquire_many({keys}) error: {resp.error}")
        if resp.type is not ResponseType.COMPOSITE_LOCK:
            raise NetworkMutexError(f"try_acquire_many({keys}) unexpected: {resp.raw}")
        if not resp.acquired or not resp.lock_uuid:
            return None
        return CompositeLockHandle(
            keys=keys, lock_uuid=resp.lock_uuid, fencing_tokens=resp.fencing_tokens or {}
        )

    def release(self, handle: Handle, *, timeout: Optional[float] = None) -> None:
        uuid = _new_uuid()
        if isinstance(handle, SingleLockHandle):
            frame = protocol.unlock_request(uuid, key=handle.key, lock_uuid=handle.lock_uuid)
        elif isinstance(handle, CompositeLockHandle):
            frame = protocol.unlock_request(uuid, keys=handle.keys, lock_uuid=handle.lock_uuid)
        else:  # pragma: no cover - defensive
            raise TypeError(f"release: unknown handle type {type(handle)!r}")
        resp = self._roundtrip(frame, uuid, timeout=timeout)
        if resp.type is not ResponseType.UNLOCK or not resp.unlocked:
            raise NetworkMutexError(f"unlock failed: {resp.raw}")

    def force_unlock(self, key: str, *, timeout: Optional[float] = None) -> None:
        uuid = _new_uuid()
        resp = self._roundtrip(
            protocol.unlock_request(uuid, key=key, force=True), uuid, timeout=timeout
        )
        if resp.type is ResponseType.ERROR:
            raise NetworkMutexError(f"force unlock failed: {resp.error}")

    # -- reader / writer locks ------------------------------------------

    def acquire_read(self, key: str, *, timeout: Optional[float] = None) -> SingleLockHandle:
        uuid = _new_uuid()
        resp = self._roundtrip_until_granted(
            protocol.register_read_request(uuid, key), uuid, timeout=timeout
        )
        return SingleLockHandle(key=key, lock_uuid=resp.lock_uuid or "", fencing_token=resp.fencing_token or 0)

    def acquire_write(self, key: str, *, timeout: Optional[float] = None) -> SingleLockHandle:
        uuid = _new_uuid()
        resp = self._roundtrip_until_granted(
            protocol.register_write_request(uuid, key), uuid, timeout=timeout
        )
        return SingleLockHandle(key=key, lock_uuid=resp.lock_uuid or "", fencing_token=resp.fencing_token or 0)

    def release_read(self, key: str, *, timeout: Optional[float] = None) -> None:
        uuid = _new_uuid()
        self._roundtrip(protocol.end_read_request(uuid, key), uuid, timeout=timeout)

    def release_write(self, key: str, *, timeout: Optional[float] = None) -> None:
        uuid = _new_uuid()
        self._roundtrip(protocol.end_write_request(uuid, key), uuid, timeout=timeout)

    # -- introspection ---------------------------------------------------

    def lock_info(self, key: str, *, timeout: Optional[float] = None) -> Response:
        uuid = _new_uuid()
        return self._roundtrip(protocol.lock_info_request(uuid, key), uuid, timeout=timeout)

    def ls(self, *, timeout: Optional[float] = None) -> List[str]:
        uuid = _new_uuid()
        resp = self._roundtrip(protocol.ls_request(uuid), uuid, timeout=timeout)
        return resp.keys or []

    # -- lifecycle -------------------------------------------------------

    def close(self) -> None:
        if self._closed.is_set():
            return
        self._closed.set()
        try:
            self._sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            self._sock.close()
        except OSError:
            pass
        with self._inflight_lock:
            for q in self._inflight.values():
                q.put(None)  # type: ignore[arg-type]
            self._inflight.clear()

    def __enter__(self) -> "NetworkMutexClient":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    # -- internals -------------------------------------------------------

    def _register(self, uuid: str) -> "queue.Queue[Response]":
        q: "queue.Queue[Response]" = queue.Queue(maxsize=8)
        with self._inflight_lock:
            if self._closed.is_set():
                raise NetworkMutexError("client closed")
            self._inflight[uuid] = q
        return q

    def _unregister(self, uuid: str) -> None:
        with self._inflight_lock:
            self._inflight.pop(uuid, None)

    def _send(self, frame: bytes) -> None:
        with self._wlock:
            self._sock.sendall(frame)

    def _next(self, q: "queue.Queue[Response]", timeout: Optional[float]) -> Response:
        try:
            item = q.get(timeout=timeout)
        except queue.Empty as exc:
            raise NetworkMutexError("timed out waiting for broker response") from exc
        if item is None:
            raise NetworkMutexError(self._read_err or "client closed")
        return item

    def _roundtrip(self, frame: bytes, uuid: str, *, timeout: Optional[float] = None) -> Response:
        q = self._register(uuid)
        try:
            self._send(frame)
            return self._next(q, timeout)
        finally:
            self._unregister(uuid)

    def _roundtrip_grant(self, frame: bytes, uuid: str, *, timeout: Optional[float] = None) -> Response:
        q = self._register(uuid)
        try:
            self._send(frame)
            while True:
                resp = self._next(q, timeout)
                if resp.type is ResponseType.ERROR:
                    return resp
                if resp.type in (ResponseType.LOCK, ResponseType.COMPOSITE_LOCK):
                    if resp.acquired or resp.error is not None:
                        return resp
                    continue  # queued notification; keep waiting
                return resp
        finally:
            self._unregister(uuid)

    def _roundtrip_until_granted(self, frame: bytes, uuid: str, *, timeout: Optional[float] = None) -> Response:
        q = self._register(uuid)
        try:
            self._send(frame)
            while True:
                resp = self._next(q, timeout)
                if resp.granted:
                    return resp
                if resp.type is ResponseType.ERROR:
                    raise NetworkMutexError(f"rw acquire failed: {resp.error}")
        finally:
            self._unregister(uuid)

    def _read_loop(self) -> None:
        buf = b""
        try:
            while not self._closed.is_set():
                chunk = self._sock.recv(64 * 1024)
                if not chunk:
                    break
                buf += chunk
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    if not line.strip():
                        continue
                    resp = Response.decode(line)
                    self._dispatch(resp)
        except (OSError, ValueError) as exc:
            if not self._closed.is_set():
                self._read_err = exc
        finally:
            self.close()

    def _dispatch(self, resp: Response) -> None:
        with self._inflight_lock:
            q = self._inflight.get(resp.uuid)
        if q is None:
            return
        try:
            q.put_nowait(resp)
        except queue.Full:
            pass  # caller already moved on
