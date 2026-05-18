#!/usr/bin/env python3
"""
End-to-end demo for the gleamlang-presence-server.

Connection topology
-------------------
A single logical "device" opens MULTIPLE websockets:

  * exactly one user-scoped ws  (/ws?user=<id>) — receives membership
    notifications like "added-to <conv>" / "removed-from <conv>"; the
    client uses these to decide when to open/close per-conv websockets.
  * one conv-scoped ws per active conversation
    (/ws?user=<id>&conv=<convId>) — receives the conv's broadcast frames.

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


class WS:
    """One websocket connection. Reads frames into a queue for assertions."""

    def __init__(self, url: str, label: str) -> None:
        self.url = url
        self.label = label
        self.queue: asyncio.Queue[str] = asyncio.Queue()
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._task: asyncio.Task | None = None
        self._closed = False

    async def open(self) -> None:
        self._ws = await websockets.connect(self.url)
        self._task = asyncio.create_task(self._reader())
        # Give the server a moment to register us under the right groups.
        await asyncio.sleep(0.05)

    async def _reader(self) -> None:
        assert self._ws is not None
        try:
            async for frame in self._ws:
                if isinstance(frame, bytes):
                    frame = frame.decode("utf-8", "replace")
                await self.queue.put(frame)
        except websockets.ConnectionClosed:
            self._closed = True

    async def close(self) -> None:
        if self._ws is not None:
            await self._ws.close()
        if self._task is not None:
            self._task.cancel()
        self._closed = True

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


class Device:
    """A logical device: one user-scoped ws + N conv-scoped ws's."""

    def __init__(self, base: str, user: str, device_id: str, label: str) -> None:
        self.base = base
        self.user = user
        self.device_id = device_id
        self.label = label
        self.user_ws: WS | None = None
        self.conv_ws: dict[str, WS] = {}

    async def open_user(self) -> None:
        url = (
            f"{_ws_base(self.base)}/ws?user={self.user}&device={self.device_id}"
        )
        ws = WS(url, f"{self.label}/user")
        await ws.open()
        self.user_ws = ws
        print(f"  • {self.label} ({self.user}@{self.base}) opened user-ws")

    async def open_conv(self, conv_id: str) -> None:
        url = (
            f"{_ws_base(self.base)}/ws?user={self.user}"
            f"&conv={conv_id}&device={self.device_id}"
        )
        ws = WS(url, f"{self.label}/{conv_id}")
        await ws.open()
        self.conv_ws[conv_id] = ws
        print(f"  • {self.label} opened conv-ws for {conv_id}")

    async def close_conv(self, conv_id: str) -> None:
        ws = self.conv_ws.pop(conv_id, None)
        if ws is not None:
            await ws.close()
            print(f"  • {self.label} closed conv-ws for {conv_id}")

    async def close(self) -> None:
        if self.user_ws is not None:
            await self.user_ws.close()
        for conv_id, ws in list(self.conv_ws.items()):
            await ws.close()
        self.conv_ws.clear()


async def expect_contains(ws: WS, fragment: str, secs: float = 1.0) -> bool:
    frames = await ws.drain_for(secs)
    hit = any(fragment in f for f in frames)
    status = "OK " if hit else "MISS"
    print(f"      {status}  {ws.label} <- waiting for '{fragment}'  (saw {frames})")
    return hit


async def expect_contains_all(ws: WS, fragments: list[str], secs: float = 1.0) -> bool:
    frames = await ws.drain_for(secs)
    hit = any(all(fr in f for fr in fragments) for f in frames)
    status = "OK " if hit else "MISS"
    print(
        f"      {status}  {ws.label} <- waiting for all of {fragments}  (saw {frames})"
    )
    return hit


async def expect_silent(ws: WS, secs: float = 0.5) -> bool:
    frames = await ws.drain_for(secs)
    hit = len(frames) == 0
    status = "OK " if hit else "MISS"
    print(f"      {status}  {ws.label} silent for {secs}s  (saw {frames})")
    return hit


