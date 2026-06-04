module OresSoftware.Dd.FsWs.WsRoutes

open System
open System.IO
open System.Net.WebSockets
open System.Reactive.Linq
open System.Reactive.Subjects
open System.Text
open System.Text.Json
open System.Text.Json.Nodes
open System.Threading
open System.Threading.Channels
open System.Threading.Tasks
open Microsoft.AspNetCore.Http
open Microsoft.AspNetCore.Http.Features
open Microsoft.Extensions.DependencyInjection
open Microsoft.Extensions.Logging
open NATS.Client.Core
open Npgsql
open OresSoftware.Dd.FsWs.RxAdvanced
open OresSoftware.Dd.FsWs.PgSchema
open OresSoftware.Dd.FsWs.PgListen
open OresSoftware.Dd.FsWs.PgWal
open OresSoftware.Dd.FsWs.PgOutbox
open OresSoftware.Dd.FsWs.NatsRx
open OresSoftware.Dd.FsWs.PresenceFanIn

// ----------------------------------------------------------------------------
// PresenceState — Program.fs builds this at boot (some fields may be `None`
// when their env var was unset / the connection failed) and hands it to
// WsRoutes via `setPresenceState`. Handlers read it through the module-level
// ref; nothing else mutates it after boot.
// ----------------------------------------------------------------------------

type PresenceState = {
    /// Postgres connection string in Npgsql format (key=value;key=value).
    /// `None` means PG-backed endpoints (/ws/rx-publish writes, /ws/rx-presence
    /// fan-in from PG sources) operate in degraded mode.
    DbConnectionString: string option
    PgListen:           PgListenHandle option
    PgWal:              PgWalHandle option
    PgOutbox:           PgOutboxHandle option
    Nats:               NatsHandle option
    FanIn:              FanInHandle
}

let mutable private presenceStateRef : PresenceState option = None

let setPresenceState (state: PresenceState) : unit =
    presenceStateRef <- Some state

let private presence () : PresenceState option = presenceStateRef

/// HTTP / WebSocket route handlers.
///
///   GET  /healthz       — liveness probe.
///   GET  /readyz        — readiness probe.
///   GET  /ws/rx         — WebSocket; each text frame runs through the Rx.NET pipeline.
///   GET  /ws/async      — WebSocket; same frame, native F# `task { }` pipeline.
///   GET  /ws/rx-*       — long-running Rx-native stream/window/throttle/sample demos.
///   GET  /v1/benchmark  — runs both pipelines N times against the same payload and
///                         returns a JSON timing summary. Iteration count comes
///                         from the `BENCHMARK_ITERATIONS` env var (default 200).

let private receiveBufferSize = 16 * 1024

