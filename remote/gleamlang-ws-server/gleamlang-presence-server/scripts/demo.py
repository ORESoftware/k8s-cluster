#!/usr/bin/env python3
"""
End-to-end demo for the gleamlang-presence-server.

Connection topology
-------------------
A single logical "device" opens MULTIPLE websockets:

  * exactly one user-scoped ws  (/ws?user=<id>) — receives membership
    notifications like `membership-changed` JSON envelopes; the client
    uses these to decide when to open/close per-conv websockets.
  * one conv-scoped ws per active conversation
    (/ws?user=<id>&conv=<convId>) — receives the conv's broadcast frames.

Both variants accept an optional `&device=<deviceId>` so device-targeted
sends (e.g. "log out this device") can address every ws of one device.

Every ws receives a `{"type":"hello", ...}` JSON envelope on open which
this demo validates as part of its open routine.

Scenarios covered
-----------------
  1. Conv broadcast lands on every member's conv-ws (cross-node).
  2. Add-member fires `membership-changed` (with member list) on the
     added user's user-ws.
  3. Remove-member fires `membership-changed` (removed) on the user-ws
     and a `kick` on every conv-ws of that user, then closes them.
  4. Per-user broadcast lands on every user-ws of that user, NOT on
     their conv-ws's, NOT on other users.
  5. Device logout sends `kick` to every ws of one device (user-scoped
     AND conv-scoped) and closes them; other devices of the same user
     and other users are unaffected.

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
        hello = await _consume_hello(ws)
        assert hello.get("scope") == "user", hello
        assert hello.get("user") == self.user, hello
        assert hello.get("device") == self.device_id, hello
        assert hello.get("conv") is None, hello
        print(f"  • {self.label} ({self.user}@{self.base}) opened user-ws; hello.node={hello.get('node')}")

    async def open_conv(self, conv_id: str) -> None:
        url = (
            f"{_ws_base(self.base)}/ws?user={self.user}"
            f"&conv={conv_id}&device={self.device_id}"
        )
        ws = WS(url, f"{self.label}/{conv_id}")
        await ws.open()
        self.conv_ws[conv_id] = ws
        hello = await _consume_hello(ws)
        assert hello.get("scope") == "conv", hello
        assert hello.get("user") == self.user, hello
        assert hello.get("conv") == conv_id, hello
        assert hello.get("device") == self.device_id, hello
        print(f"  • {self.label} opened conv-ws for {conv_id}; hello.node={hello.get('node')}")

    async def close_conv(self, conv_id: str) -> None:
        ws = self.conv_ws.pop(conv_id, None)
        if ws is not None:
            await ws.close()
            print(f"  • {self.label} closed conv-ws for {conv_id}")

    async def close(self) -> None:
        if self.user_ws is not None and not self.user_ws._closed:
            await self.user_ws.close()
        for conv_id, ws in list(self.conv_ws.items()):
            if not ws._closed:
                await ws.close()
        self.conv_ws.clear()


async def _consume_hello(ws: WS, secs: float = 1.0) -> dict:
    """Drain the JSON `hello` handshake frame that every ws receives on open."""
    frame = await asyncio.wait_for(ws.queue.get(), timeout=secs)
    parsed = json.loads(frame)
    assert parsed.get("type") == "hello", parsed
    return parsed


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

    # --- Scenario 4: per-user system broadcast lands only on user-ws's
    # of that user, on every node. Conv-ws's of the same user are NOT
    # addressed (this is "talk to the user", not "talk to a conv").
    #
    # Drain any conv-ws frames that piled up from scenarios 1-3 first
    # so the "silent" assertions below reflect actual user-broadcast
    # routing rather than leftover conv-broadcast traffic.
    for ws in (alice1.conv_ws.get("conv-1"), alice2.conv_ws.get("conv-1")):
        if ws is not None:
            await ws.drain_for(0.1)

    print(
        "\n[scenario 4] POST /user/alice/broadcast 'sys-msg'; expect alice's two"
        " user-ws's, NOT alice's conv-ws's, NOT other users"
    )
    print(f"  {_post(bases[0] + '/user/alice/broadcast', b'sys-msg')}")
    ok17 = await expect_contains(alice1.user_ws, "sys-msg")
    ok18 = await expect_contains(alice2.user_ws, "sys-msg")
    ok19 = await expect_silent(alice1.conv_ws["conv-1"])
    ok20 = await expect_silent(alice2.conv_ws["conv-1"])
    ok21 = await expect_silent(carol.user_ws)

    # --- Scenario 5: device-targeted logout. POSTing to
    # /user/alice/devices/d1/logout sends a Kick to every ws of
    # (alice, d1) — both her user-ws and her conv-ws on that device.
    # The kick handler sends a JSON envelope and calls `mist.stop()`
    # which tears down the ws actor (and therefore the connection
    # exits ETS, leaves pg, and stops receiving future broadcasts).
    # We verify the kick frame arrived AND that a follow-up broadcast
    # never reaches the kicked ws — but DOES reach alice/dev2.
    print(
        "\n[scenario 5] POST /user/alice/devices/d1/logout; expect alice/d1"
        " user-ws+conv-ws to receive kick frame and be unregistered;"
        " alice/d2 keeps working; carol silent"
    )
    alice1_user = alice1.user_ws
    alice1_conv = alice1.conv_ws["conv-1"]
    print(
        f"  {_post(bases[0] + '/user/alice/devices/d1/logout', b'manual-logout')}"
    )
    ok22 = await expect_contains_all(
        alice1_user, ['"type":"kick"', '"reason":"manual-logout"']
    )
    ok23 = await expect_contains_all(
        alice1_conv, ['"type":"kick"', '"reason":"manual-logout"']
    )
    # Give the server a moment to actually stop the ws actors.
    await asyncio.sleep(0.3)

    # Follow-up broadcasts: alice/d1 ws's should NOT receive them
    # (they've been kicked and unregistered), but alice/d2 ws's
    # should — confirming d2 is unaffected.
    print(f"  {_post(bases[0] + '/conv/conv-1/broadcast', b'post-kick-conv')}")
    print(f"  {_post(bases[0] + '/user/alice/broadcast', b'post-kick-user')}")
    ok24 = await expect_silent(alice1_user)
    ok25 = await expect_silent(alice1_conv)
    ok26 = await expect_contains(alice2.user_ws, "post-kick-user")
    ok27 = await expect_contains(alice2.conv_ws["conv-1"], "post-kick-conv")
    # carol unaffected by the device kick (different user).
    ok28 = await expect_silent(carol.user_ws)
    # Tidy up our records so the cleanup loop doesn't try to close them
    # twice.
    alice1.user_ws = None
    alice1.conv_ws.pop("conv-1", None)

    print("\n[cleanup] closing devices")
    for d in devices:
        await d.close()

    summary = [
        ok1, ok2, ok3, ok4, ok5, ok6, ok7, ok8,
        ok9, ok10, ok11, ok12, ok13, ok14, ok15, ok16,
        ok17, ok18, ok19, ok20, ok21,
        ok22, ok23, ok24, ok25, ok26, ok27, ok28,
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
