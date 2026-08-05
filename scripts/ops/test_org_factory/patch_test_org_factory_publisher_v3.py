#!/usr/bin/env python3
"""Add bounded index selection and a configurable bootstrap branch to v2 publisher."""

from __future__ import annotations

from pathlib import Path
import re
import sys


def regex_once(text: str, pattern: str, replacement: str, label: str, *, flags: int = 0) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=flags)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return updated


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one literal match, found {count}")
    return text.replace(old, new, 1)


def patch(text: str) -> str:
    text = regex_once(
        text,
        r'^BOOTSTRAP_BRANCH\s*=\s*["\']agent/bootstrap-test-portfolio["\']\s*$',
        'BOOTSTRAP_BRANCH = os.environ.get(\n'
        '    "TEST_ORG_FACTORY_BOOTSTRAP_BRANCH",\n'
        '    "agent/bootstrap-test-portfolio",\n'
        ')',
        "configurable bootstrap branch",
        flags=re.M,
    )

    text = replace_once(
        text,
        '''    parser.add_argument("--workers", type=int, default=1)\n    parser.add_argument("--target-delay-seconds", type=float, default=20.0)\n    parser.add_argument("--materialize-submodules", action="store_true")\n''',
        '''    parser.add_argument("--workers", type=int, default=1)\n    parser.add_argument("--target-delay-seconds", type=float, default=20.0)\n    parser.add_argument("--start-index", type=int, default=1)\n    parser.add_argument("--end-index", type=int, default=0)\n    parser.add_argument("--indices", default="")\n    parser.add_argument("--materialize-submodules", action="store_true")\n''',
        "bounded selection arguments",
    )

    text = replace_once(
        text,
        '''    if args.workers != 1:\n        raise SystemExit("--workers must be 1 for secondary-limit-safe publication")\n    if not 0 <= args.target_delay_seconds <= 300:\n        raise SystemExit("--target-delay-seconds must be between 0 and 300")\n''',
        '''    if args.workers != 1:\n        raise SystemExit("--workers must be 1 for secondary-limit-safe publication")\n    if not 0 <= args.target_delay_seconds <= 300:\n        raise SystemExit("--target-delay-seconds must be between 0 and 300")\n    if args.start_index < 1:\n        raise SystemExit("--start-index must be at least 1")\n    if args.end_index < 0:\n        raise SystemExit("--end-index must be zero or positive")\n''',
        "bounded selection validation",
    )

    old = '''        emit(\n            f"SELECTED repositories={len(targets)} workers={args.workers} "\n            f"target_delay_seconds={args.target_delay_seconds}"\n        )\n\n        results: list[dict[str, object]] = []\n        for index, target in enumerate(targets, start=1):\n            emit(\n                f"TARGET index={index}/{len(targets)} "\n                f"repository={target['test_org']}/{target['name']}"\n            )\n            result = publish_one(\n                target,\n                factory_root=factory_root,\n                work_root=work_root,\n                git_env=git_env,\n                materialize_submodules=args.materialize_submodules,\n            )\n            results.append(result)\n            write_result(results_path, result)\n            if index < len(targets) and args.target_delay_seconds:\n                time.sleep(args.target_delay_seconds)\n'''

    new = '''        total_targets = len(targets)\n        selected_targets: list[tuple[int, dict[str, object]]] = []\n        if args.indices.strip():\n            seen_indices: set[int] = set()\n            for raw_index in args.indices.split(","):\n                raw_index = raw_index.strip()\n                if not raw_index:\n                    continue\n                try:\n                    global_index = int(raw_index)\n                except ValueError as error:\n                    raise SystemExit(f"invalid --indices entry: {raw_index!r}") from error\n                if not 1 <= global_index <= total_targets:\n                    raise SystemExit(\n                        f"--indices entry {global_index} is outside 1..{total_targets}"\n                    )\n                if global_index in seen_indices:\n                    raise SystemExit(f"duplicate --indices entry: {global_index}")\n                seen_indices.add(global_index)\n                selected_targets.append((global_index, targets[global_index - 1]))\n            if not selected_targets:\n                raise SystemExit("--indices did not select any repositories")\n            selection_label = ",".join(str(index) for index, _ in selected_targets)\n        else:\n            end_index = args.end_index or total_targets\n            if not args.start_index <= end_index <= total_targets:\n                raise SystemExit(\n                    f"invalid range {args.start_index}..{end_index}; total is {total_targets}"\n                )\n            selected_targets = [\n                (global_index, targets[global_index - 1])\n                for global_index in range(args.start_index, end_index + 1)\n            ]\n            selection_label = f"{args.start_index}-{end_index}"\n\n        emit(\n            f"SELECTED repositories={len(selected_targets)} total={total_targets} "\n            f"selection={selection_label} workers={args.workers} "\n            f"target_delay_seconds={args.target_delay_seconds} "\n            f"bootstrap_branch={BOOTSTRAP_BRANCH}"\n        )\n\n        results: list[dict[str, object]] = []\n        for local_index, (global_index, target) in enumerate(selected_targets, start=1):\n            emit(\n                f"TARGET index={global_index}/{total_targets} "\n                f"chunk={local_index}/{len(selected_targets)} "\n                f"repository={target['test_org']}/{target['name']}"\n            )\n            result = publish_one(\n                target,\n                factory_root=factory_root,\n                work_root=work_root,\n                git_env=git_env,\n                materialize_submodules=args.materialize_submodules,\n            )\n            result["global_index"] = global_index\n            results.append(result)\n            write_result(results_path, result)\n            if local_index < len(selected_targets) and args.target_delay_seconds:\n                time.sleep(args.target_delay_seconds)\n'''
    text = replace_once(text, old, new, "bounded serial selection")
    return text


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_test_org_factory_publisher_v3.py PUBLISHER")
    path = Path(sys.argv[1])
    original = path.read_text(encoding="utf-8")
    updated = patch(original)
    path.write_text(updated, encoding="utf-8")
    print(f"PATCHED_V3 publisher={path} bytes={len(updated.encode('utf-8'))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