let private defaultMaxTextFrameBytes = 65536
let private maxTextFrameBytesCeiling = 1048576
let private defaultBenchmarkIterations = 200
let private defaultMaxBenchmarkIterations = 1000
let private maxBenchmarkIterationsCeiling = 10000
let private defaultRxStreamOutboundQueueCapacity = 1024
let private rxStreamOutboundQueueCapacityCeiling = 65536

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
        use inbound = new Subject<string>()
        use loopCts = CancellationTokenSource.CreateLinkedTokenSource(ct)
        let loopCt = loopCts.Token
        let outboundQueueCapacity =
            parseBoundedPositiveIntEnv
                "RX_STREAM_OUTBOUND_QUEUE_CAPACITY"
                defaultRxStreamOutboundQueueCapacity
                rxStreamOutboundQueueCapacityCeiling
        let outChanOptions = BoundedChannelOptions(outboundQueueCapacity)
        outChanOptions.SingleReader <- true
        outChanOptions.SingleWriter <- false
        outChanOptions.FullMode <- BoundedChannelFullMode.Wait
        let outChan = Channel.CreateBounded<string>(outChanOptions)
        let outboundQueueFull =
            TaskCompletionSource<unit>(
                TaskCreationOptions.RunContinuationsAsynchronously)

        // Materialise the pipeline ONCE for the lifetime of this socket.
        // Subscribing here means: subsequent inbound.OnNext walks the same
        // operator chain rather than rebuilding it. That's the per-call
        // overhead win Rx is supposed to deliver — visible in
        // /v1/benchmark-stream vs /v1/benchmark.
        let onNext frame =
            if not (outChan.Writer.TryWrite(frame)) then
                if outboundQueueFull.TrySetResult(()) then
                    logger.LogWarning(
                        "ws[{Pipeline}] outbound queue full ({Capacity}); closing connection",
                        pipelineLabel,
                        outboundQueueCapacity)
                    outChan.Writer.TryComplete(
                        InvalidOperationException(
                            sprintf
                                "rx outbound queue full (%d)"
                                outboundQueueCapacity)) |> ignore
                    loopCts.Cancel()
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
                        let! more = outChan.Reader.WaitToReadAsync(loopCt).AsTask()
                        if not more then run <- false
                        else
                            let mutable frame : string = null
                            while outChan.Reader.TryRead(&frame) do
                                let bytes = Encoding.UTF8.GetBytes(frame: string)
                                do! ws.SendAsync(
                                        ArraySegment(bytes),
                                        WebSocketMessageType.Text,
                                        true,
                                        loopCt)
                                RxStats.messageOut bytes.Length
                with :? OperationCanceledException -> ()
            } :> Task

        try
            let mutable run = true
            while run do
                let! frame = receiveTextFrame pipelineLabel logger ws loopCt maxTextFrameBytes
                match frame with
                | CloseFrame ->
                    do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" loopCt
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
let handleRxSample    ctx = acceptAndRunRxStream "rx-sample"   RxSampleFlow.pipeline   ctx
let handleRxBurst     ctx = acceptAndRunRxStream "rx-burst"    RxBurstFlow.pipeline    ctx

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

// ----------------------------------------------------------------------------
// Presence (NATS + PG LISTEN/NOTIFY + PG WAL + PG outbox) — the four ingest
// paths fan into PresenceFanIn's hot observable; the handlers here are pure
// projections off that single shared stream.
// ----------------------------------------------------------------------------

let private serializeUnifiedEvent (evt: UnifiedEvent) : string =
    // Build a small JSON envelope around the verbatim payload. We don't
    // re-serialize `evt.Payload` because it's already JSON; we just splice it
    // in with the parens trick. Other fields are simple enough to format
    // with String.Format under the invariant culture (no comma surprises
    // on European locales).
    let payload =
        if String.IsNullOrEmpty(evt.Payload) then "null" else evt.Payload
    System.String.Format(
        System.Globalization.CultureInfo.InvariantCulture,
        "{{\"event_id\":\"{0}\",\"seq\":{1},\"kind\":{2},\"conv_id\":\"{3}\",\"occurred_at\":\"{4:O}\",\"source\":{5},\"payload\":{6}}}",
        evt.EventId,
        evt.Seq,
        jsonString evt.Kind,
        evt.ConvId,
        evt.OccurredAt.ToUniversalTime(),
        jsonString evt.Source.Label,
        payload)

let private parseConvIdFilter (logger: ILogger) (raw: string) : Set<Guid> option =
    // First frame on /ws/rx-presence is `{"conv_ids":["uuid","uuid",...]}`
    // — empty array OR no `conv_ids` key means "all conv ids".
    try
        let root = JsonNode.Parse(raw)
        match root.["conv_ids"] with
        | :? JsonArray as arr when arr.Count > 0 ->
            arr
            |> Seq.choose (fun n ->
                if isNull n then None
                else
                    match Guid.TryParse(n.ToString()) with
                    | true, g -> Some g
                    | _ -> None)
            |> Set.ofSeq
            |> Some
        | _ -> None
    with ex ->
        logger.LogWarning(
            ex,
            "ws[rx-presence]: filter frame is not valid JSON; falling back to ALL")
        None