async def main(bases: list[str]) -> int:
    bases = _normalise_bases(bases)
    n = len(bases)
    print(f"\n=== presence-server demo against {n} base URL(s): {bases} ===")

    for b in bases:
        print(f"\n[health] {b}/healthz -> {_get(b + '/healthz')}")
        print(f"[health] {b}/nodes ->\n{_get(b + '/nodes')}")

    # Two users with two devices each + one extra user, pinned across
    # bases so cross-node fanout is exercised.
    alice1 = Device(bases[0], "alice", "d1", "alice/dev1")
    alice2 = Device(bases[min(1, n - 1)], "alice", "d2", "alice/dev2")
    bob1 = Device(bases[min(2, n - 1)] if n >= 3 else bases[0], "bob", "d1", "bob/dev1")
    bob2 = Device(bases[0], "bob", "d2", "bob/dev2")
    carol = Device(bases[min(1, n - 1)], "carol", "d1", "carol/dev1")
    devices = [alice1, alice2, bob1, bob2, carol]

    # Pre-populate conv-1 BEFORE any conv-ws is opened so on_init can
    # accept the conv-scope upgrades.
    print("\n[setup] pre-populating conv-1 with alice + bob via POST .../members/...")
    print(f"  {_post(bases[0] + '/conv/conv-1/members/alice')}")
    print(f"  {_post(bases[0] + '/conv/conv-1/members/bob')}")

    # Every device opens its user-scoped ws.
    print("\n[connect] opening user-scoped websockets")
    for d in devices:
        await d.open_user()

    # alice and bob open their conv-1 ws's (they're already members).
    print("\n[connect] opening conv-1 websockets for alice + bob")
    for d in [alice1, alice2, bob1, bob2]:
        await d.open_conv("conv-1")

    # --- Scenario 1: conv broadcast lands on the conv-ws of every member,
    #                 NOT the user-ws of anyone (and not carol).
    print(
        "\n[scenario 1] broadcast 'hello-1' to conv-1; expect alice+bob conv-1 ws's (4),"
        " NOT their user-ws's, NOT carol"
    )
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-1')}")
    ok1 = await expect_contains(alice1.conv_ws["conv-1"], "hello-1")
    ok2 = await expect_contains(alice2.conv_ws["conv-1"], "hello-1")
    ok3 = await expect_contains(bob1.conv_ws["conv-1"], "hello-1")
    ok4 = await expect_contains(bob2.conv_ws["conv-1"], "hello-1")
    ok5 = await expect_silent(alice1.user_ws)  # user-ws does NOT get conv frames
    ok6 = await expect_silent(carol.user_ws)
    assert carol.user_ws is not None

    # --- Scenario 2: carol gets added to conv-1 mid-session. Her USER-ws
    # receives a "membership-changed" JSON envelope with `change:"added"`
    # and the conv's full member list. The demo (acting as the client)
    # then opens her conv-1 ws and the next broadcast lands there.
    print(
        "\n[scenario 2] carol added to conv-1; user-ws sees membership-changed JSON"
        " (with members list), then conv-ws receives next broadcast"
    )
    print(f"  {_post(bases[0] + '/conv/conv-1/members/carol')}")
    ok7 = await expect_contains_all(
        carol.user_ws,
        [
            '"type":"membership-changed"',
            '"change":"added"',
            '"conv":"conv-1"',
            '"alice"',
            '"bob"',
            '"carol"',
        ],
    )

    await carol.open_conv("conv-1")
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-2')}")
    ok8 = await expect_contains(carol.conv_ws["conv-1"], "hello-2")

    # --- Scenario 3: bob removed from conv-1. Bob's USER-ws sees a
    # "membership-changed" JSON envelope with `change:"removed"`. Bob's
    # CONV-ws's receive a "kick" JSON envelope and close. Subsequent
    # broadcast does not reach bob.
    print(
        "\n[scenario 3] bob removed from conv-1; user-ws sees membership-changed"
        " removed, conv-ws gets kick. Next broadcast: alice+carol get it, bob silent"
    )
    bob1_conv = bob1.conv_ws["conv-1"]
    bob2_conv = bob2.conv_ws["conv-1"]
    print(f"  {_delete(bases[0] + '/conv/conv-1/members/bob')}")
    ok9 = await expect_contains_all(
        bob1.user_ws,
        ['"type":"membership-changed"', '"change":"removed"', '"conv":"conv-1"'],
    )
    ok10 = await expect_contains_all(
        bob2.user_ws,
        ['"type":"membership-changed"', '"change":"removed"', '"conv":"conv-1"'],
    )
    ok11 = await expect_contains_all(
        bob1_conv,
        ['"type":"kick"', '"reason":"removed from conv conv-1"'],
    )
    ok12 = await expect_contains_all(
        bob2_conv,
        ['"type":"kick"', '"reason":"removed from conv conv-1"'],
    )

    # Give the server a moment to actually close the conv-ws's.
    await asyncio.sleep(0.2)
    bob1.conv_ws.pop("conv-1", None)
    bob2.conv_ws.pop("conv-1", None)

    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'hello-3')}")
    ok13 = await expect_contains(alice1.conv_ws["conv-1"], "hello-3")
    ok14 = await expect_contains(carol.conv_ws["conv-1"], "hello-3")
    ok15 = await expect_silent(bob1_conv)
    ok16 = await expect_silent(bob2_conv)

    print("\n[cleanup] closing devices")
    for d in devices:
        await d.close()

    summary = [
        ok1, ok2, ok3, ok4, ok5, ok6, ok7, ok8,
        ok9, ok10, ok11, ok12, ok13, ok14, ok15, ok16,
    ]
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
