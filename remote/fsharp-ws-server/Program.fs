module OresSoftware.Dd.FsWs.Program

open System
open System.Threading.Tasks
open Microsoft.AspNetCore.Builder
open Microsoft.AspNetCore.Http
open Microsoft.AspNetCore.Routing
open Microsoft.Extensions.DependencyInjection
open Microsoft.Extensions.Hosting
open Microsoft.Extensions.Logging
open OresSoftware.Dd.FsWs.WsRoutes

let private toReqDelegate (handler: HttpContext -> Task) : RequestDelegate =
    RequestDelegate(fun ctx -> handler ctx)

/// Process entry point for dd-fsharp-ws-server. Boots an ASP.NET Core / Kestrel
/// host, wires up the two pipeline implementations (Rx.NET + native F# task),
/// and exposes each pipeline as its own WebSocket endpoint plus a side-by-side
/// micro-benchmark route. Pattern mirrors `dd-akka-ws-server` so the same
/// loadtest harness drives both services.

let private envOrDefault (name: string) (fallback: string) : string =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | v -> v

let private parsePort (name: string) (fallback: int) : int =
    match Environment.GetEnvironmentVariable name with
    | null | "" -> fallback
    | raw ->
        match Int32.TryParse(raw.Trim()) with
        | true, v when v > 0 && v < 65536 -> v
        | _ -> fallback

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

    // Allow large-ish WebSocket frames so benchmark payloads up to ~32 KiB pass
    // through without a frame split. Production traffic is well under this.
    let wsOpts = WebSocketOptions()
    wsOpts.KeepAliveInterval <- TimeSpan.FromSeconds 30.0
    app.UseWebSockets wsOpts |> ignore

    app.MapGet("/",                       toReqDelegate handleIndex)           |> ignore
    app.MapGet("/healthz",                toReqDelegate handleHealth)          |> ignore
    app.MapGet("/readyz",                 toReqDelegate handleReady)           |> ignore
    app.MapGet("/livez",                  toReqDelegate handleLive)            |> ignore
    app.MapGet("/v1/benchmark",           toReqDelegate handleBenchmark)       |> ignore
    app.MapGet("/v1/rx-stats",            toReqDelegate handleRxStats)         |> ignore
    app.MapGet("/v1/rx-stats/history",    toReqDelegate handleRxStatsHistory)  |> ignore
    app.MapGet("/sse/rx-stats",           toReqDelegate handleRxStatsSse)      |> ignore
    app.MapGet("/ws/rx",                  toReqDelegate handleRx)              |> ignore
    app.MapGet("/ws/async",               toReqDelegate handleAsync)           |> ignore
    app.MapGet("/ws/rx-stream",           toReqDelegate handleRxStream)        |> ignore
    app.MapGet("/ws/rx-window",           toReqDelegate handleRxWindow)        |> ignore
    app.MapGet("/ws/rx-throttle",         toReqDelegate handleRxThrottle)      |> ignore
    app.MapGet("/ws/rx-sample",           toReqDelegate handleRxSample)        |> ignore

    let logger =
        app.Services.GetRequiredService<ILoggerFactory>()
           .CreateLogger("dd-fsharp-ws-server")

    logger.LogInformation("dd-fsharp-ws-server listening on {Host}:{Port}", host, port)
    app.Run()
    0