let handleRxPresence (ctx: HttpContext) : Task =
    task {
        let factory = ctx.RequestServices.GetRequiredService<ILoggerFactory>()
        let logger = factory.CreateLogger("WsRoutes:rx-presence")
        match presence () with
        | None ->
            ctx.Response.StatusCode <- 503
            do! ctx.Response.WriteAsync(
                    "presence subsystem not initialised\n")
        | Some state ->
            if not ctx.WebSockets.IsWebSocketRequest then
                ctx.Response.StatusCode <- 400
                do! ctx.Response.WriteAsync("expected websocket upgrade\n")
            else
                use! ws = ctx.WebSockets.AcceptWebSocketAsync()
                RxStats.connectionOpened ()
                let ct = ctx.RequestAborted
                let maxBytes =
                    parseBoundedPositiveIntEnv
                        "MAX_WS_TEXT_FRAME_BYTES"
                        defaultMaxTextFrameBytes
                        maxTextFrameBytesCeiling
                try
                    try
                        // 1. Wait for the filter frame.
                        let! firstFrame = receiveTextFrame "rx-presence" logger ws ct maxBytes
                        match firstFrame with
                        | CloseFrame ->
                            do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                        | TextFrame raw ->
                            RxStats.messageIn (Encoding.UTF8.GetByteCount(raw: string))
                            let filter = parseConvIdFilter logger raw
                            let filterText =
                                match filter with
                                | Some s -> sprintf "%d conv ids" s.Count
                                | None -> "ALL"
                            logger.LogInformation(
                                "ws[rx-presence]: client subscribed to {Filter}",
                                filterText)
                            // 2. Wire the fan-in observable into a Channel
                            //    (Subscribe is sync-only, sends must be async
                            //    and serialised).
                            let outChan = Channel.CreateUnbounded<string>()
                            let onNext (evt: UnifiedEvent) =
                                let pass =
                                    match filter with
                                    | None     -> true
                                    | Some set -> set.Contains(evt.ConvId)
                                if pass then
                                    outChan.Writer.TryWrite(
                                        serializeUnifiedEvent evt) |> ignore
                            use _sub =
                                System.ObservableExtensions.Subscribe(
                                    state.FanIn.Events,
                                    Action<UnifiedEvent>(onNext))
                            // Send `ack` so the client knows the subscribe
                            // succeeded before it starts waiting on events.
                            let ack =
                                sprintf "{\"ok\":true,\"subscribed\":%s}"
                                    (match filter with
                                     | None -> "\"all\""
                                     | Some s -> sprintf "%d" s.Count)
                            let ackBytes = Encoding.UTF8.GetBytes(ack: string)
                            do! ws.SendAsync(
                                    ArraySegment(ackBytes),
                                    WebSocketMessageType.Text,
                                    true, ct)
                            RxStats.messageOut ackBytes.Length

                            // 3. Producer task: drain the channel into the WS.
                            //    Receive task: consume inbound frames so the
                            //    socket stays open (we currently ignore them,
                            //    but they prevent client keepalive timeouts).
                            let sender =
                                task {
                                    let mutable run = true
                                    while run && not ct.IsCancellationRequested do
                                        let! more = outChan.Reader.WaitToReadAsync(ct).AsTask()
                                        if not more then run <- false
                                        else
                                            let mutable frame : string = null
                                            while outChan.Reader.TryRead(&frame) do
                                                let bytes = Encoding.UTF8.GetBytes(frame: string)
                                                do! ws.SendAsync(
                                                        ArraySegment(bytes),
                                                        WebSocketMessageType.Text,
                                                        true, ct)
                                                RxStats.messageOut bytes.Length
                                } :> Task
                            try
                                let mutable run = true
                                while run do
                                    let! frame =
                                        receiveTextFrame "rx-presence" logger ws ct maxBytes
                                    match frame with
                                    | CloseFrame ->
                                        do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                                        run <- false
                                    | TextFrame _ ->
                                        // Future: re-subscribe / unsubscribe
                                        // commands could be parsed here. For
                                        // now we just count inbound bytes for
                                        // stats parity.
                                        ()
                            finally
                                outChan.Writer.TryComplete() |> ignore
                            try do! sender
                            with _ -> ()
                    with
                    | :? OperationCanceledException -> ()
                    | ex -> logger.LogWarning(ex, "ws[rx-presence] connection ended")
                finally
                    RxStats.connectionClosed ()
    }

