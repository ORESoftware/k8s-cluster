module OresSoftware.Dd.FsWs.WsRoutes

open System
open System.IO
open System.Net.WebSockets
open System.Reactive.Subjects
open System.Text
open System.Text.Json
open System.Threading
open System.Threading.Channels
open System.Threading.Tasks
open Microsoft.AspNetCore.Http
open Microsoft.AspNetCore.Http.Features
open Microsoft.Extensions.DependencyInjection
open Microsoft.Extensions.Logging
open OresSoftware.Dd.FsWs.RxAdvanced

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
                RxStats.messageIn (Encoding.UTF8.GetByteCount(input: string))
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
                RxStats.messageOut replyBytes.Length
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
            RxStats.connectionOpened ()
            try
                try
                    do! runWsLoop pipelineLabel pipeline logger ws ctx.RequestAborted
                with
                | :? OperationCanceledException -> ()
                | ex -> logger.LogWarning(ex, "ws[{Pipeline}] connection ended", pipelineLabel)
            finally
                RxStats.connectionClosed ()
    }

let handleRx     ctx = acceptAndRun "rx"    OresSoftware.Dd.FsWs.RxPipeline.processFrame    ctx
let handleAsync  ctx = acceptAndRun "async" OresSoftware.Dd.FsWs.AsyncPipeline.processFrame ctx

// ----------------------------------------------------------------------------
// Long-running Rx connection: the Subject + IObservable graph is materialised
// ONCE at connect and lives for the lifetime of the socket. Frames arriving
// over the wire are pushed through `Subject.OnNext`; frames coming out of
// the pipeline are written to a Channel and drained by a sender task.
//
// Splitting send / receive across two tasks matters because:
//
//   1. WebSocket.SendAsync isn't safe to call concurrently from multiple
//      threads — we need one writer.
//   2. The Rx pipeline can emit on whatever scheduler it likes (typically
//      TaskPoolScheduler, since the enrich stage uses Observable.Start) —
//      we must marshal those emissions onto the single sender task.
//
// Why a Channel rather than awaiting inside Subscribe? Subscribe callbacks
// must be synchronous (they're called from whatever thread the operator
// chose). Channels.Writer.TryWrite is the canonical "sync push into async
// queue" primitive.
// ----------------------------------------------------------------------------

let private runRxStreamLoop
        (pipelineLabel: string)
        (pipeline: IObservable<string> -> IObservable<string>)
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

        // Hot Subject: the entry point into the Rx graph for this connection.
        // Subject is single-producer-safe (only the receive loop OnNext's it),
        // which is all we need here.
        let inbound = new Subject<string>()
        let outChan = Channel.CreateUnbounded<string>()

        // Materialise the pipeline ONCE for the lifetime of this socket.
        // Subscribing here means: subsequent inbound.OnNext walks the same
        // operator chain rather than rebuilding it. That's the per-call
        // overhead win Rx is supposed to deliver — visible in
        // /v1/benchmark-stream vs /v1/benchmark.
        let onNext frame =
            outChan.Writer.TryWrite(frame) |> ignore
        let onError (ex: exn) =
            logger.LogWarning(
                ex, "ws[{Pipeline}] pipeline OnError", pipelineLabel)
            let err =
                sprintf "{\"ok\":false,\"pipeline\":%s,\"error\":%s}"
                    (jsonString pipelineLabel)
                    (jsonString (
                        sprintf "%s: %s"
                            (ex.GetType().Name)
                            (if isNull ex.Message then "" else ex.Message)))
            outChan.Writer.TryWrite(err) |> ignore
            outChan.Writer.TryComplete() |> ignore
        let onCompleted () =
            outChan.Writer.TryComplete() |> ignore

        use _sub =
            System.ObservableExtensions.Subscribe(
                pipeline (inbound :> IObservable<string>),
                Action<string>(onNext),
                Action<exn>(onError),
                Action(onCompleted))

        // Sender task: drain Channel → ws.SendAsync. Errors here are
        // terminal (broken socket); we let them propagate and the outer
        // try/finally in acceptAndRunRxStream will tear down the
        // subscription cleanly.
        let sender =
            task {
                try
                    let mutable run = true
                    while run do
                        let! more = outChan.Reader.WaitToReadAsync(ct).AsTask()
                        if not more then run <- false
                        else
                            let mutable frame : string = null
                            while outChan.Reader.TryRead(&frame) do
                                let bytes = Encoding.UTF8.GetBytes(frame: string)
                                do! ws.SendAsync(
                                        ArraySegment(bytes),
                                        WebSocketMessageType.Text,
                                        true,
                                        ct)
                                RxStats.messageOut bytes.Length
                with :? OperationCanceledException -> ()
            } :> Task

        try
            let mutable run = true
            while run do
                let! frame = receiveTextFrame pipelineLabel logger ws ct maxTextFrameBytes
                match frame with
                | CloseFrame ->
                    do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                    run <- false
                | TextFrame input ->
                    RxStats.messageIn (Encoding.UTF8.GetByteCount(input: string))
                    inbound.OnNext(input)
        finally
            inbound.OnCompleted()
            outChan.Writer.TryComplete() |> ignore

        // Wait for sender to flush remaining frames or for the socket to
        // close — whichever happens first.
        try do! sender
        with _ -> ()
    }

