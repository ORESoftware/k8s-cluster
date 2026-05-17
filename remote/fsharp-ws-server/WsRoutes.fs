module OresSoftware.Dd.FsWs.WsRoutes

open System
open System.Net.WebSockets
open System.Text
open System.Threading
open System.Threading.Tasks
open Microsoft.AspNetCore.Http
open Microsoft.Extensions.DependencyInjection
open Microsoft.Extensions.Logging

/// HTTP / WebSocket route handlers.
///
///   GET  /healthz       — liveness probe.
///   GET  /readyz        — readiness probe.
///   GET  /ws/rx         — WebSocket; each text frame runs through the Rx.NET pipeline.
///   GET  /ws/async      — WebSocket; same frame, native F# `task { }` pipeline.
///   GET  /v1/benchmark  — runs both pipelines N times against the same payload and
///                         returns a JSON timing summary. Iteration count comes
///                         from the `BENCHMARK_ITERATIONS` env var (default 200).

let private receiveBufferSize = 16 * 1024

let private escapeJson (raw: string) : string =
    if isNull raw then ""
    else raw.Replace("\\", "\\\\").Replace("\"", "\\\"")

let private okFrame (body: string) : string =
    sprintf "{\"ok\":true,\"result\":%s}" body

let private errFrame (pipeline: string) (ex: exn) : string =
    let cause = if isNull ex.InnerException then ex else ex.InnerException
    sprintf
        "{\"ok\":false,\"pipeline\":\"%s\",\"error\":\"%s: %s\"}"
        pipeline
        (cause.GetType().Name)
        (escapeJson cause.Message)

/// Drives a single WebSocket connection: receive a text frame, hand it to
/// `pipeline`, send the result back as a text frame. Errors are converted to a
/// JSON-shaped error frame so the connection isn't torn down on one bad input —
/// matches the akka-ws-server behaviour the comparison loadtests expect.
let private runWsLoop
        (pipelineLabel: string)
        (pipeline: string -> Task<string>)
        (logger: ILogger)
        (ws: WebSocket)
        (ct: CancellationToken)
        : Task =
    task {
        let buffer = Array.zeroCreate<byte> receiveBufferSize
        let segment = ArraySegment(buffer)
        let mutable keep = true
        while keep do
            let! result =
                try ws.ReceiveAsync(segment, ct)
                with ex ->
                    logger.LogWarning(ex, "ws[{Pipeline}] receive failed", pipelineLabel)
                    reraise ()
            match result.MessageType with
            | WebSocketMessageType.Close ->
                do! ws.CloseAsync(
                        WebSocketCloseStatus.NormalClosure,
                        "bye",
                        ct)
                keep <- false
            | WebSocketMessageType.Binary ->
                // We only handle text frames; reject binary politely instead of
                // dropping the connection.
                let payload = Encoding.UTF8.GetBytes(
                                "{\"ok\":false,\"error\":\"binary frames not supported\"}")
                do! ws.SendAsync(
                        ArraySegment(payload),
                        WebSocketMessageType.Text,
                        true,
                        ct)
            | _ ->
                // Strict text frame. ReceiveAsync returned exactly result.Count
                // bytes; if a frame is larger than `receiveBufferSize` we'd need
                // to loop on `EndOfMessage` — kept simple here because the
                // benchmark payloads are well under 1 KiB.
                let input = Encoding.UTF8.GetString(buffer, 0, result.Count)
                let! reply =
                    task {
                        try
                            let! out = pipeline input
                            return okFrame out
                        with ex ->
                            return errFrame pipelineLabel ex
                    }
                let replyBytes = Encoding.UTF8.GetBytes(reply: string)
                do! ws.SendAsync(
                        ArraySegment(replyBytes),
                        WebSocketMessageType.Text,
                        true,
                        ct)
    }

let private acceptAndRun
        (pipelineLabel: string)
        (pipeline: string -> Task<string>)
        (ctx: HttpContext)
        : Task =
    task {
        let factory = ctx.RequestServices.GetRequiredService<ILoggerFactory>()
        let logger = factory.CreateLogger("WsRoutes:" + pipelineLabel)
        if not ctx.WebSockets.IsWebSocketRequest then
            ctx.Response.StatusCode <- 400
            do! ctx.Response.WriteAsync("expected websocket upgrade\n")
        else
            use! ws = ctx.WebSockets.AcceptWebSocketAsync()
            try
                do! runWsLoop pipelineLabel pipeline logger ws ctx.RequestAborted
            with
            | :? OperationCanceledException -> ()
            | ex -> logger.LogWarning(ex, "ws[{Pipeline}] connection ended", pipelineLabel)
    }

let handleRx     ctx = acceptAndRun "rx"    OresSoftware.Dd.FsWs.RxPipeline.processFrame    ctx
let handleAsync  ctx = acceptAndRun "async" OresSoftware.Dd.FsWs.AsyncPipeline.processFrame ctx