let private nowIso () =
    DateTime.UtcNow.ToString(
        "yyyy-MM-ddTHH:mm:ss.fffZ",
        System.Globalization.CultureInfo.InvariantCulture)

let private writeEventToDb
        (connectionString: string)
        (eid: Guid)
        (kind: string)
        (convId: Guid)
        (payload: string)
        (ct: CancellationToken)
        : Task<int64 * DateTime> =
    task {
        use conn = new NpgsqlConnection(connectionString)
        do! conn.OpenAsync(ct)
        use cmd =
            new NpgsqlCommand(
                "SELECT seq, occurred_at FROM fsws_publish_event(@id, @kind, @conv, @payload::jsonb)",
                conn)
        cmd.Parameters.AddWithValue("id", eid) |> ignore
        cmd.Parameters.AddWithValue("kind", kind) |> ignore
        cmd.Parameters.AddWithValue("conv", convId) |> ignore
        cmd.Parameters.AddWithValue("payload", payload) |> ignore
        use! reader = cmd.ExecuteReaderAsync(ct)
        let! hasRow = reader.ReadAsync(ct)
        if hasRow then
            let seq = reader.GetInt64(0)
            let occurred = reader.GetDateTime(1).ToUniversalTime()
            return (seq, occurred)
        else
            // Insert was de-duped by the ON CONFLICT DO NOTHING — the row
            // already existed under this event_id. Return -1 to signal
            // "no-op". Caller decides whether to surface that to the client.
            return (-1L, DateTime.UtcNow)
    }

