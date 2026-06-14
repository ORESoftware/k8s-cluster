//// Shared OpenTelemetry tracing client for Gleam/Mist deployments.
////
//// Explicit instrumentation only — no monkey-patching. This wraps the BEAM
//// OpenTelemetry hex packages (`opentelemetry_api`, `opentelemetry`,
//// `opentelemetry_exporter`) behind a tiny Erlang FFI (`dd_otel_ffi`).
////
//// Usage:
////
////   1. In each server's `main`, BEFORE the Mist supervisor starts:
////
////        dd_otel_client.init("dd-my-service")
////
////      This starts the SDK + OTLP exporter. The Erlang exporter reads OS env
////      `OTEL_EXPORTER_OTLP_ENDPOINT` (defaulted to the in-cluster collector
////      `http://dd-otel-collector.observability.svc.cluster.local:4318`) and
////      `OTEL_EXPORTER_OTLP_PROTOCOL` (defaulted to `http_protobuf`). The
////      resource `service.name` is set from the argument / `OTEL_SERVICE_NAME`.
////
////   2. Wrap the Mist route handler with the `trace` middleware where the
////      listener is built:
////
////        mist.new(dd_otel_client.trace(route))
////        |> mist.bind(host)
////        |> mist.port(port)
////        |> mist.supervised
////
////      `trace` opens a SERVER span named "{METHOD} {PATH}" per request,
////      adopts any inbound W3C `traceparent` as the parent, runs the handler,
////      stamps `http.response.status_code`, and ends the span.
////
////   `with_span(req, handler)` is the same behaviour in per-call form for
////   handlers that aren't a single bare function.

import gleam/http
import gleam/http/request.{type Request}
import gleam/http/response.{type Response}
import mist.{type Connection, type ResponseData}

/// Opaque per-request span handle. Created by `start_server_span`, threaded
/// into `set_status_code` / `end_request_span`. Never inspected in Gleam.
pub type Span

@external(erlang, "dd_otel_ffi", "init")
pub fn init(service_name: String) -> Nil

@external(erlang, "dd_otel_ffi", "start_server_span")
fn start_server_span(
  method: String,
  path: String,
  traceparent: Result(String, Nil),
) -> Span

@external(erlang, "dd_otel_ffi", "set_status_code")
fn set_status_code(span: Span, status: Int) -> Nil

@external(erlang, "dd_otel_ffi", "end_request_span")
fn end_request_span(span: Span) -> Nil

/// Mist middleware. Wrap a route handler so every request is traced:
///
///     mist.new(dd_otel_client.trace(route))
///
/// The returned handler has the exact same type as the one passed in, so it
/// drops into `mist.new(...)` with no other changes.
pub fn trace(
  handler: fn(Request(Connection)) -> Response(ResponseData),
) -> fn(Request(Connection)) -> Response(ResponseData) {
  fn(req: Request(Connection)) -> Response(ResponseData) {
    with_span(req, handler)
  }
}

/// Per-call form of `trace`. Opens a SERVER span for `req`, runs `handler`,
/// stamps the response status, and ends the span. Returns the handler's
/// response unchanged.
pub fn with_span(
  req: Request(Connection),
  handler: fn(Request(Connection)) -> Response(ResponseData),
) -> Response(ResponseData) {
  let span =
    start_server_span(
      http.method_to_string(req.method),
      req.path,
      request.get_header(req, "traceparent"),
    )
  let resp = handler(req)
  set_status_code(span, resp.status)
  end_request_span(span)
  resp
}
