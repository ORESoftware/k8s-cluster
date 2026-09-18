module OresSoftware.Dd.FsWs.Program

open System
open System.IO
open System.Threading.Tasks
open Microsoft.AspNetCore.Builder
open Microsoft.AspNetCore.Http
open Microsoft.AspNetCore.Routing
open Microsoft.Extensions.DependencyInjection
open Microsoft.Extensions.Hosting
open Microsoft.Extensions.Logging
open OresSoftware.Dd.FsWs.PgSchema
open OresSoftware.Dd.FsWs.PgListen
open OresSoftware.Dd.FsWs.PgWal
open OresSoftware.Dd.FsWs.PgOutbox
open OresSoftware.Dd.FsWs.NatsRx
open OresSoftware.Dd.FsWs.PresenceFanIn
open OresSoftware.Dd.FsWs.WsRoutes

let private toReqDelegate (handler: HttpContext -> Task) : RequestDelegate =
    RequestDelegate(fun ctx -> handler ctx)

let private readGeneratedFile (fileName: string) : string =
    let candidates =
        [|
            Path.Combine(AppContext.BaseDirectory, "generated", fileName)
            Path.Combine(Environment.CurrentDirectory, "generated", fileName)
        |]

    match candidates |> Array.tryFind File.Exists with
    | Some path -> File.ReadAllText path
    | None ->
        failwithf
            "generated API docs file %s not found in %s"
            fileName
            (String.concat ", " candidates)

let private writeGeneratedFile
        (contentType: string)
        (fileName: string)
        (ctx: HttpContext)
        : Task =
    task {
        ctx.Response.ContentType <- contentType
        do! ctx.Response.WriteAsync(readGeneratedFile fileName)
    }

let private handleApiDocsHtml (ctx: HttpContext) : Task =
    writeGeneratedFile "text/html; charset=utf-8" "api-docs.html" ctx

let private handleApiDocsJson (ctx: HttpContext) : Task =
    writeGeneratedFile "application/json; charset=utf-8" "api-docs.json" ctx

/// Process entry point for dd-fsharp-ws-server. Boots an ASP.NET Core / Kestrel
/// host, wires up every Rx-based pipeline + transport, and exposes them through
/// WS / HTTP / SSE endpoints. Pattern mirrors `dd-akka-ws-server` so the same
/// loadtest harness drives the comparison pipelines; the presence subsystem
/// (NATS + PG LISTEN/NOTIFY + PG WAL + outbox) is bootstrapped here with the
/// same "graceful degrade" posture dd-gleamlang-presence-server uses.

let private envOrDefault (name: string) (fallback: string) : string =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | v -> v

let private envOpt (name: string) : string option =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> None
    | v -> Some v

let private parsePort (name: string) (fallback: int) : int =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | raw ->
        match Int32.TryParse(raw.Trim()) with
        | true, v when v > 0 && v < 65536 -> v
        | _ -> fallback

let private parseMsEnv (name: string) (fallback: int) : int =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | raw ->
        match Int32.TryParse(raw.Trim()) with
        | true, v when v > 0 -> v
        | _ -> fallback

let private parseBoolEnv (name: string) (fallback: bool) : bool =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | raw ->
        match Boolean.TryParse(raw.Trim()) with
        | true, v -> v
        | _ -> fallback

let private bootPresence
        (logger: ILogger)
        : Task<PresenceState> =
    task {
        // Pull every env var first so failures in one source don't change
        // what the others see. Defaults match dd-gleamlang-presence-server's
        // posture: anything unset → that source is silently skipped.
        let pgUrl  = envOpt "PG_DATABASE_URL"
        let natsUrl = envOpt "NATS_URL"
        let walPollMs    = parseMsEnv "FSWS_WAL_POLL_MS" 250
        let outboxPollMs = parseMsEnv "FSWS_OUTBOX_POLL_MS" 1000
        let outboxBatch  = parseMsEnv "FSWS_OUTBOX_BATCH" 256
        let outboxBackfill = parseBoolEnv "FSWS_OUTBOX_BACKFILL" false
        let walEnabled = parseBoolEnv "FSWS_WAL_ENABLED" false

        // --- Postgres ---------------------------------------------------------
        let mutable connString : string option = None
        let mutable pgListenH  : PgListenHandle option = None
        let mutable pgWalH     : PgWalHandle    option = None
        let mutable pgOutboxH  : PgOutboxHandle option = None

        match pgUrl with
        | None ->
            logger.LogInformation("boot: PG_DATABASE_URL unset, skipping PG sources")
        | Some uri ->
            let cs = pgUriToConnString uri
            // Idempotent migration. If it fails (DNS resolution, wrong
            // credentials, etc) we log and continue without PG — the
            // existing WS endpoints / RxAdvanced / NATS still work.
            let! migrated = migrate logger cs
            if migrated then
                connString <- Some cs
                pgListenH <- Some (PgListen.start logger cs)
                if walEnabled then
                    let! slotName = ensureWalSlot logger cs
                    match slotName with
                    | Some s ->
                        pgWalH <-
                            Some (
                                PgWal.start
                                    logger cs s
                                    (TimeSpan.FromMilliseconds(float walPollMs)))
                    | None -> ()
                else
                    logger.LogInformation(
                        "boot: FSWS_WAL_ENABLED=false, skipping PG WAL slot")
                pgOutboxH <-
                    Some (
                        PgOutbox.start
                            logger cs
                            (TimeSpan.FromMilliseconds(float outboxPollMs))
                            outboxBatch
                            outboxBackfill)
            else
                logger.LogWarning(
                    "boot: PG migration failed; PG-backed sources disabled \
                     (other endpoints still functional)")

        // --- NATS -------------------------------------------------------------
        let mutable natsH : NatsHandle option = None
        match natsUrl with
        | None ->
            logger.LogInformation("boot: NATS_URL unset, skipping NATS transport")
        | Some url ->
            try
                let! h = NatsRx.start logger url
                natsH <- Some h
            with ex ->
                logger.LogWarning(
                    ex,
                    "boot: NATS connection failed; transport disabled")

        // --- Fan-in graph -----------------------------------------------------
        // Always start, even with zero sources — /ws/rx-publish can still
        // inject locally and /ws/rx-presence will simply see only injected
        // events.
        let fanIn =
            PresenceFanIn.start
                logger
                PresenceFanIn.defaultOptions
                (pgListenH |> Option.map (fun h -> h.Events))
                (pgWalH    |> Option.map (fun h -> h.Events))
                (pgOutboxH |> Option.map (fun h -> h.Events))
                (natsH     |> Option.map (fun h -> h.Events))

        let summary =
            sprintf
                "pg-listen=%b pg-wal=%b pg-outbox=%b nats=%b"
                pgListenH.IsSome pgWalH.IsSome pgOutboxH.IsSome natsH.IsSome
        logger.LogInformation("presence boot summary: {Summary}", summary)

        return {
            DbConnectionString = connString
            PgListen           = pgListenH
            PgWal              = pgWalH
            PgOutbox           = pgOutboxH
            Nats               = natsH
            FanIn              = fanIn
        }
    }