let handleRxPublish (ctx: HttpContext) : Task =
    task {
        let factory = ctx.RequestServices.GetRequiredService<ILoggerFactory>()
        let logger = factory.CreateLogger("WsRoutes:rx-publish")
        if not ctx.WebSockets.IsWebSocketRequest then
            ctx.Response.StatusCode <- 400
            do! ctx.Response.WriteAsync("expected websocket upgrade\n")
        else
            use! ws = ctx.WebSockets.AcceptWebSocketAsync()
            RxStats.connectionOpened ()
            let ct = ctx.RequestAborted
            let maxBytes =
                parseBoundedPositiveIntEnv
                    "MAX_WS_TEXT_FRAME_BYTES"
                    defaultMaxTextFrameBytes
                    maxTextFrameBytesCeiling
            try
                try
                    let mutable run = true
                    while run do
                        let! frame = receiveTextFrame "rx-publish" logger ws ct maxBytes
                        match frame with
                        | CloseFrame ->
                            do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                            run <- false
                        | TextFrame raw ->
                            RxStats.messageIn (Encoding.UTF8.GetByteCount(raw: string))
                            // Expected shape:
                            //   {"conv_id":"<uuid>", "kind":"message",
                            //    "payload": { ... arbitrary ... }}
                            // event_id is generated server-side. We surface
                            // parse errors as an `ok:false` reply rather than
                            // dropping the connection — easier to test.
                            let reply =
                                try
                                    let root = JsonNode.Parse(raw)
                                    let convStr =
                                        match root.["conv_id"] with
                                        | null -> ""
                                        | v -> v.ToString()
                                    let kind =
                                        match root.["kind"] with
                                        | null -> "message"
                                        | v -> v.ToString()
                                    let payloadNode = root.["payload"]
                                    let payloadJson =
                                        if isNull payloadNode then "null"
                                        else payloadNode.ToJsonString()
                                    match Guid.TryParse(convStr) with
                                    | false, _ ->
                                        Task.FromResult(
                                            "{\"ok\":false,\"error\":\"conv_id missing or invalid\"}")
                                    | true, convId ->
                                        task {
                                            let eid = Guid.NewGuid()
                                            let presence = presence ()
                                            let! dbResult =
                                                match presence with
                                                | Some s when s.DbConnectionString.IsSome ->
                                                    task {
                                                        try
                                                            let! r =
                                                                writeEventToDb
                                                                    s.DbConnectionString.Value
                                                                    eid kind convId payloadJson ct
                                                            return Some r
                                                        with ex ->
                                                            logger.LogWarning(
                                                                ex,
                                                                "ws[rx-publish] DB write failed; falling back to ephemeral")
                                                            return None
                                                    }
                                                | _ -> Task.FromResult(None)

                                            let seq, occurred =
                                                match dbResult with
                                                | Some r -> r
                                                | None -> (-1L, DateTime.UtcNow)
                                            let evt = {
                                                EventId    = eid
                                                Seq        = seq
                                                Kind       = kind
                                                ConvId     = convId
                                                Payload    = payloadJson
                                                OccurredAt = occurred
                                                Source     = WsPublishSrc
                                            }
                                            // Inject into the fan-in graph
                                            // first — that's the lowest-
                                            // latency path so local
                                            // /ws/rx-presence subscribers see
                                            // the event before any cross-pod
                                            // delivery wakes up.
                                            match presence with
                                            | Some s -> s.FanIn.Inject(evt)
                                            | None -> ()
                                            // Then publish to NATS so other
                                            // pods / consumers see it too.
                                            match presence with
                                            | Some s when s.Nats.IsSome ->
                                                try do! s.Nats.Value.PublishEvent(evt)
                                                with ex ->
                                                    logger.LogWarning(
                                                        ex, "ws[rx-publish] NATS publish failed")
                                            | _ -> ()
                                            return
                                                sprintf
                                                    "{\"ok\":true,\"event_id\":\"%O\",\"seq\":%d,\"durable\":%s,\"nats\":%s}"
                                                    eid seq
                                                    (if dbResult.IsSome then "true" else "false")
                                                    (match presence with
                                                     | Some s when s.Nats.IsSome -> "true"
                                                     | _ -> "false")
                                        }
                                with ex ->
                                    Task.FromResult(
                                        sprintf "{\"ok\":false,\"error\":%s}"
                                            (jsonString
                                                (sprintf "%s: %s"
                                                    (ex.GetType().Name) ex.Message)))
                            let! replyText = reply
                            let bytes = Encoding.UTF8.GetBytes(replyText: string)
                            do! ws.SendAsync(
                                    ArraySegment(bytes),
                                    WebSocketMessageType.Text,
                                    true, ct)
                            RxStats.messageOut bytes.Length
                with
                | :? OperationCanceledException -> ()
                | ex -> logger.LogWarning(ex, "ws[rx-publish] connection ended")
            finally
                RxStats.connectionClosed ()
    }