let private acceptAndRunRxStream
        (pipelineLabel: string)
        (pipeline: IObservable<string> -> IObservable<string>)
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
            RxStats.connectionOpened ()
            try
                try
                    do! runRxStreamLoop pipelineLabel pipeline logger ws ctx.RequestAborted
                with
                | :? OperationCanceledException -> ()
                | ex -> logger.LogWarning(ex, "ws[{Pipeline}] connection ended", pipelineLabel)
            finally
                RxStats.connectionClosed ()
    }

let handleRxStream    ctx = acceptAndRunRxStream "rx-stream"   RxStreamFlow.pipeline   ctx
let handleRxWindow    ctx = acceptAndRunRxStream "rx-window"   RxWindowFlow.pipeline   ctx
let handleRxThrottle  ctx = acceptAndRunRxStream "rx-throttle" RxThrottleFlow.pipeline ctx

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

// ----------------------------------------------------------------------------
// /v1/rx-stats             — current snapshot (one-shot JSON).
// /v1/rx-stats/history     — last 120 snapshots, replayed synchronously off
//                            the ReplaySubject buffer.
// /sse/rx-stats            — Server-Sent Events, 1 Hz live feed.
// ----------------------------------------------------------------------------

let handleRxStats (ctx: HttpContext) : Task =
    ctx.Response.ContentType <- "application/json"
    ctx.Response.WriteAsync(serializeSnapshot (RxStats.snapshot ()))

let handleRxStatsHistory (ctx: HttpContext) : Task =
    // ReplaySubject<T>.Subscribe synchronously emits all buffered values
    // on the calling thread, then continues with live values. We attach,
    // collect into a ResizeArray, and detach immediately so we don't pick
    // up any new ticks that fire mid-serialization.
    let buf = ResizeArray<StatsSnapshot>(128)
    let sub = RxStats.history.Subscribe(fun snap -> buf.Add(snap))
    sub.Dispose()
    ctx.Response.ContentType <- "application/json"
    let items =
        buf
        |> Seq.map serializeSnapshot
        |> String.concat ","
    ctx.Response.WriteAsync(sprintf "{\"snapshots\":[%s]}" items)

