module OresSoftware.Dd.FsWs.PgListen

open System
open System.Globalization
open System.Reactive.Subjects
open System.Text.Json
open System.Text.Json.Nodes
open System.Threading
open System.Threading.Tasks
open Microsoft.Extensions.Logging
open Npgsql
open OresSoftware.Dd.FsWs.PgSchema

/// Postgres LISTEN/NOTIFY as a hot `IObservable<UnifiedEvent>`.
///
/// One long-lived `NpgsqlConnection` per process executes `LISTEN
/// fsws_change_0`..`fsws_change_15` (the same 16-way shard space the trigger
/// uses) and then sits in a `WaitAsync` loop. Every NOTIFY received fires
/// `Notification` which we marshal into the unified `Subject<UnifiedEvent>`.
///
/// Why a single connection rather than one-per-shard: the F# server is a
/// single consumer for now; nothing is gained by sharding subscribers
/// further. The sharding lives in the NOTIFY channel name purely so multiple
/// pods can each LISTEN to a subset and load-balance, *if* we ever scale out.
///
/// Reconnect strategy: any unhandled exception in the wait loop closes the
/// connection and bubbles up to the outer task, which uses an exponential
/// backoff with jitter before re-running the whole open / LISTEN / wait
/// sequence. Same posture as the Gleam reference. The Subject is *not*
/// re-created on reconnect — downstream subscribers stay attached for the
/// lifetime of the process.

type PgListenHandle = {
    Events: IObservable<UnifiedEvent>
    Stop:   unit -> Task
    /// Returns the running count for the per-source counter exposed by
    /// `/v1/rx-stats/sources`. Read-only snapshot — incremented inside the
    /// notify handler.
    DeliveredCount: unit -> int64
}

let private shardChannels =
    [| for i in 0 .. 15 -> sprintf "fsws_change_%d" i |]

let private tryReadGuid (node: JsonNode) (key: string) : Guid option =
    match node.[key] with
    | null -> None
    | v ->
        match Guid.TryParse(v.ToString()) with
        | true, g -> Some g
        | _ -> None

let private tryReadInt64 (node: JsonNode) (key: string) : int64 =
    match node.[key] with
    | null -> -1L
    | v ->
        match Int64.TryParse(
                v.ToString(),
                NumberStyles.Integer,
                CultureInfo.InvariantCulture) with
        | true, n -> n
        | _ -> -1L

let private tryReadString (node: JsonNode) (key: string) (fallback: string) : string =
    match node.[key] with
    | null -> fallback
    | v ->
        let s = v.ToString()
        if isNull s then fallback else s

let private parseNotifyPayload
        (logger: ILogger)
        (payload: string)
        : UnifiedEvent option =
    try
        let root = JsonNode.Parse(payload)
        match tryReadGuid root "event_id", tryReadGuid root "conv_id" with
        | Some eid, Some cid ->
            let occurredRaw =
                tryReadString root "occurred_at"
                    (DateTime.UtcNow.ToString("o", CultureInfo.InvariantCulture))
            let occurred =
                match DateTime.TryParse(
                        occurredRaw,
                        CultureInfo.InvariantCulture,
                        DateTimeStyles.AssumeUniversal ||| DateTimeStyles.AdjustToUniversal) with
                | true, dt -> dt
                | _ -> DateTime.UtcNow
            Some {
                EventId    = eid
                Seq        = tryReadInt64 root "seq"
                Kind       = tryReadString root "kind" "unknown"
                ConvId     = cid
                Payload    = payload
                OccurredAt = occurred
                Source     = PgNotifySrc
            }
        | _ ->
            logger.LogDebug(
                "pg-listen: ignored notify payload without event_id/conv_id: {Payload}",
                payload)
            None
    with ex ->
        logger.LogWarning(
            ex,
            "pg-listen: failed to parse notify payload: {Payload}",
            payload)
        None

let private listenLoop
        (logger: ILogger)
        (connectionString: string)
        (subject: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (ct: CancellationToken)
        : Task =
    task {
        let mutable backoffMs = 250
        while not ct.IsCancellationRequested do
            try
                use conn = new NpgsqlConnection(connectionString)
                // We have to subscribe to the .NET event BEFORE OpenAsync;
                // Npgsql guarantees no notification fires before Open().
                let notifyHandler =
                    NotificationEventHandler(fun _ args ->
                        match parseNotifyPayload logger args.Payload with
                        | Some evt ->
                            Interlocked.Increment(&counter.contents) |> ignore
                            subject.OnNext(evt)
                        | None -> ())
                conn.Notification.AddHandler(notifyHandler)
                try
                    do! conn.OpenAsync(ct)
                    for chan in shardChannels do
                        use cmd =
                            new NpgsqlCommand(sprintf "LISTEN %s" chan, conn)
                        let! _ = cmd.ExecuteNonQueryAsync(ct)
                        ()
                    logger.LogInformation(
                        "pg-listen: LISTENing on {Count} shard channels",
                        shardChannels.Length)
                    // Reset backoff after a clean attach — only the *next*
                    // failure should slow us down.
                    backoffMs <- 250
                    // Block on WaitAsync forever; this is the canonical
                    // Npgsql idle-listen idiom.
                    while not ct.IsCancellationRequested do
                        do! conn.WaitAsync(ct)
                finally
                    conn.Notification.RemoveHandler(notifyHandler)
            with
            | :? OperationCanceledException -> ()
            | ex ->
                logger.LogWarning(
                    ex,
                    "pg-listen: connection broke; reconnecting in {Ms} ms",
                    backoffMs)
                try
                    do! Task.Delay(backoffMs, ct)
                with :? OperationCanceledException -> ()
                // Exponential backoff capped at 10s; small jitter so multi-pod
                // restarts don't pile up.
                let jitter = Random.Shared.Next(0, 250)
                backoffMs <- min 10000 (backoffMs * 2) + jitter
    }

let start
        (logger: ILogger)
        (connectionString: string)
        : PgListenHandle =
    let subject = new Subject<UnifiedEvent>()
    let counter = ref 0L
    let cts = new CancellationTokenSource()
    // Fire-and-forget — the loop reconnects on its own and only exits when
    // `cts` is cancelled.
    let _task = listenLoop logger connectionString subject counter cts.Token
    {
        Events = subject :> IObservable<UnifiedEvent>
        Stop = fun () ->
            task {
                cts.Cancel()
                subject.OnCompleted()
                subject.Dispose()
            } :> Task
        DeliveredCount = fun () -> Volatile.Read(&counter.contents)
    }