let handleRxNatsEcho (ctx: HttpContext) : Task =
    task {
        let factory = ctx.RequestServices.GetRequiredService<ILoggerFactory>()
        let logger = factory.CreateLogger("WsRoutes:rx-nats-echo")
        match presence () with
        | None -> 
            ctx.Response.StatusCode <- 503
            do! ctx.Response.WriteAsync("presence subsystem not initialised\n")
        | Some state ->
            match state.Nats with
            | None ->
                ctx.Response.StatusCode <- 503
                do! ctx.Response.WriteAsync("nats not configured (NATS_URL unset)\n")
            | Some nats ->
                if not ctx.WebSockets.IsWebSocketRequest then
                    ctx.Response.StatusCode <- 400
                    do! ctx.Response.WriteAsync("expected websocket upgrade\n")
                else
                    use! ws = ctx.WebSockets.AcceptWebSocketAsync()
                    RxStats.connectionOpened ()
                    let ct = ctx.RequestAborted
                    let maxBytes =
                        parseBoundedPositiveIntEnv
                            "MAX_WS_TEXT_FRAME_BYTES"
                            defaultMaxTextFrameBytes
                            maxTextFrameBytesCeiling
                    try
                        try
                            // First frame: `{"subject":"some.subject"}`
                            let! firstFrame = receiveTextFrame "rx-nats-echo" logger ws ct maxBytes
                            match firstFrame with
                            | CloseFrame ->
                                do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                            | TextFrame raw ->
                                RxStats.messageIn (Encoding.UTF8.GetByteCount(raw: string))
                                let subjectName =
                                    try
                                        let root = JsonNode.Parse(raw)
                                        match root.["subject"] with
                                        | null -> NatsRx.echoSubjectPrefix + "default"
                                        | v ->
                                            let s = v.ToString()
                                            if String.IsNullOrWhiteSpace(s) then
                                                NatsRx.echoSubjectPrefix + "default"
                                            else s
                                    with _ ->
                                        NatsRx.echoSubjectPrefix + "default"
                                logger.LogInformation(
                                    "ws[rx-nats-echo]: bound to NATS subject {Subject}",
                                    subjectName)

                                let outChan = Channel.CreateUnbounded<string>()
                                let onNext (msg: NatsMsg<byte array>) =
                                    let body =
                                        match msg.Data with
                                        | null -> ""
                                        | b -> Encoding.UTF8.GetString(b)
                                    let env =
                                        sprintf
                                            "{\"from\":\"nats\",\"subject\":%s,\"body\":%s}"
                                            (jsonString msg.Subject)
                                            (jsonString body)
                                    outChan.Writer.TryWrite(env) |> ignore
                                use _natsSub =
                                    System.ObservableExtensions.Subscribe(
                                        nats.SubscribeRaw(subjectName),
                                        Action<NatsMsg<byte array>>(onNext))

                                let ack =
                                    sprintf "{\"ok\":true,\"subject\":%s}"
                                        (jsonString subjectName)
                                let ackBytes = Encoding.UTF8.GetBytes(ack: string)
                                do! ws.SendAsync(
                                        ArraySegment(ackBytes),
                                        WebSocketMessageType.Text,
                                        true, ct)
                                RxStats.messageOut ackBytes.Length

                                let sender =
                                    task {
                                        let mutable run = true
                                        while run && not ct.IsCancellationRequested do
                                            let! more = outChan.Reader.WaitToReadAsync(ct).AsTask()
                                            if not more then run <- false
                                            else
                                                let mutable frame : string = null
                                                while outChan.Reader.TryRead(&frame) do
                                                    let bytes = Encoding.UTF8.GetBytes(frame: string)
                                                    do! ws.SendAsync(
                                                            ArraySegment(bytes),
                                                            WebSocketMessageType.Text,
                                                            true, ct)
                                                    RxStats.messageOut bytes.Length
                                    } :> Task

                                try
                                    let mutable run = true
                                    while run do
                                        let! frame =
                                            receiveTextFrame "rx-nats-echo" logger ws ct maxBytes
                                        match frame with
                                        | CloseFrame ->
                                            do! closeIfOpen ws WebSocketCloseStatus.NormalClosure "bye" ct
                                            run <- false
                                        | TextFrame body ->
                                            RxStats.messageIn (Encoding.UTF8.GetByteCount(body: string))
                                            // Bounce every inbound WS frame to
                                            // NATS — the round trip is then
                                            // visible to *this* same connection
                                            // (because we're subscribed to the
                                            // same subject) and to anything
                                            // else subscribed.
                                            try
                                                do! nats.PublishRaw subjectName body
                                            with ex ->
                                                logger.LogWarning(
                                                    ex,
                                                    "ws[rx-nats-echo] NATS publish failed")
                                finally
                                    outChan.Writer.TryComplete() |> ignore
                                try do! sender
                                with _ -> ()
                        with
                        | :? OperationCanceledException -> ()
                        | ex -> logger.LogWarning(ex, "ws[rx-nats-echo] connection ended")
                    finally
                        RxStats.connectionClosed ()
    }

