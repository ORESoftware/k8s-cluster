#!/usr/bin/env python3
from __future__ import annotations

import copy
import json
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Literal

import uvicorn
from fastapi import Body, Depends, FastAPI, Header, HTTPException, Request, Response
from fastapi.exceptions import RequestValidationError
from fastapi.openapi.docs import get_swagger_ui_html
from fastapi.responses import HTMLResponse, JSONResponse, PlainTextResponse
from pydantic import BaseModel, ConfigDict, Field
from starlette.concurrency import run_in_threadpool

from dd_ai_ml_pipeline import (
    MAX_ACTION_IMPACTS,
    MAX_BODY_BYTES,
    MAX_REQUEST_ID_BYTES,
    MAX_SIGNAL_WEIGHT,
    MAX_SIGNALS,
    MAX_TELEMETRY_WINDOW_MS,
    Config,
    PipelineApp,
    RuntimeConfigClient,
    SERVICE_NAME,
    constant_time_equals,
    init_telemetry,
)

API_VERSION = "1.0.0"
PUBLIC_VISIBILITY = "public"
INTERNAL_VISIBILITY = "internal"
GENERATED_DIR = Path(__file__).resolve().parents[1] / "generated"


class ApiModel(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="forbid")


class ErrorResponse(ApiModel):
    ok: Literal[False] = False
    error: str


class HealthResponse(ApiModel):
    ok: Literal[True] = True
    service: str


class ReadinessResponse(ApiModel):
    ok: bool
    service: str
    auth_ready: bool = Field(alias="authReady")
    nats_configured: bool = Field(alias="natsConfigured")
    generated_at_ms: int = Field(alias="generatedAtMs")


class StatusResponse(ApiModel):
    ok: Literal[True] = True
    service: str
    nats_configured: bool = Field(alias="natsConfigured")
    nats_url: str = Field(alias="natsUrl")
    auth_required: bool = Field(alias="authRequired")
    metrics: dict[str, int]
    generated_at_ms: int = Field(alias="generatedAtMs")


class ActionImpact(ApiModel):
    action: str = "observe"
    delta: float = Field(default=0.0, ge=-1.0, le=1.0)
    confidence: float = Field(default=1.0, ge=0.0, le=1.0)


class SignalInput(ApiModel):
    name: str = Field(min_length=1)
    value: float
    service: str | None = None
    layer: str | None = None
    baseline: float | None = None
    target: float | None = None
    warning: float | None = None
    critical: float | None = None
    higher_is_better: bool | None = Field(default=None, alias="higherIsBetter")
    weight: float | None = Field(default=None, ge=0.0, le=MAX_SIGNAL_WEIGHT)
    action_impacts: list[ActionImpact] | None = Field(
        default=None,
        alias="actionImpacts",
        max_length=MAX_ACTION_IMPACTS,
    )


class TelemetryBody(BaseModel):
    """Typed HTTP envelope while retaining customer-defined metric names."""

    model_config = ConfigDict(populate_by_name=True, extra="allow")

    request_id: str | None = Field(default=None, alias="requestId", max_length=MAX_REQUEST_ID_BYTES)
    service: str | None = None
    scope: str | None = None
    window_ms: int | None = Field(
        default=None,
        alias="windowMs",
        ge=1,
        le=MAX_TELEMETRY_WINDOW_MS,
    )
    action: str | None = None
    last_action: str | None = Field(default=None, alias="lastAction")
    signals: list[SignalInput] | None = Field(default=None, max_length=MAX_SIGNALS)
    metrics: dict[str, Any] | None = None
    values: dict[str, Any] | None = None


class TelemetryRequest(TelemetryBody):
    telemetry: TelemetryBody | None = None


class MdpTelemetryResponse(ApiModel):
    request_id: str = Field(alias="requestId")
    scope: str
    window_ms: int = Field(alias="windowMs")
    signals: list[dict[str, Any]]
    actions: list[str]
    gamma: float
    tolerance: float
    max_iterations: int = Field(alias="maxIterations")


