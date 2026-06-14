// Shared OpenTelemetry tracing + trace-correlated logging for the Node/Fastify
// services in k8s-cluster/remote.
//
// Explicit, SDK-level instrumentation only — NO @opentelemetry/auto-instrumentations
// and NO `registerInstrumentations()` require()-patching. We register a tracer
// provider, an OTLP/HTTP exporter, the W3C propagator and an AsyncLocalStorage
// context manager, then open one SERVER span per request from an ordinary Fastify
// plugin (onRequest/onResponse/onError hooks).
//
// Usage:
//   import { initTelemetry, otelPlugin, loggerMixin } from "@dd/telemetry";
//   const tel = initTelemetry("dd-browser-test-server");
//   const app = Fastify({ logger: { mixin: loggerMixin } });
//   await app.register(otelPlugin, { service: "dd-browser-test-server" });
//   // ... on shutdown: await tel.shutdown();

import {
  context,
  propagation,
  trace,
  SpanKind,
  SpanStatusCode,
} from "@opentelemetry/api";
import { W3CTraceContextPropagator } from "@opentelemetry/core";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { Resource } from "@opentelemetry/resources";
import { BatchSpanProcessor } from "@opentelemetry/sdk-trace-base";
import { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";

// Stable OTel attribute keys as plain strings — avoids coupling to a specific
// @opentelemetry/semantic-conventions major (the constant names churn across them).
const ATTR_SERVICE_NAME = "service.name";
const ATTR_HTTP_REQUEST_METHOD = "http.request.method";
const ATTR_HTTP_RESPONSE_STATUS_CODE = "http.response.status_code";
const ATTR_URL_PATH = "url.path";

const DEFAULT_ENDPOINT =
  "http://dd-otel-collector.observability.svc.cluster.local:4318";

const TRACER_NAME = "@dd/telemetry";
const SPAN = Symbol("dd.otel.span");

function firstEnv(...keys) {
  for (const k of keys) {
    const v = process.env[k];
    if (v && v.trim() !== "") return v;
  }
  return undefined;
}

/**
 * Install the global tracer provider (OTLP/HTTP -> in-cluster collector -> Tempo/
 * Jaeger), the W3C propagator and an AsyncLocalStorage context manager. Returns a
 * handle with `shutdown()` to flush spans on exit. Never throws — telemetry must
 * not take the service down.
 */
export function initTelemetry(serviceName) {
  const base = (
    process.env.OTEL_EXPORTER_OTLP_ENDPOINT || DEFAULT_ENDPOINT
  ).replace(/\/$/, "");

  const attrs = {
    [ATTR_SERVICE_NAME]: process.env.OTEL_SERVICE_NAME || serviceName,
  };
  const ns = firstEnv("POD_NAMESPACE", "K8S_NAMESPACE_NAME");
  if (ns) attrs["k8s.namespace.name"] = ns;
  const pod = firstEnv("POD_NAME", "K8S_POD_NAME", "HOSTNAME");
  if (pod) attrs["k8s.pod.name"] = pod;

  const provider = new NodeTracerProvider({ resource: new Resource(attrs) });
  provider.addSpanProcessor(
    new BatchSpanProcessor(new OTLPTraceExporter({ url: `${base}/v1/traces` })),
  );

  // register() installs the AsyncLocalStorage context manager by default on Node,
  // which is what propagates the active span across awaits (no patching required).
  provider.register({ propagator: new W3CTraceContextPropagator() });

  return {
    provider,
    shutdown: () => provider.shutdown().catch(() => {}),
  };
}

/**
 * Add OpenTelemetry hooks directly to the (root) Fastify instance: one SERVER span
 * per request, parented to any inbound W3C traceparent and made active for the
 * request via the context manager so handler code + logs (see `loggerMixin`)
 * correlate. Call on the root app — NOT via `app.register` — so the hooks apply to
 * all routes (a normally-registered plugin would be encapsulated to its own scope).
 */
export function instrumentFastify(fastify, opts) {
  const tracer = trace.getTracer(TRACER_NAME);
  const service = opts?.service ?? "dd-node-service";

  fastify.addHook("onRequest", (req, reply, hookDone) => {
    const parentCtx = propagation.extract(context.active(), req.headers);
    const route = req.routeOptions?.url ?? req.url ?? "";
    const span = tracer.startSpan(
      `${req.method} ${route}`,
      {
        kind: SpanKind.SERVER,
        attributes: {
          [ATTR_HTTP_REQUEST_METHOD]: req.method,
          [ATTR_URL_PATH]: req.url,
          "dd.service": service,
        },
      },
      parentCtx,
    );
    req[SPAN] = span;
    const ctx = trace.setSpan(parentCtx, span);
    // Run the rest of the request lifecycle inside the span's context. With the
    // AsyncLocalStorage context manager this propagates across the handler's awaits.
    context.with(ctx, () => hookDone());
  });

  const finish = (req, reply) => {
    const span = req[SPAN];
    if (!span) return;
    span.setAttribute(ATTR_HTTP_RESPONSE_STATUS_CODE, reply.statusCode);
    if (reply.statusCode >= 500) {
      span.setStatus({ code: SpanStatusCode.ERROR });
    }
    span.end();
    req[SPAN] = undefined;
  };

  fastify.addHook("onResponse", (req, reply, hookDone) => {
    finish(req, reply);
    hookDone();
  });

  fastify.addHook("onError", (req, reply, error, hookDone) => {
    const span = req[SPAN];
    if (span) {
      span.recordException(error);
      span.setStatus({ code: SpanStatusCode.ERROR, message: String(error?.message ?? error) });
    }
    hookDone();
  });
}

/**
 * pino `mixin` that stamps the active span's trace_id/span_id onto every log line
 * so logs correlate with traces. Use as `Fastify({ logger: { mixin: loggerMixin } })`.
 */
export function loggerMixin() {
  const span = trace.getActiveSpan();
  if (!span) return {};
  const sc = span.spanContext();
  return { trace_id: sc.traceId, span_id: sc.spanId };
}
