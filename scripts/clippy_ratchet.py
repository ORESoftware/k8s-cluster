#!/usr/bin/env python3
"""clippy_ratchet.py — a clippy gate that can only get stricter.

WHY THIS EXISTS
---------------
`cargo clippy --all-targets -- -D warnings` reports ~583 findings on this crate,
almost all of them in the large restored legacy files (`src/des/general/soccer/
tests.rs`, `src/des/general/soccer/world.rs`, `src/des/general/soccer.rs`,
`src/des/soccer_learning.rs`). Fixing all of them is a large, separate job.

The tempting shortcut is `continue-on-error: true` on the clippy step. That is
strictly worse than having no step at all: it burns CI minutes producing a green
check mark that carries no information, and it hides *new* findings behind the
old ones. This script is the honest alternative:

  * every finding that exists today is recorded, per (file, lint) pair, in
    scripts/clippy-baseline.json;
  * CI FAILS if any (file, lint) count goes UP, or if a pair appears that is not
    in the baseline — i.e. new code is held to a real `-D warnings` standard;
  * CI FAILS if any count goes DOWN without the baseline being lowered to match,
    so the debt is booked as repaid and can never silently come back.

The baseline is therefore a monotonically shrinking budget, not an exemption.

USAGE
-----
  python3 scripts/clippy_ratchet.py            # check (this is the CI gate)
  python3 scripts/clippy_ratchet.py --update   # re-record baseline after fixing
  python3 scripts/clippy_ratchet.py --summary  # print the debt, no pass/fail

NOTE ON THE CLIPPY INVOCATION
-----------------------------
We deliberately run clippy WITHOUT `-D warnings` and judge the findings
ourselves. `-D warnings` aborts the compilation partway through, so later
targets never get linted and findings hide behind the abort — with plain
warnings we see every target. The strictness is not reduced: for anything not in
the baseline, this script denies exactly what `-D warnings` would.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from collections import Counter

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE_PATH = os.path.join(REPO_ROOT, "scripts", "clippy-baseline.json")
CARGO = os.environ.get("CARGO", "cargo")


def run_clippy() -> list[dict]:
    """Run clippy over every target and return the parsed compiler messages."""
    cmd = [
        CARGO,
        "clippy",
        "--all-targets",
        "--message-format=json",
    ]
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    messages = []
    for line in proc.stdout.splitlines():
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    if not messages:
        sys.stderr.write(proc.stderr)
        raise SystemExit(
            "clippy produced no parseable output — the crate probably failed to "
            "build. Fix the build first; the ratchet cannot judge a tree that "
            "does not compile."
        )
    return messages


def collect(messages: list[dict]) -> Counter:
    """Count unique findings, keyed 'path::lint'.

    `--all-targets` lints the lib and the lib-test build of the same source, so
    the same finding is reported twice; dedupe on (lint, file, line, column,
    text) before counting. Only findings with a `clippy::` code count — plain
    rustc warnings (dead_code and friends) are a different gate's business.
    """
    seen = set()
    counts: Counter = Counter()
    for message in messages:
        if message.get("reason") != "compiler-message":
            continue
        diagnostic = message.get("message") or {}
        if diagnostic.get("level") not in ("warning", "error"):
            continue
        code = ((diagnostic.get("code") or {}).get("code")) or ""
        if not code.startswith("clippy::"):
            continue
        spans = [s for s in (diagnostic.get("spans") or []) if s.get("is_primary")]
        if spans:
            path = spans[0]["file_name"]
            line = spans[0]["line_start"]
            column = spans[0]["column_start"]
        else:
            path, line, column = "<crate>", 0, 0
        key = (code, path, line, column, diagnostic.get("message"))
        if key in seen:
            continue
        seen.add(key)
        counts[f"{path}::{code}"] += 1
    return counts


def load_baseline() -> Counter:
    if not os.path.exists(BASELINE_PATH):
        return Counter()
    with open(BASELINE_PATH, encoding="utf-8") as handle:
        data = json.load(handle)
    return Counter(data.get("findings", {}))


def write_baseline(counts: Counter) -> None:
    payload = {
        "_comment": (
            "Ratcheting clippy baseline — see scripts/clippy_ratchet.py. Counts "
            "of pre-existing `cargo clippy --all-targets` findings, keyed "
            "'path::lint'. This file may only ever SHRINK. Never add an entry by "
            "hand to make CI pass: a new finding is a finding to fix."
        ),
        "total": sum(counts.values()),
        "findings": dict(sorted(counts.items())),
    }
    with open(BASELINE_PATH, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=False)
        handle.write("\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--update",
        action="store_true",
        help="rewrite the baseline from the current tree (use after fixing lints)",
    )
    parser.add_argument(
        "--summary",
        action="store_true",
        help="print the current debt by lint and by file, then exit 0",
    )
    args = parser.parse_args()

    counts = collect(run_clippy())
    total = sum(counts.values())

    if args.summary:
        by_lint: Counter = Counter()
        by_file: Counter = Counter()
        for key, value in counts.items():
            path, _, lint = key.rpartition("::clippy::")
            by_lint[f"clippy::{lint}"] += value
            by_file[path] += value
        print(f"{total} clippy findings\n")
        print("by lint:")
        for lint, value in by_lint.most_common():
            print(f"  {value:5d}  {lint}")
        print("\nby file:")
        for path, value in by_file.most_common():
            print(f"  {value:5d}  {path}")
        return 0

    if args.update:
        write_baseline(counts)
        print(f"baseline written: {total} findings across {len(counts)} file/lint pairs")
        return 0

    baseline = load_baseline()
    regressions = []
    improvements = []
    for key in sorted(set(baseline) | set(counts)):
        allowed = baseline.get(key, 0)
        actual = counts.get(key, 0)
        if actual > allowed:
            regressions.append((key, allowed, actual))
        elif actual < allowed:
            improvements.append((key, allowed, actual))

    if regressions:
        print("clippy ratchet FAILED — new findings are not allowed.\n")
        print("These are NEW violations. Fix them; do not add them to the")
        print("baseline. The baseline records debt that predates the gate, and it")
        print("only ever shrinks.\n")
        for key, allowed, actual in regressions:
            path, _, lint = key.rpartition("::clippy::")
            print(f"  {path}\n    clippy::{lint}: {allowed} allowed, {actual} found (+{actual - allowed})")
        print(f"\ntotal: {sum(baseline.values())} baseline -> {total} now")
        return 1

    if improvements:
        fixed = sum(allowed - actual for _, allowed, actual in improvements)
        print("clippy ratchet needs its baseline lowered.\n")
        print(f"You fixed {fixed} finding(s) — thank you. Book it so it cannot")
        print("come back:\n")
        print("    python3 scripts/clippy_ratchet.py --update\n")
        print("and commit scripts/clippy-baseline.json with your change.\n")
        for key, allowed, actual in improvements[:25]:
            path, _, lint = key.rpartition("::clippy::")
            print(f"  {path}\n    clippy::{lint}: {allowed} -> {actual}")
        if len(improvements) > 25:
            print(f"  ... and {len(improvements) - 25} more")
        return 1

    print(f"clippy ratchet OK — {total} known findings, 0 new.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
