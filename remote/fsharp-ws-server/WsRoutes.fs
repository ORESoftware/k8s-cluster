module OresSoftware.Dd.FsWs.WsRoutes

open System
open System.IO
open System.Net.WebSockets
open System.Text
open System.Text.Json
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

let private defaultMaxTextFrameBytes = 65536
let private maxTextFrameBytesCeiling = 1048576
let private defaultBenchmarkIterations = 200
let private defaultMaxBenchmarkIterations = 1000
let private maxBenchmarkIterationsCeiling = 10000

let private jsonString (raw: string) : string =
    JsonSerializer.Serialize(if isNull raw then "" else raw)

let private okFrame (body: string) : string =
    sprintf "{\"ok\":true,\"result\":%s}" body

let private errFrame (pipeline: string) (ex: exn) : string =
    let cause = if isNull ex.InnerException then ex else ex.InnerException
    let error = sprintf "%s: %s" (cause.GetType().Name) cause.Message
    sprintf
        "{\"ok\":false,\"pipeline\":%s,\"error\":%s}"
        (jsonString pipeline)
        (jsonString error)

let private parseBoundedPositiveIntEnv (name: string) (fallback: int) (upperBound: int) : int =
    let boundedFallback = max 1 (min upperBound fallback)
    match Environment.GetEnvironmentVariable name with
    | null | "" -> boundedFallback
    | raw ->
        match Int32.TryParse(raw.Trim()) with
        | true, v when v > 0 -> max 1 (min upperBound v)
        | _ -> boundedFallback

let private closeIfOpen
        (ws: WebSocket)
        (status: WebSocketCloseStatus)
        (description: string)
        (ct: CancellationToken)
        : Task =
    task {
        if ws.State = WebSocketState.Open || ws.State = WebSocketState.CloseReceived then
            do! ws.CloseAsync(status, description, ct)
    }

type private InboundFrame =
    | TextFrame of string
    | CloseFrame

let private receiveTextFrame
        (pipelineLabel: string)
        (logger: ILogger)
        (ws: WebSocket)
        (ct: CancellationToken)
        (maxTextFrameBytes: int)
        : Task<InboundFrame> =
    task {
        let buffer = Array.zeroCreate<byte> receiveBufferSize
        let segment = ArraySegment(buffer)
        use message = new MemoryStream()
        let mutable frame = CloseFrame
        let mutable finished = false

        while not finished do
            let! result =
                try ws.ReceiveAsync(segment, ct)
                with ex ->
                    logger.LogWarning(ex, "ws[{Pipeline}] receive failed", pipelineLabel)
                    reraise ()

            match result.MessageType with
            | WebSocketMessageType.Close ->
                finished <- true
            | WebSocketMessageType.Binary ->
                logger.LogInformation(sprintf "ws[%s] rejected binary frame" pipelineLabel)
                do! closeIfOpen ws WebSocketCloseStatus.InvalidMessageType "binary frames not supported" ct
                finished <- true
            | _ ->
                if result.Count > 0 then
                    message.Write(buffer, 0, result.Count)

                if message.Length > int64 maxTextFrameBytes then
                    logger.LogWarning(
                        sprintf
                            "ws[%s] rejected oversized text frame: %d > %d"
                            pipelineLabel
                            message.Length
                            maxTextFrameBytes)
                    do! closeIfOpen ws WebSocketCloseStatus.MessageTooBig "text frame too large" ct
                    finished <- true
                elif result.EndOfMessage then
                    frame <- TextFrame(Encoding.UTF8.GetString(message.ToArray()))
                    finished <- true

        return frame
    }

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
        let maxTextFrameBytes =
            parseBoundedPositiveIntEnv
                "MAX_WS_TEXT_FRAME_BYTES"
                defaultMaxTextFrameBytes
                maxTextFrameBytesCeiling
        let mutable keep = true
        while keep do
            let! frame = receiveTextFrame pipelineLabel logger ws ct maxTextFrameBytes
            match frame with
            | CloseFrame ->
                do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                keep <- false
            | TextFrame input ->
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

let handleBenchmark (ctx: HttpContext) : Task =
    task {
        let maxIterations =
            parseBoundedPositiveIntEnv
                "MAX_BENCHMARK_ITERATIONS"
                defaultMaxBenchmarkIterations
                maxBenchmarkIterationsCeiling
        let iterations =
            parseBoundedPositiveIntEnv
                "BENCHMARK_ITERATIONS"
                defaultBenchmarkIterations
                maxIterations
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
    let runtime = "dotnet-" + Environment.Version.ToString()
    let body =
        sprintf
            "{\"ok\":true,\"service\":\"dd-fsharp-ws-server\",\"runtime\":%s,\"machine\":%s,\"uptime_ms\":%d}"
            (jsonString runtime)
            (jsonString machine)
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