[<EntryPoint>]
let main args =
    let host = envOrDefault "HTTP_HOST" "0.0.0.0"
    let port = parsePort   "HTTP_PORT" 8087

    let builder = WebApplication.CreateBuilder(args)

    builder.Logging
        .ClearProviders()
        .AddSimpleConsole(fun o ->
            o.SingleLine <- true
            o.TimestampFormat <- "yyyy-MM-ddTHH:mm:ss.fffZ ")
    |> ignore

    let app = builder.Build()
    app.Urls.Add(sprintf "http://%s:%d" host port)

    let wsOpts = WebSocketOptions()
    wsOpts.KeepAliveInterval <- TimeSpan.FromSeconds 30.0
    app.UseWebSockets wsOpts |> ignore

    let logger =
        app.Services.GetRequiredService<ILoggerFactory>()
           .CreateLogger("dd-fsharp-ws-server")

    // Boot the presence subsystem BEFORE Run so handlers see a ready state
    // on the first request. `bootPresence` itself is fully fault-tolerant —
    // it always returns a PresenceState (possibly with all sources None).
    let presenceState = (bootPresence logger).GetAwaiter().GetResult()
    setPresenceState presenceState

    app.MapGet("/",                       toReqDelegate handleIndex)            |> ignore
    app.MapGet("/healthz",                toReqDelegate handleHealth)           |> ignore
    app.MapGet("/readyz",                 toReqDelegate handleReady)            |> ignore
    app.MapGet("/livez",                  toReqDelegate handleLive)             |> ignore
    app.MapGet("/metrics",                toReqDelegate handleMetrics)          |> ignore
    app.MapGet("/docs/api",               toReqDelegate handleApiDocsHtml)      |> ignore
    app.MapGet("/api/docs",               toReqDelegate handleApiDocsHtml)      |> ignore
    app.MapGet("/api/docs.json",          toReqDelegate handleApiDocsJson)      |> ignore
    app.MapGet("/v1/benchmark",           toReqDelegate handleBenchmark)        |> ignore
    app.MapGet("/v1/rx-stats",            toReqDelegate handleRxStats)          |> ignore
    app.MapGet("/v1/rx-stats/history",    toReqDelegate handleRxStatsHistory)   |> ignore
    app.MapGet("/v1/rx-stats/sources",    toReqDelegate handleRxStatsSources)   |> ignore
    app.MapGet("/sse/rx-stats",           toReqDelegate handleRxStatsSse)       |> ignore
    app.MapGet("/ws/rx",                  toReqDelegate handleRx)               |> ignore
    app.MapGet("/ws/async",               toReqDelegate handleAsync)            |> ignore
    app.MapGet("/ws/rx-stream",           toReqDelegate handleRxStream)         |> ignore
    app.MapGet("/ws/rx-window",           toReqDelegate handleRxWindow)         |> ignore
    app.MapGet("/ws/rx-throttle",         toReqDelegate handleRxThrottle)       |> ignore
    app.MapGet("/ws/rx-sample",           toReqDelegate handleRxSample)         |> ignore
    app.MapGet("/ws/rx-burst",            toReqDelegate handleRxBurst)          |> ignore
    app.MapGet("/ws/rx-presence",         toReqDelegate handleRxPresence)       |> ignore
    app.MapGet("/ws/rx-publish",          toReqDelegate handleRxPublish)        |> ignore
    app.MapGet("/ws/rx-nats-echo",        toReqDelegate handleRxNatsEcho)       |> ignore

    logger.LogInformation("dd-fsharp-ws-server listening on {Host}:{Port}", host, port)
    app.Run()
    0
