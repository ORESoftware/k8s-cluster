#!/usr/bin/env python3
from __future__ import annotations

import base64
import json
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

REST = "https://api.github.com"
GRAPHQL = f"{REST}/graphql"
VERSION = "2022-11-28"
TRANSIENT = {429, 500, 502, 503, 504}

PROJECT_QUERY = """
query($login:String!,$number:Int!,$after:String){
 organization(login:$login){projectV2(number:$number){
  id number title url closed
  fields(first:50){nodes{__typename ... on ProjectV2SingleSelectField{id name options{id name}}}}
  items(first:100,after:$after){nodes{id content{__typename ... on DraftIssue{id title body} ... on Issue{id title body url} ... on PullRequest{id title body url}}} pageInfo{hasNextPage endCursor}}
 }}
}
"""
ADD_DRAFT = """
mutation($projectId:ID!,$title:String!,$body:String!){
 addProjectV2DraftIssue(input:{projectId:$projectId,title:$title,body:$body}){projectItem{id}}
}
"""
SET_STATUS = """
mutation($projectId:ID!,$itemId:ID!,$fieldId:ID!,$optionId:String!){
 updateProjectV2ItemFieldValue(input:{projectId:$projectId,itemId:$itemId,fieldId:$fieldId,value:{singleSelectOptionId:$optionId}}){projectV2Item{id}}
}
"""


class ApiError(RuntimeError):
    pass


def bounded(value: object, limit: int = 350) -> str:
    text = " ".join(str(value).replace("\r", " ").replace("\n", " ").split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def _request(method: str, url: str, token: str, body: dict[str, Any] | None = None) -> dict[str, Any]:
    encoded = json.dumps(body).encode() if body is not None else None
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "User-Agent": "oresoftware-linear-project-sync/1.0",
        "X-GitHub-Api-Version": VERSION,
    }
    if encoded is not None:
        headers["Content-Type"] = "application/json"
    for attempt in range(4):
        try:
            with urlopen(Request(url, data=encoded, method=method, headers=headers), timeout=30) as response:
                parsed = json.loads(response.read().decode() or "{}")
                if not isinstance(parsed, dict):
                    raise ApiError(f"GitHub returned non-object JSON from {url}")
                return parsed
        except HTTPError as exc:
            raw = exc.read().decode(errors="replace")
            if exc.code in TRANSIENT and attempt < 3:
                time.sleep(min(2**attempt, 8))
                continue
            try:
                detail = json.loads(raw).get("message", raw)
            except json.JSONDecodeError:
                detail = raw
            raise ApiError(f"GitHub HTTP {exc.code} for {url}: {bounded(detail)}") from exc
        except URLError as exc:
            if attempt < 3:
                time.sleep(2**attempt)
                continue
            raise ApiError(f"GitHub network error for {url}: {bounded(exc.reason)}") from exc
    raise ApiError(f"GitHub request exhausted retries for {url}")


def graphql(token: str, query: str, variables: dict[str, Any]) -> dict[str, Any]:
    response = _request("POST", GRAPHQL, token, {"query": query, "variables": variables})
    if response.get("errors"):
        messages = "; ".join(bounded(error.get("message", error)) for error in response["errors"])
        raise ApiError(f"GitHub GraphQL error: {messages}")
    data = response.get("data")
    if not isinstance(data, dict):
        raise ApiError("GitHub GraphQL response has no data object")
    return data


def _b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def make_app_jwt(app_id: str, private_key: str) -> str:
    if not app_id.isdigit() or "PRIVATE KEY-----" not in private_key:
        raise ApiError("GitHub App ID/private key secret is invalid")
    now = int(time.time())
    header = _b64(b'{"alg":"RS256","typ":"JWT"}')
    payload = _b64(json.dumps({"iat": now - 60, "exp": now + 540, "iss": int(app_id)}, separators=(",", ":")).encode())
    unsigned = f"{header}.{payload}"
    with tempfile.TemporaryDirectory(prefix="github-app-") as directory:
        key = Path(directory) / "key.pem"
        key.write_text(private_key.rstrip() + "\n")
        key.chmod(0o600)
        try:
            signed = subprocess.run(
                ["openssl", "dgst", "-sha256", "-sign", str(key)],
                input=unsigned.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=True,
            ).stdout
        except (FileNotFoundError, subprocess.CalledProcessError) as exc:
            raise ApiError(f"Could not sign GitHub App JWT: {bounded(exc)}") from exc
    return f"{unsigned}.{_b64(signed)}"


def mint_org_token(app_jwt: str, organization: str) -> str:
    installation = _request("GET", f"{REST}/orgs/{organization}/installation", app_jwt)
    installation_id = installation.get("id")
    if not isinstance(installation_id, int) or installation_id <= 0:
        raise ApiError(f"No valid App installation for {organization}")
    minted = _request(
        "POST",
        f"{REST}/app/installations/{installation_id}/access_tokens",
        app_jwt,
        {"permissions": {"organization_projects": "write", "metadata": "read"}},
    )
    token = minted.get("token")
    permission = (minted.get("permissions") or {}).get("organization_projects")
    if not isinstance(token, str) or not token or permission not in {"write", "admin"}:
        raise ApiError(f"App installation for {organization} lacks organization_projects write")
    return token


def load_project(token: str, organization: str, number: int) -> dict[str, Any]:
    cursor = None
    project = None
    items: list[dict[str, Any]] = []
    while True:
        data = graphql(token, PROJECT_QUERY, {"login": organization, "number": number, "after": cursor})
        org = data.get("organization")
        current = org.get("projectV2") if isinstance(org, dict) else None
        if not isinstance(current, dict):
            raise ApiError(f"Project {organization}/projects/{number} is not visible")
        project = project or current
        connection = current.get("items") or {}
        items.extend(connection.get("nodes") or [])
        page = connection.get("pageInfo") or {}
        if not page.get("hasNextPage"):
            break
        cursor = page.get("endCursor")
        if not cursor:
            raise ApiError(f"Project pagination for {organization} returned no cursor")
    assert project is not None
    project["all_items"] = items
    return project


def add_draft(token: str, project_id: str, title: str, body: str) -> str:
    data = graphql(token, ADD_DRAFT, {"projectId": project_id, "title": title, "body": body})
    item_id = ((data.get("addProjectV2DraftIssue") or {}).get("projectItem") or {}).get("id")
    if not isinstance(item_id, str) or not item_id:
        raise ApiError("addProjectV2DraftIssue returned no item ID")
    return item_id


def set_status(token: str, project_id: str, item_id: str, field_id: str, option_id: str) -> None:
    graphql(token, SET_STATUS, {"projectId": project_id, "itemId": item_id, "fieldId": field_id, "optionId": option_id})
