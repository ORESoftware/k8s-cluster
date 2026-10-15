#!/usr/bin/env python3
"""End-to-end smoke test mirroring clients/go/cmd/smoke/main.go.

    python3 smoke.py

Override host/port via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT.
"""

from __future__ import annotations

import os
import sys
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from network_mutex import NetworkMutexClient  # noqa: E402


def main() -> None:
    host = os.environ.get("LIVE_MUTEX_HOST", "127.0.0.1")
    port = int(os.environ.get("LIVE_MUTEX_PORT", "6970"))

    with NetworkMutexClient.connect(host, port, connect_timeout=30) as client:
        print(f"[smoke-python] connected {host}:{port}")

        ex = client.acquire("smoke-python-exclusive", ttl_ms=5000, timeout=30)
        print(f"[smoke-python] exclusive grant: lockUuid={ex.lock_uuid} fencing={ex.fencing_token}")
        client.release(ex, timeout=30)
        print("[smoke-python] released exclusive")

        comp = client.acquire_many(
            ["smoke-python-a", "smoke-python-b", "smoke-python-c"], ttl_ms=5000, timeout=30
        )
        print(f"[smoke-python] composite grant: lockUuid={comp.lock_uuid} tokens={comp.fencing_tokens}")
        client.release(comp, timeout=30)
        print("[smoke-python] released composite")

        w = client.acquire_write("smoke-python-rw", timeout=30)
        print(f"[smoke-python] writer grant: id={w.lock_uuid} fencing={w.fencing_token}")
        client.release_write("smoke-python-rw", timeout=30)

        results = []
        lock = threading.Lock()

        def reader(idx: int) -> None:
            r = client.acquire_read("smoke-python-rw", timeout=30)
            with lock:
                results.append((idx, r.fencing_token))

        threads = [threading.Thread(target=reader, args=(i,)) for i in range(2)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        print(f"[smoke-python] readers: {sorted(results)}")
        client.release_read("smoke-python-rw", timeout=30)
        client.release_read("smoke-python-rw", timeout=30)

        print("[smoke-python] OK")


if __name__ == "__main__":
    main()