class PublishedState(ApiModel):
    features: bool
    mdp: bool
    events: bool


class AnalysisResponse(ApiModel):
    ok: Literal[True] = True
    request_id: str = Field(alias="requestId")
    kind: str
    source: str
    service: str
    scope: str
    window_ms: int = Field(alias="windowMs")
    state: str
    risk: float
    recommended_next: str = Field(alias="recommendedNext")
    features: list[dict[str, Any]]
    anomalies: list[dict[str, Any]]
    reward_estimate: float = Field(alias="rewardEstimate")
    transition_model: list[dict[str, Any]] = Field(alias="transitionModel")
    mdp_telemetry: MdpTelemetryResponse = Field(alias="mdpTelemetry")
    published: PublishedState
    generated_at_ms: int = Field(alias="generatedAtMs")


class RuntimeConfigEntry(ApiModel):
    key: str
    value: Any = None


class RuntimeConfigSnapshotInput(ApiModel):
    snapshot_version: int = Field(default=0, alias="snapshotVersion", ge=0)
    entries: list[RuntimeConfigEntry] = Field(default_factory=list)


class RuntimeConfigApplyRequest(ApiModel):
    snapshot: RuntimeConfigSnapshotInput
    push_id: str | None = Field(default=None, alias="pushId")
    reason: str | None = None


class RuntimeConfigSnapshotResponse(ApiModel):
    service: str
    scope: str
    env: str
    snapshot_version: int = Field(alias="snapshotVersion")
    applied_at: str | None = Field(alias="appliedAt")
    entries: dict[str, Any]
    last_push_id: str | None = Field(alias="lastPushId")
    last_reason: str | None = Field(alias="lastReason")


class RuntimeConfigApplyResponse(ApiModel):
    ok: Literal[True] = True
    service: str
    applied_at: str = Field(alias="appliedAt")
    applied_version: int = Field(alias="appliedVersion")
    previous_version: int = Field(alias="previousVersion")
    stale: bool | None = None
    ignored_version: int | None = Field(default=None, alias="ignoredVersion")


class OkResponse(ApiModel):
    ok: Literal[True] = True


def _route_metadata(visibility: str, auth: str) -> dict[str, Any]:
    return {
        "x-dd-visibility": visibility,
        "x-dd-auth": auth,
        "x-dd-implementation": "fastapi-pydantic-v2",
    }


def _error_responses(*statuses: int) -> dict[int, dict[str, Any]]:
    descriptions = {
        400: "The request failed Pydantic or domain validation.",
        401: "The service credential is absent or invalid.",
        413: "The request body exceeds the configured bound.",
        500: "The service rejected an unexpected internal failure.",
    }
    return {
        status: {"model": ErrorResponse, "description": descriptions[status]}
        for status in statuses
    }


