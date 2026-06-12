"""Shared OpenTelemetry tracing for dd-ai-ml-pipeline.

Explicit, SDK-level instrumentation only — no `opentelemetry-instrument` bootstrap
and no library auto-instrumentation. We register a tracer provider with an
OTLP/HTTP exporter and wrap the stdlib ``BaseHTTPRequestHandler.do_*`` methods so
each request opens a SERVER span (parented to any inbound W3C ``traceparent``).

Degrades gracefully: if the ``opentelemetry`` packages are not installed,
``init_telemetry`` is a no-op and ``instrument_handler`` returns the class
unchanged, so the server still runs (e.g. a local checkout without the deps).
"""
from __future__ import annotations

import atexit
import functools
import os
from typing import Any, Optional

try:
    from opentelemetry import context as otel_context
    from opentelemetry import trace
    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.propagate import extract
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor
    from opentelemetry.trace import SpanKind, Status, StatusCode

    _OTEL_AVAILABLE = True
except ImportError:  # opentelemetry not installed — run without tracing.
    _OTEL_AVAILABLE = False

_DEFAULT_ENDPOINT = "http://dd-otel-collector.observability.svc.cluster.local:4318"


def _first_env(*keys: str) -> Optional[str]:
    for key in keys:
        value = os.environ.get(key)
        if value and value.strip():
            return value
    return None


def init_telemetry(service_name: str) -> Any:
    """Install the global tracer provider (OTLP/HTTP -> in-cluster collector).

    Returns the provider (or ``None`` if OpenTelemetry is unavailable). Safe to
    call once at startup; never raises.
    """
    if not _OTEL_AVAILABLE:
        return None
    try:
        base = (os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT") or _DEFAULT_ENDPOINT).rstrip("/")
        attrs = {"service.name": os.environ.get("OTEL_SERVICE_NAME") or service_name}
        namespace = _first_env("POD_NAMESPACE", "K8S_NAMESPACE_NAME")
        if namespace:
            attrs["k8s.namespace.name"] = namespace
        pod = _first_env("POD_NAME", "K8S_POD_NAME", "HOSTNAME")
        if pod:
            attrs["k8s.pod.name"] = pod

        provider = TracerProvider(resource=Resource.create(attrs))
        provider.add_span_processor(
            BatchSpanProcessor(OTLPSpanExporter(endpoint=f"{base}/v1/traces"))
        )
        trace.set_tracer_provider(provider)
        atexit.register(provider.shutdown)
        return provider
    except Exception as exc:  # never let telemetry setup take the server down
        print(f"dd-telemetry: OTLP setup failed ({exc}); continuing without traces", flush=True)
        return None


def _wrap_do_method(method):
    @functools.wraps(method)
    def wrapper(self):
        tracer = trace.get_tracer("dd.ai_ml_pipeline")
        parent = extract(dict(self.headers.items())) if getattr(self, "headers", None) else None
        path = (self.path or "").split("?", 1)[0]
        span = tracer.start_span(
            f"{self.command} {path}",
            context=parent,
            kind=SpanKind.SERVER,
            attributes={"http.request.method": self.command, "url.path": self.path},
        )
        self._otel_span = span
        token = otel_context.attach(trace.set_span_in_context(span, parent))
        try:
            return method(self)
        except Exception as exc:
            span.record_exception(exc)
            span.set_status(Status(StatusCode.ERROR))
            raise
        finally:
            otel_context.detach(token)
            span.end()
            self._otel_span = None

    return wrapper


def _wrap_send_response(orig):
    @functools.wraps(orig)
    def wrapper(self, code, *args, **kwargs):
        span = getattr(self, "_otel_span", None)
        if span is not None:
            try:
                status = int(code.value if hasattr(code, "value") else code)
                span.set_attribute("http.response.status_code", status)
            except Exception:
                pass
        return orig(self, code, *args, **kwargs)

    return wrapper


def instrument_handler(cls):
    """Class decorator: wrap every ``do_*`` method of a ``BaseHTTPRequestHandler``
    subclass in a SERVER span, and capture the response status via ``send_response``.
    No-op if OpenTelemetry is unavailable.
    """
    if not _OTEL_AVAILABLE:
        return cls
    for name in list(vars(cls)):
        if name.startswith("do_") and callable(getattr(cls, name)):
            setattr(cls, name, _wrap_do_method(getattr(cls, name)))
    if "send_response" in vars(cls):
        cls.send_response = _wrap_send_response(cls.send_response)
    else:
        # send_response lives on the stdlib base; override on the subclass so we
        # only touch our handler, not BaseHTTPRequestHandler globally.
        import http.server

        cls.send_response = _wrap_send_response(http.server.BaseHTTPRequestHandler.send_response)
    return cls
