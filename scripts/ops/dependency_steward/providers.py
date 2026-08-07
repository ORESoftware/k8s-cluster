"""GitHub and Linear provider clients used by the dependency steward."""

from __future__ import annotations

import argparse
import base64
import configparser
import csv
import dataclasses
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

from .models import *

class JsonHttpClient:
    def __init__(self, token: str, *, user_agent: str = JOB_MARKER) -> None:
        self.token = token
        self.user_agent = user_agent

    def request(
        self,
        method: str,
        url: str,
        *,
        payload: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
        timeout: int = 60,
    ) -> Any:
        data = None
        request_headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": self.user_agent,
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if headers:
            request_headers.update(headers)
        if payload is not None:
            data = json.dumps(payload).encode()
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            url, data=data, headers=request_headers, method=method
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode(errors="replace")
            raise StewardError(
                f"{method} {url} failed with HTTP {exc.code}: {redact(raw[-1000:])}"
            ) from exc
        except urllib.error.URLError as exc:
            raise StewardError(f"{method} {url} failed: {exc.reason}") from exc


class GitHubClient:
    def __init__(self, token: str, api_url: str = "https://api.github.com") -> None:
        self.http = JsonHttpClient(token)
        self.token = token
        self.api_url = api_url.rstrip("/")

    def _url(self, path: str) -> str:
        return self.api_url + path

    def list_org_repositories(self, org: str) -> list[dict[str, Any]]:
        repositories: list[dict[str, Any]] = []
        page = 1
        while True:
            path = (
                f"/orgs/{urllib.parse.quote(org)}/repos?type=all&per_page=100&page={page}"
            )
            batch = self.http.request("GET", self._url(path))
            if not isinstance(batch, list):
                raise StewardError(f"unexpected repository response for {org}")
            repositories.extend(item for item in batch if isinstance(item, dict))
            if len(batch) < 100:
                break
            page += 1
        return repositories

    def repository(self, full_name: str) -> dict[str, Any]:
        owner, name = full_name.split("/", 1)
        result = self.http.request(
            "GET", self._url(f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}")
        )
        if not isinstance(result, dict):
            raise StewardError(f"unexpected repository response for {full_name}")
        return result

    def branch_sha(self, full_name: str, branch: str) -> str:
        owner, name = full_name.split("/", 1)
        encoded_branch = urllib.parse.quote(branch, safe="")
        result = self.http.request(
            "GET",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                f"/commits/{encoded_branch}"
            ),
        )
        sha = result.get("sha") if isinstance(result, dict) else None
        if not isinstance(sha, str) or not SHA_RE.fullmatch(sha):
            raise StewardError(f"cannot resolve exact head for {full_name}@{branch}")
        return sha

    def open_pulls(self, full_name: str) -> list[dict[str, Any]]:
        owner, name = full_name.split("/", 1)
        result = self.http.request(
            "GET",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                "/pulls?state=open&per_page=100"
            ),
        )
        return [item for item in result if isinstance(item, dict)]

    def create_pull(
        self,
        full_name: str,
        *,
        title: str,
        head: str,
        base: str,
        body: str,
    ) -> dict[str, Any]:
        owner, name = full_name.split("/", 1)
        result = self.http.request(
            "POST",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}/pulls"
            ),
            payload={"title": title, "head": head, "base": base, "body": body},
        )
        if not isinstance(result, dict):
            raise StewardError(f"unexpected PR response for {full_name}")
        return result

    def update_pull(
        self,
        full_name: str,
        number: int,
        *,
        title: str | None = None,
        body: str | None = None,
        state: str | None = None,
    ) -> dict[str, Any]:
        owner, name = full_name.split("/", 1)
        payload = {
            key: value
            for key, value in {"title": title, "body": body, "state": state}.items()
            if value is not None
        }
        result = self.http.request(
            "PATCH",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                f"/pulls/{number}"
            ),
            payload=payload,
        )
        if not isinstance(result, dict):
            raise StewardError(f"unexpected PR update response for {full_name}#{number}")
        return result

    def comment(self, full_name: str, number: int, body: str) -> None:
        owner, name = full_name.split("/", 1)
        self.http.request(
            "POST",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                f"/issues/{number}/comments"
            ),
            payload={"body": body},
        )

    def add_labels(self, full_name: str, number: int, labels: Sequence[str]) -> None:
        owner, name = full_name.split("/", 1)
        for label in labels:
            try:
                self.http.request(
                    "POST",
                    self._url(
                        f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                        "/labels"
                    ),
                    payload={"name": label, "color": "5319e7"},
                )
            except StewardError as exc:
                if "HTTP 422" not in str(exc):
                    raise
        self.http.request(
            "POST",
            self._url(
                f"/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(name)}"
                f"/issues/{number}/labels"
            ),
            payload={"labels": list(labels)},
        )


class LinearClient:
    def __init__(self, token: str, team_fallback: str | None = None) -> None:
        self.token = token
        self.team_fallback = team_fallback
        self.endpoint = "https://api.linear.app/graphql"

    def graphql(self, query: str, variables: Mapping[str, Any]) -> dict[str, Any]:
        request = urllib.request.Request(
            self.endpoint,
            data=json.dumps({"query": query, "variables": variables}).encode(),
            headers={
                "Authorization": self.token,
                "Content-Type": "application/json",
                "User-Agent": JOB_MARKER,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = json.loads(response.read())
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode(errors="replace")
            raise StewardError(
                f"Linear HTTP {exc.code}: {redact(raw[-1000:])}"
            ) from exc
        except urllib.error.URLError as exc:
            raise StewardError(f"Linear request failed: {exc.reason}") from exc
        if payload.get("errors"):
            raise StewardError(f"Linear GraphQL error: {payload['errors']}")
        data = payload.get("data")
        if not isinstance(data, dict):
            raise StewardError("Linear returned no data")
        return data

    def ensure_issue(
        self,
        *,
        project_id: str,
        title: str,
        description: str,
        marker: str,
        priority: int = 2,
    ) -> str:
        query = """
        query DependencyStewardProject($id: String!) {
          project(id: $id) { id teams { nodes { id } } }
          issues(first: 250, filter: { project: { id: { eq: $id } } }) {
            nodes { id identifier url title description }
          }
        }
        """
        data = self.graphql(query, {"id": project_id})
        for issue in (data.get("issues") or {}).get("nodes") or []:
            if marker in str(issue.get("description") or ""):
                return str(issue.get("url") or issue.get("identifier") or issue.get("id"))

        project = data.get("project") or {}
        teams = (project.get("teams") or {}).get("nodes") or []
        team_id = str(teams[0].get("id")) if teams else self.team_fallback
        if not team_id:
            raise StewardError(f"Linear project {project_id} has no resolvable team")
        mutation = """
        mutation DependencyStewardIssue($input: IssueCreateInput!) {
          issueCreate(input: $input) { success issue { id identifier url } }
        }
        """
        created = self.graphql(
            mutation,
            {
                "input": {
                    "teamId": team_id,
                    "projectId": project_id,
                    "title": title,
                    "description": description.rstrip() + f"\n\n`{marker}`\n",
                    "priority": priority,
                }
            },
        )
        result = created.get("issueCreate") or {}
        issue = result.get("issue") or {}
        if not result.get("success") or not issue:
            raise StewardError(f"Linear did not create issue: {created}")
        return str(issue.get("url") or issue.get("identifier") or issue.get("id"))