let handleRxStatsSources (ctx: HttpContext) : Task =
    let body =
        match presence () with
        | None ->
            "{\"ok\":false,\"error\":\"presence subsystem not initialised\"}"
        | Some state ->
            let pgNotifyDelivered =
                match state.PgListen with
                | Some h -> h.DeliveredCount() | None -> 0L
            let pgWalDelivered =
                match state.PgWal with
                | Some h -> h.DeliveredCount() | None -> 0L
            let pgWalSlot =
                match state.PgWal with
                | Some h -> sprintf "%s" h.SlotName | None -> ""
            let pgOutboxDelivered =
                match state.PgOutbox with
                | Some h -> h.DeliveredCount() | None -> 0L
            let pgOutboxLastSeq =
                match state.PgOutbox with
                | Some h -> h.LastSeq() | None -> 0L
            let natsDelivered =
                match state.Nats with
                | Some h -> h.DeliveredCount() | None -> 0L
            let natsPublished =
                match state.Nats with
                | Some h -> h.PublishedCount() | None -> 0L
            System.String.Format(
                System.Globalization.CultureInfo.InvariantCulture,
                "{{\"pg_notify\":{{\"available\":{0},\"delivered\":{1}}},\
                  \"pg_wal\":{{\"available\":{2},\"delivered\":{3},\"slot\":{4}}},\
                  \"pg_outbox\":{{\"available\":{5},\"delivered\":{6},\"last_seq\":{7}}},\
                  \"nats\":{{\"available\":{8},\"delivered\":{9},\"published\":{10}}},\
                  \"fan_in\":{{\"dedup_hits\":{11},\"dedup_misses\":{12},\"cache_size\":{13}}}}}",
                (if state.PgListen.IsSome then "true" else "false"),
                pgNotifyDelivered,
                (if state.PgWal.IsSome then "true" else "false"),
                pgWalDelivered,
                jsonString pgWalSlot,
                (if state.PgOutbox.IsSome then "true" else "false"),
                pgOutboxDelivered,
                pgOutboxLastSeq,
                (if state.Nats.IsSome then "true" else "false"),
                natsDelivered,
                natsPublished,
                state.FanIn.DedupHitCount(),
                state.FanIn.DedupMissCount(),
                state.FanIn.DedupCurrentSize())
    ctx.Response.ContentType <- "application/json"
    ctx.Response.WriteAsync(body)

let private promMetric (name: string) (metricType: string) (help: string) (value: string) : string =
    sprintf "# HELP %s %s\n# TYPE %s %s\n%s %s\n" name help name metricType name value

let private promIntMetric (name: string) (metricType: string) (help: string) (value: int64) : string =
    promMetric name metricType help (value.ToString(System.Globalization.CultureInfo.InvariantCulture))

let private promFloatMetric (name: string) (metricType: string) (help: string) (value: float) : string =
    promMetric name metricType help (value.ToString("0.###", System.Globalization.CultureInfo.InvariantCulture))