let handleRxStatsSse (ctx: HttpContext) : Task =
    task {
        let factory = ctx.RequestServices.GetRequiredService<ILoggerFactory>()
        let logger = factory.CreateLogger("WsRoutes:rx-stats-sse")
        let ct = ctx.RequestAborted

        ctx.Response.Headers.["Content-Type"] <- "text/event-stream"
        ctx.Response.Headers.["Cache-Control"] <- "no-cache"
        // X-Accel-Buffering off tells nginx not to buffer the SSE stream;
        // /fsws/ already has proxy_buffering off so this is belt-and-braces.
        ctx.Response.Headers.["X-Accel-Buffering"] <- "no"

        // Disable kestrel response buffering so each `data:` line flushes
        // to the client immediately instead of piling up in a 4 KiB buffer.
        let body = ctx.Features.Get<IHttpResponseBodyFeature>()
        if not (isNull body) then body.DisableBuffering()

        // Initial hello so the client knows the stream is alive even when
        // the next tick is up to 1s away.
        do! ctx.Response.WriteAsync("event: hello\ndata: connected\n\n", ct)
        do! ctx.Response.Body.FlushAsync(ct)

        // Subscribe is sync-callback, we need an async sink. Channel
        // bridges the gap.
        let outChan = Channel.CreateUnbounded<StatsSnapshot>()
        let onNext snap = outChan.Writer.TryWrite(snap) |> ignore
        use _sub =
            System.ObservableExtensions.Subscribe(
                RxStats.live,
                Action<StatsSnapshot>(onNext))

        try
            let mutable run = true
            while run do
                let! more = outChan.Reader.WaitToReadAsync(ct).AsTask()
                if not more then run <- false
                else
                    let mutable snap = Unchecked.defaultof<StatsSnapshot>
                    while outChan.Reader.TryRead(&snap) do
                        let line =
                            sprintf "data: %s\n\n" (serializeSnapshot snap)
                        do! ctx.Response.WriteAsync(line, ct)
                        do! ctx.Response.Body.FlushAsync(ct)
        with
        | :? OperationCanceledException -> ()
        | ex -> logger.LogWarning(ex, "sse[rx-stats] terminated")
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
  h2.grp { font-size: 0.95rem; font-weight: 600; margin: 1.4rem 0 0.3rem;
           text-transform: uppercase; letter-spacing: 0.04em; color: #6aa; }
  .sub { color: #888; margin-bottom: 1.5rem; }
  table { border-collapse: collapse; width: 100%; margin-bottom: 0.4rem; }
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

  <h2 class="grp">Probes &amp; metadata</h2>
  <table>
    <tbody>
      <tr><td>GET</td><td><a href="/healthz"><code>/healthz</code></a></td><td>liveness probe (text)</td></tr>
      <tr><td>GET</td><td><a href="/readyz"><code>/readyz</code></a></td><td>readiness probe (text)</td></tr>
      <tr><td>GET</td><td><a href="/livez"><code>/livez</code></a></td><td>liveness blob (JSON, runtime &amp; uptime)</td></tr>
    </tbody>
  </table>

  <h2 class="grp">Per-message comparison (apples-to-apples)</h2>
  <table>
    <tbody>
      <tr><td>WS</td><td><code>/ws/rx</code></td><td>each text frame builds a fresh <code>Observable.Return → … → ToTask</code> — worst-case Rx (the comparison target)</td></tr>
      <tr><td>WS</td><td><code>/ws/async</code></td><td>same work, F# <code>task { }</code> + <code>Task.WhenAll</code></td></tr>
      <tr><td>GET</td><td><a href="/v1/benchmark"><code>/v1/benchmark</code></a></td><td>side-by-side micro-benchmark, JSON timing summary</td></tr>
    </tbody>
  </table>

  <h2 class="grp">Rx-native long-running pipelines</h2>
  <table>
    <tbody>
      <tr><td>WS</td><td><code>/ws/rx-stream</code></td><td>per-connection <code>Subject&lt;string&gt;</code>, pipeline materialised <em>once</em> at connect</td></tr>
      <tr><td>WS</td><td><code>/ws/rx-window</code></td><td>same input, output goes through <code>Buffer(200ms, 16)</code> — one batched frame per window</td></tr>
      <tr><td>WS</td><td><code>/ws/rx-throttle</code></td><td>same input, output <code>Throttle(50ms)</code> — flood the socket, you only get the last reply per quiet window</td></tr>
    </tbody>
  </table>

  <h2 class="grp">Live process telemetry (Rx <code>BehaviorSubject</code> + <code>ReplaySubject</code> + SSE)</h2>
  <table>
    <tbody>
      <tr><td>GET</td><td><a href="/v1/rx-stats"><code>/v1/rx-stats</code></a></td><td>current open-connections / msgs / bytes (JSON, one-shot)</td></tr>
      <tr><td>GET</td><td><a href="/v1/rx-stats/history"><code>/v1/rx-stats/history</code></a></td><td>last ~120 snapshots, replayed off the <code>ReplaySubject</code> buffer</td></tr>
      <tr><td>SSE</td><td><a href="/sse/rx-stats"><code>/sse/rx-stats</code></a></td><td>1 Hz Server-Sent Events feed of the live snapshot</td></tr>
    </tbody>
  </table>

  <p>
    The five-stage pipeline is
    <code>parse → validate → enrich (lookupA &#8741; lookupB) → score → serialize</code>.
    The per-stage work is byte-for-byte identical between every implementation;
    only the orchestration around it differs. See the
    <a href="https://github.com/ORESoftware/k8s-cluster/blob/dev/remote/fsharp-ws-server/readme.md">readme</a>
    for the long-form comparison.
  </p>
</body>
</html>
"""
    ctx.Response.ContentType <- "text/html; charset=utf-8"
    ctx.Response.WriteAsync(html)
