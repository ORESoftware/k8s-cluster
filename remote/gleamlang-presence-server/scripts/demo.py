#!/usr/bin/env python3
"""
End-to-end demo for the gleamlang-presence-server.

Connects several simulated user devices, joins/leaves conversations, and
broadcasts messages. Asserts the broadcast fan-out across users + devices
behaves as expected and that "immediate leave" semantics hold.

For a single-node run, point this at a single base URL (default
http://localhost:8181). For a 3-node local cluster, pass several URLs via
`--bases http://localhost:8181 http://localhost:8182 http://localhost:8183`
and the script will pin each user's two devices to two different bases so
broadcasts have to cross nodes to reach them.

Requires: pip install websockets
"""
from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time
import urllib.error
import urllib.request

import websockets


def _normalise_bases(bases: list[str]) -> list[str]:
    return [b.rstrip("/") for b in bases]


def _ws_base(http_base: str) -> str:
    return http_base.replace("http://", "ws://").replace("https://", "wss://")


def _post(url: str, body: bytes = b"") -> str:
    req = urllib.request.Request(url, data=body, method="POST")
    return _read(req)


def _delete(url: str) -> str:
    req = urllib.request.Request(url, method="DELETE")
    return _read(req)


def _get(url: str) -> str:
    return _read(urllib.request.Request(url, method="GET"))


def _read(req) -> str:
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            return resp.read().decode("utf-8", "replace").strip()
    except urllib.error.HTTPError as e:
        return f"HTTP {e.code}: {e.read().decode('utf-8', 'replace')}"


class Device:
    """One websocket connection. Reads frames into a queue for assertions."""

    def __init__(self, base: str, user: str, label: str) -> None:
        self.base = base
        self.user = user
        self.label = label
        self.queue: asyncio.Queue[str] = asyncio.Queue()
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._task: asyncio.Task | None = None

    async def open(self) -> None:
        url = f"{_ws_base(self.base)}/ws?user={self.user}"
        self._ws = await websockets.connect(url)
        self._task = asyncio.create_task(self._reader())
        # Give the server a moment to register us under ByUser(_) and any
        # ByConv(_) groups loaded from Postgres.
        await asyncio.sleep(0.05)
        print(f"  • {self.label} ({self.user}@{self.base}) opened")

    async def _reader(self) -> None:
        assert self._ws is not None
        try:
            async for frame in self._ws:
                if isinstance(frame, bytes):
                    frame = frame.decode("utf-8", "replace")
                await self.queue.put(frame)
        except websockets.ConnectionClosed:
            pass

    async def close(self) -> None:
        if self._ws is not None:
            await self._ws.close()
        if self._task is not None:
            self._task.cancel()

    async def drain_for(self, secs: float = 0.5) -> list[str]:
        out: list[str] = []
        deadline = time.monotonic() + secs
        while time.monotonic() < deadline:
            try:
                frame = await asyncio.wait_for(
                    self.queue.get(), timeout=max(0.01, deadline - time.monotonic())
                )
                out.append(frame)
            except asyncio.TimeoutError:
                break
        return out


async def expect_contains(dev: Device, fragment: str, secs: float = 1.0) -> bool:
    frames = await dev.drain_for(secs)
    hit = any(fragment in f for f in frames)
    status = "OK " if hit else "MISS"
    print(f"      {status}  {dev.label} <- waiting for '{fragment}'  (saw {frames})")
    return hit


async def expect_silent(dev: Device, secs: float = 0.5) -> bool:
    frames = await dev.drain_for(secs)
    hit = len(frames) == 0
    status = "OK " if hit else "MISS"
    print(f"      {status}  {dev.label} silent for {secs}s  (saw {frames})")
    return hit


async def main(bases: list[str]) -> int:
    bases = _normalise_bases(bases)
    n = len(bases)
    print(f"\n=== presence-server demo against {n} base URL(s): {bases} ===")

    # Health check.
    for b in bases:
        print(f"\n[health] {b}/healthz -> {_get(b + '/healthz')}")
        print(f"[health] {b}/nodes ->\n{_get(b + '/nodes')}")

    # Set up: two users, each with two devices. If we have multiple bases,
    # pin devices on different bases so broadcasts have to cross nodes.
    alice1 = Device(bases[0], "alice", "alice/dev1")
    alice2 = Device(bases[min(1, n - 1)], "alice", "alice/dev2")
    bob1 = Device(bases[min(2, n - 1)] if n >= 3 else bases[0], "bob", "bob/dev1")
    bob2 = Device(bases[0], "bob", "bob/dev2")
    carol = Device(bases[min(1, n - 1)], "carol", "carol/dev1")

    devices = [alice1, alice2, bob1, bob2, carol]

    # Join alice and bob to conv-1 BEFORE they connect, so on_init loads
    # their membership from the store.
    print("\n[setup] pre-populating conv-1 with alice + bob via POST /conv/.../members/...")
    print(f"  {_post(bases[0] + '/conv/conv-1/members/alice')}")
    print(f"  {_post(bases[0] + '/conv/conv-1/members/bob')}")

    print("\n[connect] opening websockets")
    for d in devices:
        await d.open()

    # --- Scenario 1: broadcast to conv-1 reaches alice+bob (4 devices) but not carol.
    print("\n[scenario 1] broadcast 'hello-1' to conv-1; expect alice+bob (4 devices), NOT carol")
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-1')}")
    ok1 = await expect_contains(alice1, "hello-1")
    ok2 = await expect_contains(alice2, "hello-1")
    ok3 = await expect_contains(bob1, "hello-1")
    ok4 = await expect_contains(bob2, "hello-1")
    ok5 = await expect_silent(carol)

    # --- Scenario 2: carol joins conv-1 mid-session; gets JoinConv + next broadcast.
    print("\n[scenario 2] carol joins conv-1 mid-session; should receive next broadcast")
    print(f"  {_post(bases[0] + '/conv/conv-1/members/carol')}")
    ok6 = await expect_contains(carol, "joined conv-1")
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-2')}")
    ok7 = await expect_contains(carol, "hello-2")

    # --- Scenario 3: immediate-leave semantics — bob leaves and is silent on the next broadcast.
    print("\n[scenario 3] bob leaves conv-1; on next broadcast bob should be silent, others get it")
    print(f"  {_delete(bases[0] + '/conv/conv-1/members/bob')}")
    ok8 = await expect_contains(bob1, "left conv-1")
    ok9 = await expect_contains(bob2, "left conv-1")
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-3')}")
    ok10 = await expect_contains(alice1, "hello-3")
    ok11 = await expect_contains(carol, "hello-3")
    ok12 = await expect_silent(bob1)
    ok13 = await expect_silent(bob2)

    print("\n[cleanup] closing devices")
    for d in devices:
        await d.close()

    summary = [ok1, ok2, ok3, ok4, ok5, ok6, ok7, ok8, ok9, ok10, ok11, ok12, ok13]
    passed = sum(summary)
    print(f"\n=== {passed}/{len(summary)} checks passed ===")
    return 0 if passed == len(summary) else 1


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--bases",
        nargs="+",
        default=["http://localhost:8181"],
        help="One or more presence-server base URLs (e.g. for a multi-node test)",
    )
    args = ap.parse_args()
    sys.exit(asyncio.run(main(args.bases)))
