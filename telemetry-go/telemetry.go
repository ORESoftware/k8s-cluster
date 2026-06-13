// Package telemetry provides shared OpenTelemetry tracing + trace-correlated
// structured logging for the Go services in k8s-cluster/remote.
//
// It is explicit, SDK-level instrumentation only — no auto-instrumentation, no
// runtime/bytecode patching. Call telemetry.Init once in main, defer the returned
// shutdown, and wrap HTTP handlers with telemetry.Handler.
//
//	shutdown, _ := telemetry.Init(ctx, "dd-go-wss-server")
//	defer shutdown(context.Background())
//	http.Handle("/", telemetry.Handler(mux, "dd-go-wss-server"))
package telemetry

import (
	"context"
	"log/slog"
	"net/http"
	"os"
	"strings"
	"time"

	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	semconv "go.opentelemetry.io/otel/semconv/v1.26.0"
	"go.opentelemetry.io/otel/trace"
)

// defaultEndpoint is the in-cluster OTel collector (host:port, no scheme — that is
// what otlptracehttp.WithEndpoint expects). Used when OTEL_EXPORTER_OTLP_ENDPOINT
// is not set.
const defaultEndpoint = "dd-otel-collector.observability.svc.cluster.local:4318"

// Init installs the global tracer provider (OTLP/HTTP -> collector -> Tempo/Jaeger)
// and a JSON slog logger that stamps trace_id/span_id onto every record. The returned
// func flushes and shuts the provider down; defer it in main.
//
// Init never aborts the process: if the exporter cannot be created, tracing is
// disabled (the returned shutdown is a no-op) and the error is returned for logging.
func Init(ctx context.Context, serviceName string) (func(context.Context) error, error) {
	slog.SetDefault(slog.New(&traceHandler{
		inner: slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo}),
	}))

	endpoint, insecure := resolveEndpoint()
	opts := []otlptracehttp.Option{otlptracehttp.WithEndpoint(endpoint)}
	if insecure {
		opts = append(opts, otlptracehttp.WithInsecure())
	}
	exporter, err := otlptracehttp.New(ctx, opts...)
	if err != nil {
		return func(context.Context) error { return nil }, err
	}

	// resource.Default() folds in OTEL_SERVICE_NAME / OTEL_RESOURCE_ATTRIBUTES from
	// the env; the explicit attrs (incl. k8s pod/namespace) take precedence on merge.
	res, _ := resource.Merge(
		resource.Default(),
		resource.NewWithAttributes(semconv.SchemaURL, serviceAttrs(serviceName)...),
	)

	tp := sdktrace.NewTracerProvider(
		sdktrace.WithBatcher(exporter, sdktrace.WithBatchTimeout(5*time.Second)),
		sdktrace.WithResource(res),
	)
	otel.SetTracerProvider(tp)
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{}, propagation.Baggage{},
	))
	return tp.Shutdown, nil
}

// Handler wraps next so every inbound request opens a server span, extracting any
// upstream W3C traceparent. operation names the span (use the service name).
func Handler(next http.Handler, operation string) http.Handler {
	return otelhttp.NewHandler(next, operation)
}

// Tracer returns a named tracer for manual spans (e.g. an operator reconcile loop).
func Tracer(name string) trace.Tracer { return otel.Tracer(name) }

func resolveEndpoint() (endpoint string, insecure bool) {
	ep := strings.TrimSpace(os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT"))
	if ep == "" {
		return defaultEndpoint, true
	}
	insecure = !strings.HasPrefix(ep, "https://")
	ep = strings.TrimPrefix(strings.TrimPrefix(ep, "https://"), "http://")
	return strings.TrimSuffix(ep, "/"), insecure
}

func serviceAttrs(name string) []attribute.KeyValue {
	if v := strings.TrimSpace(os.Getenv("OTEL_SERVICE_NAME")); v != "" {
		name = v
	}
	attrs := []attribute.KeyValue{semconv.ServiceName(name)}
	if ns := firstEnv("POD_NAMESPACE", "K8S_NAMESPACE_NAME"); ns != "" {
		attrs = append(attrs, semconv.K8SNamespaceName(ns))
	}
	if pod := firstEnv("POD_NAME", "K8S_POD_NAME", "HOSTNAME"); pod != "" {
		attrs = append(attrs, semconv.K8SPodName(pod))
	}
	return attrs
}

func firstEnv(keys ...string) string {
	for _, k := range keys {
		if v := strings.TrimSpace(os.Getenv(k)); v != "" {
			return v
		}
	}
	return ""
}

// traceHandler decorates slog records with the active span's trace/span ids so logs
// correlate with traces in Loki/Grafana.
type traceHandler struct{ inner slog.Handler }

func (h *traceHandler) Enabled(ctx context.Context, l slog.Level) bool {
	return h.inner.Enabled(ctx, l)
}

func (h *traceHandler) WithAttrs(a []slog.Attr) slog.Handler {
	return &traceHandler{inner: h.inner.WithAttrs(a)}
}

func (h *traceHandler) WithGroup(n string) slog.Handler {
	return &traceHandler{inner: h.inner.WithGroup(n)}
}

func (h *traceHandler) Handle(ctx context.Context, r slog.Record) error {
	if sc := trace.SpanContextFromContext(ctx); sc.IsValid() {
		r.AddAttrs(
			slog.String("trace_id", sc.TraceID().String()),
			slog.String("span_id", sc.SpanID().String()),
		)
	}
	return h.inner.Handle(ctx, r)
}
