"""Bounded GitHub, Linear, worker, and repair-provider clients."""

from __future__ import annotations

import contextlib
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import asdict
from typing import Any, Mapping

from .model import (
    GITHUB_API_URL,
    LINEAR_GRAPHQL_URL,
    SHA_RE,
    USER_AGENT,
    BotError,
    Candidate,
    DependencyEdge,
    Policy,
    PortfolioProject,
    Repository,
    SemVer,
    TestOutcome,
    worker_repository_url,
)

class JsonHttpClient:
    def __init__(self, *, token: str | None = None, auth_header: str = "Bearer {token}") -> None:
        self.token = token or ""
        self.auth_header = auth_header

    def request_json(
        self,
        method: str,
        url: str,
        payload: Mapping[str, Any] | None = None,
        headers: Mapping[str, str] | None = None,
        timeout: int = 60,
    ) -> Any:
        request_headers = {
            "Accept": "application/vnd.github+json, application/json",
            "User-Agent": USER_AGENT,
        }
        if self.token:
            request_headers["Authorization"] = self.auth_header.format(token=self.token)
        if payload is not None:
            request_headers["Content-Type"] = "application/json"
        if headers:
            request_headers.update(headers)
        request = urllib.request.Request(
            url,
            data=None if payload is None else json.dumps(payload).encode("utf-8"),
            headers=request_headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                body = response.read()
        except urllib.error.HTTPError as exc:
            body = exc.read(4000).decode("utf-8", "replace")
            raise BotError(f"HTTP {exc.code} for {method} {url}: {body[:4000]}") from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise BotError(f"request failed for {method} {url}: {exc}") from exc
        if not body:
            return None
        try:
            return json.loads(body)
        except json.JSONDecodeError as exc:
            raise BotError(f"provider returned non-JSON for {method} {url}") from exc

    def request_text(
        self,
        method: str,
        url: str,
        headers: Mapping[str, str] | None = None,
        timeout: int = 60,
    ) -> str:
        request_headers = {"Accept": "text/plain", "User-Agent": USER_AGENT}
        if self.token:
            request_headers["Authorization"] = self.auth_header.format(token=self.token)
        if headers:
            request_headers.update(headers)
        request = urllib.request.Request(url, headers=request_headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as exc:
            body = exc.read(4000).decode("utf-8", "replace")
            raise BotError(f"HTTP {exc.code} for {method} {url}: {body[:4000]}") from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise BotError(f"request failed for {method} {url}: {exc}") from exc


class GitHubClient:
    def __init__(self, token: str) -> None:
        self.token = token
        self.http = JsonHttpClient(token=token)

    def _url(self, path: str, query: Mapping[str, str | int] | None = None) -> str:
        url = f"{GITHUB_API_URL}{path}"
        if query:
            url += "?" + urllib.parse.urlencode(query)
        return url

    def list_org_repositories(self, org: str, limit: int) -> list[Repository]:
        repositories: list[Repository] = []
        page = 1
        while len(repositories) < limit:
            value = self.http.request_json(
                "GET",
                self._url(
                    f"/orgs/{urllib.parse.quote(org)}/repos",
                    {"type": "all", "sort": "full_name", "per_page": 100, "page": page},
                ),
            )
            if not isinstance(value, list):
                raise BotError(f"GitHub returned invalid repository list for {org}")
            for item in value:
                if not isinstance(item, Mapping):
                    continue
                full_name = item.get("full_name")
                default_branch = item.get("default_branch")
                clone_url = item.get("clone_url")
                if all(isinstance(part, str) and part for part in (full_name, default_branch, clone_url)):
                    repositories.append(
                        Repository(
                            full_name=full_name,
                            default_branch=default_branch,
                            clone_url=clone_url,
                            archived=bool(item.get("archived")),
                            fork=bool(item.get("fork")),
                        )
                    )
                    if len(repositories) >= limit:
                        break
            if len(value) < 100:
                break
            page += 1
        return repositories

    def repository(self, full_name: str) -> Repository:
        value = self.http.request_json("GET", self._url(f"/repos/{full_name}"))
        if not isinstance(value, Mapping):
            raise BotError(f"GitHub returned invalid repository metadata for {full_name}")
        return Repository(
            full_name=str(value["full_name"]),
            default_branch=str(value["default_branch"]),
            clone_url=str(value["clone_url"]),
            archived=bool(value.get("archived")),
            fork=bool(value.get("fork")),
        )

    def branch_sha(self, full_name: str, branch: str) -> str:
        value = self.http.request_json(
            "GET",
            self._url(
                f"/repos/{full_name}/branches/{urllib.parse.quote(branch, safe='')}"
            ),
        )
        sha = value.get("commit", {}).get("sha") if isinstance(value, Mapping) else None
        if not isinstance(sha, str) or not SHA_RE.fullmatch(sha):
            raise BotError(f"GitHub omitted branch SHA for {full_name}@{branch}")
        return sha

    def tags(self, full_name: str, maximum: int = 500) -> list[tuple[SemVer, str, str]]:
        output: list[tuple[SemVer, str, str]] = []
        page = 1
        while len(output) < maximum:
            value = self.http.request_json(
                "GET",
                self._url(f"/repos/{full_name}/tags", {"per_page": 100, "page": page}),
            )
            if not isinstance(value, list):
                raise BotError(f"GitHub returned invalid tags for {full_name}")
            for item in value:
                if not isinstance(item, Mapping):
                    continue
                name = item.get("name")
                sha = item.get("commit", {}).get("sha")
                if isinstance(name, str) and isinstance(sha, str):
                    version = SemVer.parse(name)
                    if version is not None and SHA_RE.fullmatch(sha):
                        output.append((version, name, sha))
                        if len(output) >= maximum:
                            break
            if len(value) < 100:
                break
            page += 1
        return output

    def compare_commits(
        self,
        full_name: str,
        base_sha: str,
        head_sha: str,
        maximum: int,
    ) -> tuple[str, list[str], int]:
        commits: list[str] = []
        page = 1
        total_commits = 0
        status = "unknown"
        while len(commits) < maximum:
            value = self.http.request_json(
                "GET",
                self._url(
                    f"/repos/{full_name}/compare/{base_sha}...{head_sha}",
                    {"per_page": 100, "page": page},
                ),
            )
            if not isinstance(value, Mapping):
                raise BotError(f"GitHub returned invalid comparison for {full_name}")
            status = str(value.get("status", "unknown"))
            total_commits = int(value.get("total_commits", 0))
            page_commits = value.get("commits")
            if not isinstance(page_commits, list):
                raise BotError(f"GitHub comparison omitted commits for {full_name}")
            for item in page_commits:
                sha = item.get("sha") if isinstance(item, Mapping) else None
                if isinstance(sha, str) and SHA_RE.fullmatch(sha):
                    commits.append(sha)
                    if len(commits) >= maximum:
                        break
            if len(page_commits) < 100 or len(commits) >= total_commits:
                break
            page += 1
        return status, commits, total_commits

    def open_pull_requests(self, full_name: str) -> list[Mapping[str, Any]]:
        value = self.http.request_json(
            "GET", self._url(f"/repos/{full_name}/pulls", {"state": "open", "per_page": 100})
        )
        if not isinstance(value, list):
            raise BotError(f"GitHub returned invalid pull-request list for {full_name}")
        return [item for item in value if isinstance(item, Mapping)]

    def create_pull_request(
        self,
        full_name: str,
        *,
        title: str,
        body: str,
        head: str,
        base: str,
        draft: bool,
    ) -> Mapping[str, Any]:
        value = self.http.request_json(
            "POST",
            self._url(f"/repos/{full_name}/pulls"),
            {"title": title, "body": body, "head": head, "base": base, "draft": draft},
        )
        if not isinstance(value, Mapping):
            raise BotError(f"GitHub did not return the created pull request for {full_name}")
        return value

    def update_pull_request(
        self,
        full_name: str,
        number: int,
        *,
        title: str | None = None,
        body: str | None = None,
        state: str | None = None,
    ) -> Mapping[str, Any]:
        payload = {key: value for key, value in {"title": title, "body": body, "state": state}.items() if value is not None}
        value = self.http.request_json(
            "PATCH", self._url(f"/repos/{full_name}/pulls/{number}"), payload
        )
        if not isinstance(value, Mapping):
            raise BotError(f"GitHub did not return updated pull request {full_name}#{number}")
        return value

    def comment_issue(self, full_name: str, number: int, body: str) -> None:
        self.http.request_json(
            "POST", self._url(f"/repos/{full_name}/issues/{number}/comments"), {"body": body}
        )


class LinearClient:
    def __init__(self, token: str) -> None:
        self.http = JsonHttpClient(token=token, auth_header="{token}")

    def graphql(self, query: str, variables: Mapping[str, Any]) -> Mapping[str, Any]:
        value = self.http.request_json(
            "POST", LINEAR_GRAPHQL_URL, {"query": query, "variables": variables}
        )
        if not isinstance(value, Mapping):
            raise BotError("Linear returned a non-object response")
        errors = value.get("errors")
        if errors:
            raise BotError(f"Linear GraphQL error: {json.dumps(errors)[:3000]}")
        data = value.get("data")
        if not isinstance(data, Mapping):
            raise BotError("Linear response omitted data")
        return data

    def ensure_issue(
        self,
        project: PortfolioProject,
        *,
        marker: str,
        title: str,
        description: str,
    ) -> str:
        query = """
        query PortfolioDependencyProject($id: String!) {
          project(id: $id) {
            id
            name
            teams { nodes { id name } }
            issues(first: 250) {
              nodes { id identifier title description url state { type } }
            }
          }
          teams(first: 50) { nodes { id name } }
        }
        """
        data = self.graphql(query, {"id": project.linear_project_id})
        project_value = data.get("project")
        if not isinstance(project_value, Mapping):
            raise BotError(f"Linear project is not visible: {project.linear_project_id}")
        issues_value = project_value.get("issues")
        issues = issues_value.get("nodes", []) if isinstance(issues_value, Mapping) else []
        for issue in issues if isinstance(issues, list) else []:
            if not isinstance(issue, Mapping):
                continue
            state = issue.get("state")
            state_type = state.get("type") if isinstance(state, Mapping) else None
            haystack = f"{issue.get('title', '')}\n{issue.get('description', '')}"
            if marker in haystack and state_type not in {"completed", "canceled"}:
                url = issue.get("url")
                if isinstance(url, str):
                    return url

        team_nodes: list[Mapping[str, Any]] = []
        project_teams = project_value.get("teams")
        if isinstance(project_teams, Mapping) and isinstance(project_teams.get("nodes"), list):
            team_nodes = [node for node in project_teams["nodes"] if isinstance(node, Mapping)]
        if not team_nodes:
            all_teams = data.get("teams")
            if isinstance(all_teams, Mapping) and isinstance(all_teams.get("nodes"), list):
                team_nodes = [node for node in all_teams["nodes"] if isinstance(node, Mapping)]
        team_id = team_nodes[0].get("id") if team_nodes else None
        if not isinstance(team_id, str):
            raise BotError(f"Linear project {project.linear_project_id} has no writable team")

        mutation = """
        mutation PortfolioDependencyIssueCreate($input: IssueCreateInput!) {
          issueCreate(input: $input) {
            success
            issue { id identifier url }
          }
        }
        """
        result = self.graphql(
            mutation,
            {
                "input": {
                    "teamId": team_id,
                    "projectId": project.linear_project_id,
                    "title": title,
                    "description": description,
                }
            },
        ).get("issueCreate")
        issue = result.get("issue") if isinstance(result, Mapping) else None
        if not isinstance(result, Mapping) or result.get("success") is not True or not isinstance(issue, Mapping):
            raise BotError("Linear issueCreate did not report success")
        url = issue.get("url")
        if not isinstance(url, str):
            raise BotError("Linear issueCreate omitted issue URL")
        return url


class WorkerClient:
    def __init__(self, base_url: str, auth_secret: str, policy: Policy) -> None:
        self.base_url = base_url.rstrip("/")
        self.auth_secret = auth_secret
        self.policy = policy
        self.http = JsonHttpClient()

    @property
    def headers(self) -> Mapping[str, str]:
        return {"x-server-auth": self.auth_secret}

    def test_branch(
        self,
        repository: Repository,
        branch: str,
        profile: str,
        request_id: str,
    ) -> TestOutcome:
        value = self.http.request_json(
            "POST",
            f"{self.base_url}/builds",
            {
                "schemaVersion": "build-server.v1",
                "jobKind": "run-profile",
                "repoUrl": worker_repository_url(repository),
                "gitRef": branch,
                "profile": profile,
                "requestId": request_id,
            },
            self.headers,
        )
        if not isinstance(value, Mapping):
            raise BotError("gha-indie-worker returned invalid submission response")
        job_id = value.get("id")
        if not isinstance(job_id, str):
            raise BotError("gha-indie-worker omitted job id")
        deadline = time.monotonic() + self.policy.worker_timeout_seconds
        last_status = "Queued"
        detail = ""
        while time.monotonic() < deadline:
            job = self.http.request_json(
                "GET", f"{self.base_url}/builds/{urllib.parse.quote(job_id)}", headers=self.headers
            )
            if not isinstance(job, Mapping):
                raise BotError("gha-indie-worker returned invalid job status")
            last_status = str(job.get("status", "Unknown"))
            error = job.get("error")
            if isinstance(error, str):
                detail = error
            if last_status in {"Succeeded", "Failed"}:
                logs = ""
                if last_status == "Failed":
                    with contextlib.suppress(BotError):
                        logs = self.http.request_text(
                            "GET",
                            f"{self.base_url}/builds/{urllib.parse.quote(job_id)}/logs",
                            self.headers,
                        )[-16000:]
                return TestOutcome(
                    passed=last_status == "Succeeded",
                    profile=profile,
                    job_id=job_id,
                    status=last_status.lower(),
                    detail=detail,
                    logs=logs,
                )
            time.sleep(self.policy.worker_poll_seconds)
        return TestOutcome(
            passed=False,
            profile=profile,
            job_id=job_id,
            status="timeout",
            detail="gha-indie-worker test timed out",
        )


class RepairClient:
    def __init__(self, endpoint: str, token: str | None) -> None:
        self.endpoint = endpoint
        self.http = JsonHttpClient(token=token, auth_header="Bearer {token}")

    def repair(
        self,
        repository: Repository,
        branch: str,
        edge: DependencyEdge,
        candidate: Candidate,
        outcome: TestOutcome,
        attempt: int,
    ) -> bool:
        value = self.http.request_json(
            "POST",
            self.endpoint,
            {
                "schemaVersion": "portfolio-dependency-repair.v1",
                "attempt": attempt,
                "repository": repository.full_name,
                "defaultBranch": repository.default_branch,
                "branch": branch,
                "dependency": asdict(edge) | {"stableKey": edge.stable_key},
                "candidate": asdict(candidate),
                "test": asdict(outcome),
                "constraints": {
                    "minorOnly": True,
                    "patchOnly": False,
                    "major": False,
                    "doNotForcePushOutsideBotPrefix": True,
                },
            },
        )
        return isinstance(value, Mapping) and value.get("status") == "repaired"