let private parsePositiveIntEnv (name: string) (fallback: int) : int =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | raw ->
        match Int32.TryParse(raw.Trim()) with
        | true, v when v > 0 -> v
        | _ -> fallback

let handleBenchmark (ctx: HttpContext) : Task =
    task {
        let iterations = parsePositiveIntEnv "BENCHMARK_ITERATIONS" 200
        let payload =
            match Environment.GetEnvironmentVariable "BENCHMARK_PAYLOAD" with
            | null | "" -> "{\"id\":\"bench\",\"payload\":\"a benchmark message body\"}"
            | v -> v
        let! json = OresSoftware.Dd.FsWs.BenchmarkRunner.runAsync iterations payload
        ctx.Response.ContentType <- "application/json"
        do! ctx.Response.WriteAsync(json)
    }

let handleHealth (ctx: HttpContext) : Task =
    ctx.Response.WriteAsync("ok\n")

let handleReady (ctx: HttpContext) : Task =
    ctx.Response.WriteAsync("ready\n")

/// Machine-readable liveness blob — same intent as `/healthz` but returns
/// JSON so dashboards / probes that prefer structured data don't have to
/// pattern-match on "ok\n".
let handleLive (ctx: HttpContext) : Task =
    let machine = Environment.MachineName
    let proc = System.Diagnostics.Process.GetCurrentProcess()
    let uptimeMs =
        (DateTime.UtcNow - proc.StartTime.ToUniversalTime()).TotalMilliseconds
        |> int64
    let runtime = Environment.Version.ToString()
    let body =
        sprintf
            "{\"ok\":true,\"service\":\"dd-fsharp-ws-server\",\"runtime\":\"dotnet-%s\",\"machine\":\"%s\",\"uptime_ms\":%d}"
            runtime
            (escapeJson machine)
            uptimeMs
    ctx.Response.ContentType <- "application/json"
    ctx.Response.WriteAsync(body)

/// Tiny self-describing HTML landing page. Useful for a quick "yes the pod is
/// alive and serving" eyeball check from a browser / `kubectl port-forward`.
/// The akka-ws-server doesn't have one of these — feel free to copy this
/// pattern over there if you want the same affordance.
let handleIndex (ctx: HttpContext) : Task =
    let html = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>dd-fsharp-ws-server</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
         max-width: 760px; margin: 2rem auto; padding: 0 1rem; }
  h1 { font-size: 1.3rem; margin-bottom: 0.2rem; }
  .sub { color: #888; margin-bottom: 1.5rem; }
  table { border-collapse: collapse; width: 100%; }
  th, td { text-align: left; padding: 6px 10px; border-bottom: 1px solid #8884; }
  code { background: #8881; padding: 1px 6px; border-radius: 4px; }
  .ok { color: #2a8; }
  .dot { display:inline-block; width: 8px; height: 8px; border-radius: 50%;
         background: #2a8; margin-right: 6px; vertical-align: middle; }
</style>
</head>
<body>
  <h1><span class="dot"></span>dd-fsharp-ws-server</h1>
  <div class="sub">ASP.NET Core + F# &middot; Rx.NET vs native <code>task { }</code> pipeline comparison</div>

  <table>
    <thead><tr><th>method</th><th>path</th><th>purpose</th></tr></thead>
    <tbody>
      <tr><td>GET</td><td><a href="/healthz"><code>/healthz</code></a></td><td>liveness probe (text)</td></tr>
      <tr><td>GET</td><td><a href="/readyz"><code>/readyz</code></a></td><td>readiness probe (text)</td></tr>
      <tr><td>GET</td><td><a href="/livez"><code>/livez</code></a></td><td>liveness blob (JSON)</td></tr>
      <tr><td>GET</td><td><a href="/v1/benchmark"><code>/v1/benchmark</code></a></td><td>side-by-side micro-benchmark, JSON timing summary</td></tr>
      <tr><td>WS</td><td><code>/ws/rx</code></td><td>WebSocket; each text frame runs through the Rx.NET pipeline</td></tr>
      <tr><td>WS</td><td><code>/ws/async</code></td><td>WebSocket; same pipeline, F# <code>task { }</code></td></tr>
    </tbody>
  </table>

  <p>
    The five-stage pipeline is
    <code>parse → validate → enrich (lookupA &#8741; lookupB) → score → serialize</code>.
    The per-stage work is byte-for-byte identical between the two implementations;
    only the orchestration around it differs. See the
    <a href="https://github.com/ORESoftware/k8s-cluster/blob/dev/remote/fsharp-ws-server/readme.md">readme</a>
    for the long-form comparison.
  </p>
</body>
</html>
"""
    ctx.Response.ContentType <- "text/html; charset=utf-8"
    ctx.Response.WriteAsync(html)