let handleMetrics (ctx: HttpContext) : Task =
    let snap = RxStats.snapshot ()
    let body =
        [ promIntMetric
              "dd_fsharp_ws_open_connections"
              "gauge"
              "Current open WebSocket connections handled by dd-fsharp-ws-server."
              (int64 snap.openConnections)
          promIntMetric
              "dd_fsharp_ws_messages_in_total"
              "counter"
              "Total text frames received by dd-fsharp-ws-server."
              snap.messagesIn
          promIntMetric
              "dd_fsharp_ws_messages_out_total"
              "counter"
              "Total text frames sent by dd-fsharp-ws-server."
              snap.messagesOut
          promIntMetric
              "dd_fsharp_ws_bytes_in_total"
              "counter"
              "Total text-frame bytes received by dd-fsharp-ws-server."
              snap.bytesIn
          promIntMetric
              "dd_fsharp_ws_bytes_out_total"
              "counter"
              "Total text-frame bytes sent by dd-fsharp-ws-server."
              snap.bytesOut
          promFloatMetric
              "dd_fsharp_ws_uptime_seconds"
              "gauge"
              "Process uptime for dd-fsharp-ws-server."
              (float snap.uptimeMs / 1000.0) ]
        |> String.concat "\n"

    ctx.Response.ContentType <- "text/plain; version=0.0.4; charset=utf-8"
    ctx.Response.WriteAsync(body)

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
      <tr><td>WS</td><td><code>/ws/rx-sample</code></td><td>same input, output <code>Sample(100ms)</code> — at most the latest completed reply every dashboard tick</td></tr>
      <tr><td>WS</td><td><code>/ws/rx-burst</code></td><td>same input, output <code>Timestamp → Buffer(250ms, 64) → Scan</code> — stateful per-connection load windows</td></tr>
    </tbody>
  </table>

  <h2 class="grp">Live process telemetry (Rx <code>BehaviorSubject</code> + <code>ReplaySubject</code> + SSE)</h2>
  <table>
    <tbody>
      <tr><td>GET</td><td><a href="/v1/rx-stats"><code>/v1/rx-stats</code></a></td><td>current open-connections / msgs / bytes (JSON, one-shot)</td></tr>
      <tr><td>GET</td><td><a href="/v1/rx-stats/history"><code>/v1/rx-stats/history</code></a></td><td>last ~120 snapshots, replayed off the <code>ReplaySubject</code> buffer</td></tr>
      <tr><td>SSE</td><td><a href="/sse/rx-stats"><code>/sse/rx-stats</code></a></td><td>1 Hz Server-Sent Events feed of the live snapshot</td></tr>
      <tr><td>GET</td><td><a href="/v1/rx-stats/sources"><code>/v1/rx-stats/sources</code></a></td><td>per-source counters (pg-notify / pg-wal / pg-outbox / nats / fan-in dedup)</td></tr>
    </tbody>
  </table>

  <h2 class="grp">Presence pipeline (Rx fan-in: NATS &#8741; PG LISTEN/NOTIFY &#8741; PG WAL &#8741; PG outbox)</h2>
  <table>
    <tbody>
      <tr><td>WS</td><td><code>/ws/rx-presence</code></td><td>subscribe to the merged, deduped, per-conv throttled hot observable. First frame: <code>{"conv_ids":[…]}</code> (empty = all).</td></tr>
      <tr><td>WS</td><td><code>/ws/rx-publish</code></td><td>write events into <code>fsws_events</code>. Frame shape: <code>{"conv_id":"…","kind":"message","payload":{…}}</code>. Trigger fires NOTIFY, WAL captures the row, NATS broadcasts — all three paths come back through the fan-in and are dedup'd on <code>event_id</code>.</td></tr>
      <tr><td>WS</td><td><code>/ws/rx-nats-echo</code></td><td>raw NATS round-trip demo. First frame: <code>{"subject":"…"}</code>. Subsequent inbound frames are <code>Publish</code>'d to that subject; any message on it is delivered back.</td></tr>
    </tbody>
  </table>

  <p>
    The five-stage pipeline is
    <code>parse → validate → enrich (lookupA &#8741; lookupB) → score → serialize</code>.
    The per-stage work is byte-for-byte identical between every implementation;
    only the orchestration around it differs. See the
    <a href="https://github.com/ORESoftware/k8s-cluster/blob/dev/remote/deployments/fsharp-ws-server/readme.md">readme</a>
    for the long-form comparison.
  </p>
</body>
</html>
"""
    ctx.Response.ContentType <- "text/html; charset=utf-8"
    ctx.Response.WriteAsync(html)
