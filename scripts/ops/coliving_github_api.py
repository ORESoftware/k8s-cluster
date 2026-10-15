"""Minimal fail-closed GitHub REST client for repository publication."""
from __future__ import annotations
import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Iterable

API = "https://api.github.com"
API_VERSION = "2022-11-28"
USER_AGENT = "ores-coliving-repository-publisher/1"

@dataclass(frozen=True)
class ApiResponse:
    status: int
    payload: Any


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


class GitHubApi:
    def __init__(self, token: str) -> None:
        self._token = token
        self._opener = urllib.request.build_opener(NoRedirect())

    def request(self, method: str, path: str, payload: object | None = None) -> ApiResponse:
        if not path.startswith("/") or ".." in path:
            raise ValueError(f"unsafe GitHub API path: {path!r}")
        data = None if payload is None else json.dumps(payload, separators=(",", ":")).encode("utf-8")
        request = urllib.request.Request(
            API + path,
            data=data,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "X-GitHub-Api-Version": API_VERSION,
                "User-Agent": USER_AGENT,
                **({"Content-Type": "application/json"} if data is not None else {}),
            },
        )
        try:
            with self._opener.open(request, timeout=45) as response:
                raw = response.read()
                return ApiResponse(response.status, json.loads(raw.decode("utf-8")) if raw else None)
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                decoded: Any = json.loads(raw.decode("utf-8")) if raw else None
            except (UnicodeDecodeError, json.JSONDecodeError):
                decoded = None
            return ApiResponse(error.code, decoded)
        except (urllib.error.URLError, TimeoutError) as error:
            raise RuntimeError(f"GitHub API transport failed for {method} {path}") from error

    def require_object(self, response: ApiResponse, operation: str, expected: Iterable[int]) -> dict[str, Any]:
        if response.status not in set(expected):
            message = response.payload.get("message") if isinstance(response.payload, dict) else None
            raise RuntimeError(f"{operation} failed at HTTP {response.status}: {message or 'no diagnostic'}")
        if not isinstance(response.payload, dict):
            raise RuntimeError(f"{operation} returned a non-object payload")
        return response.payload

    def require_list(self, response: ApiResponse, operation: str, expected: Iterable[int]) -> list[Any]:
        if response.status not in set(expected):
            message = response.payload.get("message") if isinstance(response.payload, dict) else None
            raise RuntimeError(f"{operation} failed at HTTP {response.status}: {message or 'no diagnostic'}")
        if not isinstance(response.payload, list):
            raise RuntimeError(f"{operation} returned a non-list payload")
        return response.payload
