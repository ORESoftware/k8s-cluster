#!/usr/bin/env python3
"""Development-only nerdctl shim for local container-pool smoke tests.

It implements the tiny subset of `nerdctl` used by dd-container-pool, but launches the common
HTTP worker as a local process. Use only when Docker/containerd is unavailable; EC2 should use the
real `/usr/local/bin/nerdctl`.
"""

import json
import os
import signal
import subprocess
import sys
from pathlib import Path


STATE_PATH = Path(
    os.getenv("DD_NERDCTL_PROCESS_SHIM_STATE", "/private/tmp/dd-nerdctl-process-shim.json")
)
WORKER = os.getenv(
    "DD_NERDCTL_PROCESS_SHIM_WORKER",
    str(Path(__file__).resolve().parents[1] / "runtime-images/common/worker.py"),
)


def load_state():
    if not STATE_PATH.exists():
        return {}
    try:
        return json.loads(STATE_PATH.read_text())
    except json.JSONDecodeError:
        return {}


def save_state(state):
    STATE_PATH.write_text(json.dumps(state, indent=2, sort_keys=True))


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def strip_namespace(args):
    result = []
    index = 0
    while index < len(args):
        if args[index] == "-n" and index + 1 < len(args):
            index += 2
        else:
            result.append(args[index])
            index += 1
    return result


def handle_ps():
    state = load_state()
    live = {}
    for name, record in state.items():
        if alive(record["pid"]):
            live[name] = record
            print(name)
    save_state(live)


def handle_run(args):
    name = None
    published_host_port = None
    env = os.environ.copy()
    env["DD_POOL_HANDLER"] = ""
    index = 0
    while index < len(args):
        item = args[index]
        if item == "--name" and index + 1 < len(args):
            name = args[index + 1]
            index += 2
        elif item == "--env" and index + 1 < len(args):
            key, _, value = args[index + 1].partition("=")
            if key:
                env[key] = value
            index += 2
        elif item == "--publish" and index + 1 < len(args):
            published_host_port = args[index + 1].split(":")[-2]
            index += 2
        elif item in {"--label", "--network", "--pull"} and index + 1 < len(args):
            index += 2
        elif item == "-d":
            index += 1
        else:
            index += 1
    if not name:
        print("missing --name", file=sys.stderr)
        return 2
    if published_host_port:
        env["PORT"] = published_host_port
    process = subprocess.Popen(
        [sys.executable, WORKER],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    state = load_state()
    state[name] = {
        "pid": process.pid,
        "port": env.get("PORT"),
        "runtime": env.get("DD_POOL_RUNTIME"),
    }
    save_state(state)
    print(name)
    return 0


def handle_rm(args):
    state = load_state()
    for item in args:
        if item == "-f":
            continue
        record = state.pop(item, None)
        if record:
            try:
                os.killpg(record["pid"], signal.SIGTERM)
            except OSError:
                pass
    save_state(state)
    return 0


def handle_inspect(args):
    if not args:
        print("missing container name", file=sys.stderr)
        return 2
    state = load_state()
    inspected = []
    changed = False
    for name in args:
        record = state.get(name)
        if not record:
            print(f"no such container: {name}", file=sys.stderr)
            return 1
        running = alive(record["pid"])
        if not running:
            state.pop(name, None)
            changed = True
        inspected.append(
            {
                "Name": name,
                "State": {
                    "Running": running,
                    "Status": "running" if running else "exited",
                },
            }
        )
    if changed:
        save_state(state)
    print(json.dumps(inspected))
    return 0


def main():
    args = strip_namespace(sys.argv[1:])
    if not args:
        return 2
    command, rest = args[0], args[1:]
    if command == "ps":
        handle_ps()
        return 0
    if command == "run":
        return handle_run(rest)
    if command == "rm":
        return handle_rm(rest)
    if command == "inspect":
        return handle_inspect(rest)
    print(f"unsupported command: {command}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
