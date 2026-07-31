from __future__ import annotations

from fastapi.routing import APIRoute
from fastapi.testclient import TestClient

from dd_ai_ml_pipeline import Config
from dd_ai_ml_pipeline_api import (
    PUBLIC_VISIBILITY,
    build_openapi_documents,
    canonical_openapi_bytes,
    create_app,
)
from dd_nats_subject_defs import (
    ML_DEAD_LETTER_SUBJECT,
    ML_FEATURES_SUBJECT,
    RUNTIME_EVENTS_SUBJECT,
    TELEMETRY_MDP_SUBJECT,
    TELEMETRY_RAW_QUEUE_GROUP,
    TELEMETRY_RAW_SUBJECT,
)


def make_app():
    return create_app(
        config=Config(server_auth_secret="service-secret", allow_unauthenticated=False),
        contract_only=True,
    )


def test_contract_shim_matches_deployed_nats_defaults() -> None:
    """Keep the CI-only shim aligned without replacing production generation."""

    assert (
        ML_DEAD_LETTER_SUBJECT,
        ML_FEATURES_SUBJECT,
        RUNTIME_EVENTS_SUBJECT,
        TELEMETRY_MDP_SUBJECT,
        TELEMETRY_RAW_QUEUE_GROUP,
        TELEMETRY_RAW_SUBJECT,
    ) == (
        "dd.remote.ml.deadletter",
        "dd.remote.ml.features",
        "dd.remote.events",
        "dd.remote.telemetry.mdp",
        "dd-ai-ml-pipeline",
        "dd.remote.telemetry.raw",
    )


def test_route_set_equals_internal_openapi_contract() -> None:
    app = make_app()
    _, internal = build_openapi_documents(app)
    runtime = {
        (route.path, method.lower())
        for route in app.routes
        if isinstance(route, APIRoute)
        for method in route.methods
        if method not in {"HEAD", "OPTIONS"}
    }
    documented = {
        (path, method)
        for path, item in internal["paths"].items()
        for method in item
        if method in {"get", "post", "put", "patch", "delete"}
    }
    assert runtime == documented


def test_exports_are_deterministic_and_public_is_fail_closed() -> None:
    app = make_app()
    assert canonical_openapi_bytes(app, PUBLIC_VISIBILITY) == canonical_openapi_bytes(
        app, PUBLIC_VISIBILITY
    )
    public, internal = build_openapi_documents(app)
    assert set(public["paths"]) < set(internal["paths"])
    assert "/healthz" in public["paths"]
    assert "/analyze" not in public["paths"]
    assert "/internal/runtime-config" not in public["paths"]
    assert "securitySchemes" not in public.get("components", {})
    assert internal["paths"]["/analyze"]["post"]["security"] == [{"ServiceAuth": []}]


def test_public_docs_aliases_serve_exact_canonical_bytes() -> None:
    app = make_app()
    expected = canonical_openapi_bytes(app, PUBLIC_VISIBILITY)
    with TestClient(app) as client:
        assert client.get("/openapi.json").content == expected
        assert client.get("/api/docs.json").content == expected


def test_auth_validation_and_domain_errors_match_documented_contract() -> None:
    app = make_app()
    with TestClient(app) as client:
        assert client.post("/analyze", json={"metrics": {"latency": 1}}).status_code == 401
        invalid = client.post(
            "/analyze",
            headers={"X-Server-Auth": "service-secret"},
            json={"metrics": {}},
        )
        assert invalid.status_code == 400
        assert invalid.json()["ok"] is False
        valid = client.post(
            "/analyze",
            headers={"X-Server-Auth": "service-secret"},
            json={"requestId": "contract-test", "metrics": {"latency": 1}},
        )
        assert valid.status_code == 200
        assert valid.json()["requestId"] == "contract-test"


def test_every_local_schema_ref_resolves() -> None:
    app = make_app()
    for document in build_openapi_documents(app):
        schemas = document.get("components", {}).get("schemas", {})

        def walk(value):
            if isinstance(value, dict):
                ref = value.get("$ref")
                if isinstance(ref, str) and ref.startswith("#/components/schemas/"):
                    assert ref.rsplit("/", 1)[-1] in schemas
                for nested in value.values():
                    walk(nested)
            elif isinstance(value, list):
                for nested in value:
                    walk(nested)

        walk(document)
