#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

from linear_project_sync_github import ApiError, add_draft, bounded, load_project, make_app_jwt, mint_org_token, set_status

STATE_STATUS = {"started": "In Progress", "unstarted": "Todo", "backlog": "Backlog"}
ALIASES = {
    "inprogress": {"inprogress", "doing", "active", "started"},
    "todo": {"todo", "ready", "planned", "notstarted"},
    "backlog": {"backlog"},
}


def norm(value: object) -> str:
    return "".join(ch for ch in str(value).casefold() if ch.isalnum())


def load_manifest(directory: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for path in sorted(directory.glob("*.json")):
        part = json.loads(path.read_text())
        if not isinstance(part, list):
            raise ApiError(f"{path} must contain a JSON array")
        entries.extend(part)
    if len(entries) != 40:
        raise ApiError(f"Manifest must contain 40 organizations, found {len(entries)}")
    seen_orgs: set[str] = set()
    seen_issues: set[str] = set()
    count = 0
    for entry in entries:
        org = entry.get("organization")
        if not isinstance(org, str) or not org or org.casefold() in seen_orgs:
            raise ApiError(f"Invalid or duplicate organization: {org!r}")
        seen_orgs.add(org.casefold())
        if entry.get("project_number") != 1 or entry.get("project_title") != f"{org}-project":
            raise ApiError(f"{org} does not target its canonical project number/title")
        if entry.get("project_url") != f"https://github.com/orgs/{org}/projects/1":
            raise ApiError(f"{org} does not target its canonical project URL")
        issues = entry.get("issues")
        if not isinstance(issues, list) or len(issues) > 3:
            raise ApiError(f"{org} must select zero to three issues")
        if not issues and entry.get("empty_verified_states") != ["started", "unstarted", "backlog"]:
            raise ApiError(f"{org} empty state is not fully verified")
        for issue in issues:
            ident = issue.get("identifier")
            state = issue.get("linear_state")
            base = f"https://linear.app/denman/issue/{ident}"
            if not isinstance(ident, str) or not ident.startswith("DEN-") or ident in seen_issues:
                raise ApiError(f"Invalid or duplicate Linear identifier: {ident!r}")
            if state not in STATE_STATUS or issue.get("project_status") != STATE_STATUS[state]:
                raise ApiError(f"{ident} has inconsistent state/status")
            if not isinstance(issue.get("title"), str) or not issue["title"].strip():
                raise ApiError(f"{ident} has no title")
            url = str(issue.get("url") or "")
            if url != base and not url.startswith(base + "/"):
                raise ApiError(f"{ident} has a non-canonical Linear URL")
            seen_issues.add(ident)
            count += 1
    if count != 85:
        raise ApiError(f"Manifest must select 85 unique issues, found {count}")
    return entries


def status_field(project: dict[str, Any]) -> dict[str, Any] | None:
    for field in (project.get("fields") or {}).get("nodes") or []:
        if field.get("__typename") == "ProjectV2SingleSelectField" and norm(field.get("name")) == "status":
            return field
    return None


def status_option(field: dict[str, Any], desired: str) -> str | None:
    accepted = ALIASES.get(norm(desired), {norm(desired)})
    for option in field.get("options") or []:
        if norm(option.get("name")) in accepted and isinstance(option.get("id"), str):
            return option["id"]
    return None


def haystack(item: dict[str, Any]) -> str:
    content = item.get("content") or {}
    return "\n".join(str(content.get(key) or "") for key in ("title", "body", "url")).casefold()


def matches(items: list[dict[str, Any]], issue: dict[str, Any]) -> list[dict[str, Any]]:
    ident = issue["identifier"].casefold()
    url = issue["url"].casefold()
    marker = f"linear-sync-key:{ident}"
    return [item for item in items if marker in haystack(item) or url in haystack(item) or f"[{ident}]" in haystack(item)]


def body(entry: dict[str, Any], issue: dict[str, Any]) -> str:
    return "\n".join([
        f"<!-- linear-sync-key:{issue['identifier']} -->",
        f"Linear: {issue['url']}",
        f"Linear project: {entry['linear_project']}",
        f"Linear state: {issue['linear_state']}",
        f"Priority: {issue['priority']}",
        "Selected: 2026-08-04",
        "Source of truth: Linear",
    ])


def result(org: str, selected: int) -> dict[str, Any]:
    return {"organization": org, "selected": selected, "created": 0, "reused": 0, "status": 0, "duplicates": 0, "outcome": "ok", "errors": []}


def sync_one(credential: str, entry: dict[str, Any], dry_run: bool, direct_token: bool = False) -> dict[str, Any]:
    org, issues = entry["organization"], entry["issues"]
    out = result(org, len(issues))
    try:
        token = credential if direct_token else mint_org_token(credential, org)
        project = load_project(token, org, 1)
        if project.get("closed") or project.get("title") != entry["project_title"] or project.get("url") != entry["project_url"]:
            raise ApiError(f"{org} project number 1 title/URL/open-state contract failed")
        if not issues:
            out["outcome"] = "empty"
            return out
        project_id = project.get("id")
        field = status_field(project)
        if not isinstance(project_id, str) or not field or not isinstance(field.get("id"), str):
            raise ApiError(f"{org} project ID or Status field is missing")
        items = list(project.get("all_items") or [])
        for issue in issues:
            found = matches(items, issue)
            if found:
                item_id = found[0].get("id")
                out["reused"] += 1
                if len(found) > 1:
                    out["duplicates"] += len(found) - 1
            else:
                item_id = f"dry:{issue['identifier']}" if dry_run else add_draft(
                    token, project_id, f"[{issue['identifier']}] {issue['title']}", body(entry, issue)
                )
                out["created"] += 1
                items.append({"id": item_id, "content": {"title": f"[{issue['identifier']}] {issue['title']}", "body": body(entry, issue), "url": issue["url"]}})
            option = status_option(field, issue["project_status"])
            if not isinstance(item_id, str) or not option:
                raise ApiError(f"{org}/{issue['identifier']} item ID or status option is missing")
            if not dry_run:
                set_status(token, project_id, item_id, field["id"], option)
            out["status"] += 1
        if out["duplicates"]:
            out["outcome"] = "warning"
            out["errors"].append(f"{out['duplicates']} duplicate matching items")
    except Exception as exc:
        out["outcome"] = "failed"
        out["errors"].append(bounded(exc))
    return out


def report(results: list[dict[str, Any]], dry_run: bool) -> str:
    total = lambda key: sum(int(row[key]) for row in results)
    failed = sum(row["outcome"] == "failed" for row in results)
    warning = sum(row["outcome"] == "warning" for row in results)
    empty = sum(row["outcome"] == "empty" for row in results)
    lines = [
        f"# {'Dry-run ' if dry_run else ''}Linear → GitHub Projects v2 sync", "",
        f"- Organizations checked: **{len(results)}/40**",
        "- Selected Linear issues: **85**",
        f"- Project items created: **{total('created')}**",
        f"- Existing project items reused: **{total('reused')}**",
        f"- Status values written: **{total('status')}**",
        f"- Verified empty Linear projects: **{empty}**",
        f"- Organizations with warnings: **{warning}**",
        f"- Organizations failed: **{failed}**",
        f"- Duplicate matching items detected: **{total('duplicates')}**", "",
        "| Organization | Selected | Created | Reused | Status | Outcome |",
        "|---|---:|---:|---:|---:|---|",
    ]
    for row in results:
        detail = " — " + "; ".join(row["errors"]) if row["errors"] else ""
        lines.append(f"| `{row['organization']}` | {row['selected']} | {row['created']} | {row['reused']} | {row['status']} | {row['outcome']}{detail} |")
    lines += ["", "Linear remains the source of truth. Items are keyed by exact Linear identifier and URL, so reruns are idempotent.", "No personal access token, installation token, App JWT, private key, or API response body is included in this report."]
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest-dir", type=Path, required=True)
    parser.add_argument("--report-path", type=Path)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        entries = load_manifest(args.manifest_dir)
        if args.validate_only:
            print("validated 40 organizations and 85 selected issues")
            return 0
        direct_token = os.environ.get("PROJECT_SYNC_GITHUB_TOKEN", "").strip()
        if direct_token:
            results = [sync_one(direct_token, entry, args.dry_run, direct_token=True) for entry in entries]
        else:
            app_jwt = make_app_jwt(os.environ.get("K8S_SUBMODULE_APP_ID", ""), os.environ.get("K8S_SUBMODULE_APP_PRIVATE_KEY", ""))
            results = [sync_one(app_jwt, entry, args.dry_run) for entry in entries]
        text = report(results, args.dry_run)
        if args.report_path:
            args.report_path.parent.mkdir(parents=True, exist_ok=True)
            args.report_path.write_text(text)
        else:
            sys.stdout.write(text)
        return 1 if any(row["outcome"] in {"failed", "warning"} for row in results) else 0
    except Exception as exc:
        text = f"# Linear → GitHub Projects v2 sync failed\n\nFatal: {bounded(exc)}\n"
        if args.report_path:
            args.report_path.parent.mkdir(parents=True, exist_ok=True)
            args.report_path.write_text(text)
        print(text, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