def _canonical_json_bytes(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode(
        "utf-8"
    )


def _collect_schema_refs(value: Any, refs: set[str]) -> None:
    if isinstance(value, dict):
        ref = value.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/components/schemas/"):
            refs.add(ref.rsplit("/", 1)[-1])
        for nested in value.values():
            _collect_schema_refs(nested, refs)
    elif isinstance(value, list):
        for nested in value:
            _collect_schema_refs(nested, refs)


def _prune_component_schemas(document: dict[str, Any]) -> None:
    components = document.get("components")
    if not isinstance(components, dict):
        return
    schemas = components.get("schemas")
    if not isinstance(schemas, dict):
        return

    required: set[str] = set()
    _collect_schema_refs(document.get("paths", {}), required)
    queue = list(required)
    while queue:
        name = queue.pop()
        schema = schemas.get(name)
        if schema is None:
            continue
        nested: set[str] = set()
        _collect_schema_refs(schema, nested)
        for dependency in nested:
            if dependency not in required:
                required.add(dependency)
                queue.append(dependency)
    components["schemas"] = {name: schemas[name] for name in sorted(required) if name in schemas}


def build_openapi_documents(app: FastAPI) -> tuple[dict[str, Any], dict[str, Any]]:
    internal = copy.deepcopy(app.openapi())
    internal["info"]["version"] = API_VERSION
    internal["jsonSchemaDialect"] = "https://json-schema.org/draft/2020-12/schema"
    internal.setdefault("components", {})["securitySchemes"] = {
        "ServiceAuth": {
            "type": "apiKey",
            "in": "header",
            "name": "X-Server-Auth",
            "description": "Service credential. The legacy `Auth` header remains accepted at runtime.",
        }
    }

    for path_item in internal.get("paths", {}).values():
        if not isinstance(path_item, dict):
            continue
        for operation in path_item.values():
            if not isinstance(operation, dict) or "responses" not in operation:
                continue
            operation["responses"].pop("422", None)
            if operation.get("x-dd-auth") == "service":
                operation["security"] = [{"ServiceAuth": []}]
            else:
                operation["security"] = []

    public = copy.deepcopy(internal)
    for path in list(public.get("paths", {})):
        path_item = public["paths"][path]
        for method in list(path_item):
            operation = path_item[method]
            if not isinstance(operation, dict):
                continue
            if operation.get("x-dd-visibility") != PUBLIC_VISIBILITY:
                del path_item[method]
        if not path_item:
            del public["paths"][path]
    public.get("components", {}).pop("securitySchemes", None)
    _prune_component_schemas(public)
    return public, internal


def canonical_openapi_bytes(app: FastAPI, visibility: str) -> bytes:
    public, internal = build_openapi_documents(app)
    if visibility == PUBLIC_VISIBILITY:
        return _canonical_json_bytes(public)
    if visibility == INTERNAL_VISIBILITY:
        return _canonical_json_bytes(internal)
    raise ValueError(f"unsupported OpenAPI visibility: {visibility}")


def export_openapi(app: FastAPI, output_dir: Path = GENERATED_DIR) -> tuple[Path, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    public_path = output_dir / "openapi.public.json"
    internal_path = output_dir / "openapi.internal.json"
    public_path.write_bytes(canonical_openapi_bytes(app, PUBLIC_VISIBILITY))
    internal_path.write_bytes(canonical_openapi_bytes(app, INTERNAL_VISIBILITY))
    return public_path, internal_path


def create_app(*, config: Config | None = None, contract_only: bool = False) -> FastAPI:
    service_config = config or Config()
    if not contract_only and not service_config.server_auth_secret and not service_config.allow_unauthenticated:
        raise RuntimeError("SERVER_AUTH_SECRET is required unless ML_ALLOW_UNAUTHENTICATED=true")

    pipeline = PipelineApp(service_config)
    runtime_config = RuntimeConfigClient()

    @asynccontextmanager
    async def lifespan(_: FastAPI):
        if not contract_only:
            pipeline.start_nats_consumer()
            runtime_config.start_registration_thread()
        yield

    app = FastAPI(
        title="ai-ml-pipeline API",
        version=API_VERSION,
        description=(
            "Online telemetry feature engineering, anomaly scoring, and MDP feature publishing. "
            "Runtime validation and OpenAPI originate from this FastAPI/Pydantic application factory."
        ),
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
        lifespan=lifespan,
    )
    app.state.pipeline = pipeline
    app.state.runtime_config = runtime_config
    app.state.contract_only = contract_only

    @app.middleware("http")
    async def service_contract_middleware(request: Request, call_next):  # type: ignore[no-untyped-def]
        pipeline.metrics.inc("requests_total")
        raw_length = request.headers.get("content-length")
        if raw_length is not None:
            try:
                length = int(raw_length)
            except ValueError:
                return JSONResponse(status_code=400, content={"ok": False, "error": "invalid Content-Length"})
            if length < 0:
                return JSONResponse(
                    status_code=400,
                    content={"ok": False, "error": "Content-Length must be non-negative"},
                )
            if length > MAX_BODY_BYTES:
                return JSONResponse(
                    status_code=413,
                    content={"ok": False, "error": f"request body must be at most {MAX_BODY_BYTES} bytes"},
                )
        response = await call_next(request)
        response.headers["Cache-Control"] = "no-store"
        response.headers["X-Content-Type-Options"] = "nosniff"
        return response

    @app.exception_handler(RequestValidationError)
    async def request_validation_handler(_: Request, error: RequestValidationError) -> JSONResponse:
        pipeline.metrics.inc("errors_total")
        first = error.errors()[0] if error.errors() else {"msg": "request validation failed"}
        location = ".".join(str(item) for item in first.get("loc", ()) if item != "body")
        message = str(first.get("msg", "request validation failed"))
        if location:
            message = f"{location}: {message}"
        return JSONResponse(status_code=400, content={"ok": False, "error": message})

    @app.exception_handler(HTTPException)
    async def http_exception_handler(_: Request, error: HTTPException) -> JSONResponse:
        message = error.detail if isinstance(error.detail, str) else "request rejected"
        return JSONResponse(status_code=error.status_code, content={"ok": False, "error": message})

    async def require_service_auth(request: Request) -> None:
        if service_config.server_auth_secret:
            values = request.headers.getlist("x-server-auth") + request.headers.getlist("auth")
            if any(constant_time_equals(value, service_config.server_auth_secret) for value in values):
                return
        elif service_config.allow_unauthenticated:
            return
        pipeline.metrics.inc("auth_failures_total")
        raise HTTPException(status_code=401, detail="unauthorized")

    async def require_runtime_config_auth(
        x_server_auth: str | None = Header(default=None, alias="X-Server-Auth"),
    ) -> None:
        if runtime_config.check_server_auth(x_server_auth):
            return
        raise HTTPException(status_code=401, detail="unauthorized")

    @app.get(
        "/",
        operation_id="describeAiMlPipeline",
        response_model=dict[str, Any],
        dependencies=[Depends(require_service_auth)],
        responses=_error_responses(401),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def describe_service() -> dict[str, Any]:
        return pipeline.descriptor()

    @app.get(
        "/healthz",
        operation_id="healthAiMlPipeline",
        response_model=HealthResponse,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def healthz() -> dict[str, Any]:
        return {"ok": True, "service": SERVICE_NAME}

    @app.get(
        "/readyz",
        operation_id="readyAiMlPipeline",
        response_model=ReadinessResponse,
        responses={503: {"model": ReadinessResponse, "description": "The service is not ready."}},
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def readyz(response: Response) -> dict[str, Any]:
        readiness = pipeline.readiness()
        if not readiness["ok"]:
            response.status_code = 503
        return readiness

    @app.get(
        "/metrics",
        operation_id="scrapeAiMlPipelineMetrics",
        response_class=PlainTextResponse,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def metrics() -> PlainTextResponse:
        return PlainTextResponse(pipeline.metrics.prometheus(), media_type="text/plain; version=0.0.4")

    @app.get(
        "/status",
        operation_id="getAiMlPipelineStatus",
        response_model=StatusResponse,
        dependencies=[Depends(require_service_auth)],
        responses=_error_responses(401),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def status() -> dict[str, Any]:
        return pipeline.status()

    async def analyze_payload(payload: TelemetryRequest, publish: bool) -> dict[str, Any]:
        body = payload.model_dump(by_alias=True, exclude_none=True)
        try:
            return await run_in_threadpool(pipeline.analyze, body, "http", publish)
        except ValueError as error:
            pipeline.metrics.inc("errors_total")
            raise HTTPException(status_code=400, detail=str(error)) from error

    @app.post(
        "/analyze",
        operation_id="analyzeTelemetry",
        response_model=AnalysisResponse,
        dependencies=[Depends(require_service_auth)],
        responses=_error_responses(400, 401, 413, 500),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def analyze(payload: TelemetryRequest = Body(...)) -> dict[str, Any]:
        return await analyze_payload(payload, False)

    @app.post(
        "/ingest",
        operation_id="ingestTelemetry",
        response_model=AnalysisResponse,
        dependencies=[Depends(require_service_auth)],
        responses=_error_responses(400, 401, 413, 500),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def ingest(payload: TelemetryRequest = Body(...)) -> dict[str, Any]:
        pipeline.metrics.inc("ingest_requests_total")
        return await analyze_payload(payload, True)

    @app.post(
        "/mdp/features",
        operation_id="buildMdpTelemetryFeatures",
        response_model=MdpTelemetryResponse,
        dependencies=[Depends(require_service_auth)],
        responses=_error_responses(400, 401, 413, 500),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def mdp_features(payload: TelemetryRequest = Body(...)) -> dict[str, Any]:
        result = await analyze_payload(payload, False)
        return result["mdpTelemetry"]

    @app.get(
        RuntimeConfigClient.SNAPSHOT_ROUTE,
        operation_id="getRuntimeConfigSnapshot",
        response_model=RuntimeConfigSnapshotResponse,
        dependencies=[Depends(require_runtime_config_auth)],
        responses=_error_responses(401),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def runtime_config_snapshot() -> dict[str, Any]:
        return runtime_config.snapshot()

    @app.post(
        RuntimeConfigClient.APPLY_ROUTE,
        operation_id="applyRuntimeConfigSnapshot",
        response_model=RuntimeConfigApplyResponse,
        dependencies=[Depends(require_runtime_config_auth)],
        responses=_error_responses(400, 401),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def apply_runtime_config(payload: RuntimeConfigApplyRequest = Body(...)) -> dict[str, Any]:
        try:
            return runtime_config.apply(payload.model_dump(by_alias=True, exclude_none=True))
        except ValueError as error:
            raise HTTPException(status_code=400, detail=str(error)) from error

    @app.post(
        RuntimeConfigClient.RESET_ROUTE,
        operation_id="resetRuntimeConfigSnapshot",
        response_model=OkResponse,
        dependencies=[Depends(require_runtime_config_auth)],
        responses=_error_responses(401),
        openapi_extra=_route_metadata(INTERNAL_VISIBILITY, "service"),
    )
    async def reset_runtime_config() -> dict[str, bool]:
        runtime_config.reset()
        return {"ok": True}

    @app.get(
        "/openapi.json",
        operation_id="getPublicOpenApi",
        response_class=Response,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def public_openapi() -> Response:
        return Response(
            canonical_openapi_bytes(app, PUBLIC_VISIBILITY),
            media_type="application/json",
        )

    @app.get(
        "/api/docs.json",
        operation_id="getPublicApiContract",
        response_class=Response,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def public_api_contract() -> Response:
        return Response(
            canonical_openapi_bytes(app, PUBLIC_VISIBILITY),
            media_type="application/json",
        )

    @app.get(
        "/docs/api",
        operation_id="getHumanApiDocs",
        response_class=HTMLResponse,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def docs_api() -> HTMLResponse:
        return get_swagger_ui_html(openapi_url="/openapi.json", title=f"{SERVICE_NAME} API")

    @app.get(
        "/api/docs",
        operation_id="getHumanApiDocsAlias",
        response_class=HTMLResponse,
        openapi_extra=_route_metadata(PUBLIC_VISIBILITY, "public"),
    )
    async def api_docs_alias() -> HTMLResponse:
        return get_swagger_ui_html(openapi_url="/openapi.json", title=f"{SERVICE_NAME} API")

    return app


def create_contract_app() -> FastAPI:
    return create_app(
        config=Config(
            server_auth_secret="contract-export-only",
            allow_unauthenticated=False,
            nats_url="nats://contract.invalid:4222",
        ),
        contract_only=True,
    )


def main() -> None:
    init_telemetry(SERVICE_NAME)
    config = Config()
    app = create_app(config=config, contract_only=False)
    uvicorn.run(
        app,
        host=config.host,
        port=config.port,
        access_log=True,
        server_header=False,
    )


if __name__ == "__main__":
    main()
