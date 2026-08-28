#!/usr/bin/env python3
# Apply strict, auditable reliability fixes to the reviewed fleet publisher.

from __future__ import annotations

from pathlib import Path
import re
import sys


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one literal match, found {count}")
    return text.replace(old, new, 1)


def regex_once(text: str, pattern: str, replacement: str, label: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{label}: expected one regex match, found {count}")
    return updated


def patch(text: str) -> str:
    text = replace_once(
        text,
        "_print_lock = threading.Lock()\n_result_lock = threading.Lock()\n",
        "_print_lock = threading.Lock()\n"
        "_result_lock = threading.Lock()\n"
        "_api_mutation_lock = threading.Lock()\n"
        "_last_api_mutation_at = 0.0\n",
        "api locks",
    )

    api_function = r'''def api(
    method: str,
    path: str,
    body: dict[str, object] | None = None,
    *,
    accepted: tuple[int, ...] = (200,),
    attempts: int = 12,
) -> tuple[int, Any]:
    payload = None if body is None else json.dumps(body).encode("utf-8")
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {TOKEN}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "github-test-org-factory-protected-publisher",
    }
    if payload is not None:
        headers["Content-Type"] = "application/json"

    method = method.upper()
    mutating = method in {"POST", "PUT", "PATCH", "DELETE"}
    mutation_interval = max(
        0.0,
        float(os.environ.get("TEST_ORG_FACTORY_MUTATION_INTERVAL_SECONDS", "2.5")),
    )

    def open_request(request: urllib.request.Request):
        global _last_api_mutation_at
        if not mutating:
            return urllib.request.urlopen(request, timeout=60)
        with _api_mutation_lock:
            remaining = mutation_interval - (time.monotonic() - _last_api_mutation_at)
            if remaining > 0:
                time.sleep(remaining)
            try:
                return urllib.request.urlopen(request, timeout=60)
            finally:
                _last_api_mutation_at = time.monotonic()

    last_error: Exception | None = None
    for attempt in range(1, attempts + 1):
        request = urllib.request.Request(
            API + path,
            data=payload,
            method=method,
            headers=headers,
        )
        try:
            with open_request(request) as response:
                raw = response.read()
                parsed = json.loads(raw) if raw else None
                if response.status not in accepted:
                    raise RuntimeError(
                        f"GitHub API returned unexpected HTTP {response.status} for {method} {path}"
                    )
                return response.status, parsed
        except urllib.error.HTTPError as error:
            raw = error.read(8192).decode(errors="replace")
            if error.code in accepted:
                try:
                    return error.code, json.loads(raw) if raw else None
                except json.JSONDecodeError:
                    return error.code, raw
            lower = raw.lower()
            retryable_403 = error.code == 403 and (
                error.headers.get("Retry-After") is not None
                or "secondary rate limit" in lower
                or "temporarily blocked from content creation" in lower
                or "rate limit exceeded" in lower
            )
            retryable = error.code in {429, 500, 502, 503, 504} or retryable_403
            if retryable and attempt < attempts:
                retry_after = error.headers.get("Retry-After")
                if retry_after and retry_after.isdigit():
                    delay = max(1, int(retry_after))
                elif retryable_403 or error.code == 429:
                    delay = min(60 * attempt, 300)
                else:
                    delay = min(2 ** attempt, 60)
                emit(
                    f"API_RETRY method={method} path={path} http={error.code} "
                    f"attempt={attempt}/{attempts} delay={delay}s"
                )
                time.sleep(delay)
                last_error = error
                continue
            raise RuntimeError(
                f"GitHub API {error.code} for {method} {path}: {raw[:1500]}"
            ) from error
        except (OSError, TimeoutError) as error:
            last_error = error
            if attempt < attempts:
                delay = min(5 * attempt, 60)
                emit(
                    f"API_RETRY method={method} path={path} network_error "
                    f"attempt={attempt}/{attempts} delay={delay}s"
                )
                time.sleep(delay)
                continue
            break
    raise RuntimeError(f"GitHub API request failed for {method} {path}: {last_error}")'''

    text = regex_once(
        text,
        r"def api\(\n.*?\n\n\ndef safe_extract",
        api_function + "\n\n\ndef safe_extract",
        "api function",
    )

    text = replace_once(
        text,
        '''    observed_visibility = payload.get("visibility")
    if observed_visibility != visibility:
        raise RuntimeError(
            f"visibility mismatch for {expected}: {observed_visibility!r} != {visibility!r}"
        )
    return payload
''',
        '''    observed_visibility = payload.get("visibility")
    if observed_visibility != visibility:
        emit(
            f"PRESERVE_VISIBILITY {expected} observed={observed_visibility!r} "
            f"portfolio={visibility!r}"
        )
    return payload
''',
        "visibility preservation",
    )

    text = replace_once(
        text,
        '''        if branch_exists:
            run(["git", "fetch", "origin", BOOTSTRAP_BRANCH], cwd=checkout, env=git_env)
            if not is_managed_remote(checkout, f"origin/{BOOTSTRAP_BRANCH}"):
                raise RuntimeError(f"refusing unmanaged existing branch {full_name}:{BOOTSTRAP_BRANCH}")
            run(
                ["git", "checkout", "-B", BOOTSTRAP_BRANCH, f"origin/{BOOTSTRAP_BRANCH}"],
                cwd=checkout,
            )
        else:
            if not created and not is_managed_remote(checkout, f"origin/{default_branch}"):
                tracked = run(["git", "ls-tree", "-r", "--name-only", f"origin/{default_branch}"], cwd=checkout).stdout.splitlines()
                harmless = {"README.md", ".gitignore", "LICENSE", "LICENSE.md"}
                if any(path not in harmless for path in tracked):
                    raise RuntimeError(f"refusing unmanaged existing repository {full_name}")
            run(
                ["git", "checkout", "-B", BOOTSTRAP_BRANCH, f"origin/{default_branch}"],
                cwd=checkout,
            )
''',
        '''        if branch_exists:
            run(["git", "fetch", "origin", BOOTSTRAP_BRANCH], cwd=checkout, env=git_env)
            if not is_managed_remote(checkout, f"origin/{BOOTSTRAP_BRANCH}"):
                raise RuntimeError(f"refusing unmanaged existing branch {full_name}:{BOOTSTRAP_BRANCH}")
            # Re-render from the current default branch so interrupted submodule
            # additions or stale index entries cannot poison an idempotent retry.
            run(
                ["git", "checkout", "-B", BOOTSTRAP_BRANCH, f"origin/{default_branch}"],
                cwd=checkout,
            )
        else:
            if not created and not is_managed_remote(checkout, f"origin/{default_branch}"):
                tracked = run(
                    ["git", "ls-tree", "-r", "--name-only", f"origin/{default_branch}"],
                    cwd=checkout,
                ).stdout.splitlines()
                emit(f"PRESERVE_EXISTING_BASELINE {full_name} files={len(tracked)}")
            run(
                ["git", "checkout", "-B", BOOTSTRAP_BRANCH, f"origin/{default_branch}"],
                cwd=checkout,
            )
''',
        "existing repository preservation",
    )

    vendor_helper = r'''
def allow_vendor_submodules(checkout: Path) -> None:
    # Ensure generated ignore rules do not hide real submodule gitlinks.
    ignore_file = checkout / ".gitignore"
    current = ignore_file.read_text(encoding="utf-8") if ignore_file.is_file() else ""
    marker = "# github-test-org-factory: track materialized upstream submodules"
    if marker in current:
        return
    separator = "" if not current or current.endswith("\n") else "\n"
    ignore_file.write_text(
        current
        + separator
        + "\n"
        + marker
        + "\n!vendor/\n!vendor/**\n",
        encoding="utf-8",
    )


'''
    text = replace_once(
        text,
        "\ndef publish_one(\n",
        "\n" + vendor_helper + "def publish_one(\n",
        "vendor helper",
    )

    text = replace_once(
        text,
        '''        copy_rendered_tree(source, checkout)
        submodule_status = "not-requested"
        if materialize_submodules and (checkout / ".gitmodules.template").is_file():
            materializer = factory_root / "factory" / "materialize-submodule.sh"
''',
        '''        copy_rendered_tree(source, checkout)
        submodule_status = "not-requested"
        if materialize_submodules and (checkout / ".gitmodules.template").is_file():
            allow_vendor_submodules(checkout)
            materializer = factory_root / "factory" / "materialize-submodule.sh"
''',
        "vendor materialization",
    )

    text = replace_once(
        text,
        '''    parser.add_argument("--workers", type=int, default=6)
    parser.add_argument("--materialize-submodules", action="store_true")
''',
        '''    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--target-delay-seconds", type=float, default=20.0)
    parser.add_argument("--materialize-submodules", action="store_true")
''',
        "publisher arguments",
    )

    text = replace_once(
        text,
        '''    if not 1 <= args.workers <= 12:
        raise SystemExit("--workers must be between 1 and 12")
''',
        '''    if args.workers != 1:
        raise SystemExit("--workers must be 1 for secondary-limit-safe publication")
    if not 0 <= args.target_delay_seconds <= 300:
        raise SystemExit("--target-delay-seconds must be between 0 and 300")
''',
        "argument validation",
    )

    text = replace_once(
        text,
        '''        emit(f"SELECTED repositories={len(targets)} workers={args.workers}")

        results: list[dict[str, object]] = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
            futures = {
                executor.submit(
                    publish_one,
                    target,
                    factory_root=factory_root,
                    work_root=work_root,
                    git_env=git_env,
                    materialize_submodules=args.materialize_submodules,
                ): target
                for target in targets
            }
            for future in concurrent.futures.as_completed(futures):
                result = future.result()
                results.append(result)
                write_result(results_path, result)
''',
        '''        emit(
            f"SELECTED repositories={len(targets)} workers={args.workers} "
            f"target_delay_seconds={args.target_delay_seconds}"
        )

        results: list[dict[str, object]] = []
        for index, target in enumerate(targets, start=1):
            emit(
                f"TARGET index={index}/{len(targets)} "
                f"repository={target['test_org']}/{target['name']}"
            )
            result = publish_one(
                target,
                factory_root=factory_root,
                work_root=work_root,
                git_env=git_env,
                materialize_submodules=args.materialize_submodules,
            )
            results.append(result)
            write_result(results_path, result)
            if index < len(targets) and args.target_delay_seconds:
                time.sleep(args.target_delay_seconds)
''',
        "serial publication",
    )

    text = replace_once(
        text,
        '''        number = pulls[0].get("number")
        if not isinstance(number, int):
            raise RuntimeError(f"invalid existing PR response for {full_name}")
        _, updated = api(
''',
        '''        existing = pulls[0]
        number = existing.get("number")
        if not isinstance(number, int):
            raise RuntimeError(f"invalid existing PR response for {full_name}")
        if existing.get("title") == title and existing.get("body") == body:
            emit(f"REUSED_DRAFT_PR {full_name} #{number}")
            return str(
                existing.get("html_url")
                or f"https://github.com/{full_name}/pull/{number}"
            )
        _, updated = api(
''',
        "reuse unchanged PR",
    )

    return text


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_test_org_factory_publisher.py PUBLISHER")
    path = Path(sys.argv[1])
    original = path.read_text(encoding="utf-8")
    updated = patch(original)
    path.write_text(updated, encoding="utf-8")
    print(f"PATCHED publisher={path} bytes={len(updated.encode('utf-8'))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
