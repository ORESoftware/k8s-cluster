module OresSoftware.Dd.FsWs.PgWal

open System
open System.Collections.Generic
open System.Globalization
open System.Reactive.Subjects
open System.Text.Json.Nodes
open System.Threading
open System.Threading.Tasks
open Microsoft.Extensions.Logging
open Npgsql
open OresSoftware.Dd.FsWs.PgSchema

/// Logical-replication CDC via `pg_logical_slot_get_changes`.
///
/// The Gleam reference is the source of truth for the *protocol*. We mirror
/// it almost exactly, just translated to F#:
///
///   1. `select fsws_wal_available()` — bail with a friendly log message if
///      the operator hasn't enabled logical replication.
///   2. `select fsws_ensure_wal_slot('fsws_wal_<pod>')` — idempotent.
///   3. Poll loop calls
///          select * from pg_logical_slot_get_changes(
///              'fsws_wal_<pod>', NULL, NULL,
///              'add-tables', 'public.fsws_events',
///              'format-version', '2')
///      every `pollInterval`. Each returned row is one wal2json line per
///      change.
///
/// wal2json format-version 2 emits a JSON object per change with fields:
///     action  : "I" | "U" | "D" | "T" | "M" | "B" | "C"
///     schema  : "public"
///     table   : "fsws_events"
///     columns : [{name, type, value}, ...]
///     identity: same shape (only on U/D)
///
/// We pivot the `columns` array into a name → value map and rebuild a
/// `UnifiedEvent`. Slot advancement is automatic — `_get_changes` (as
/// opposed to `_peek_changes`) is what advances `confirmed_flush_lsn`. If
/// we crash mid-batch the slot redelivers from the current position, and
/// the dedup cache in PresenceFanIn absorbs the overlap.

type PgWalHandle = {
    Events: IObservable<UnifiedEvent>
    Stop:   unit -> Task
    DeliveredCount: unit -> int64
    SlotName: string
}

let private columnsToMap (columns: JsonArray) : Dictionary<string, JsonNode> =
    let d = Dictionary<string, JsonNode>(StringComparer.OrdinalIgnoreCase)
    for col in columns do
        let name = col.["name"].ToString()
        d.[name] <- col.["value"]
    d

let private tryColumnString (cols: Dictionary<string, JsonNode>) (key: string) : string option =
    match cols.TryGetValue(key) with
    | true, v when not (isNull v) -> Some (v.ToString())
    | _ -> None

let private parseChangeRow
        (logger: ILogger)
        (jsonLine: string)
        : UnifiedEvent option =
    try
        let root = JsonNode.Parse(jsonLine)
        let action =
            match root.["action"] with
            | null -> ""
            | v -> v.ToString()
        // We only care about row-level data changes — Insert, Update, Delete.
        // Transaction-boundary actions (B, C, T, M) are noise for our use.
        if action <> "I" && action <> "U" && action <> "D" then
            None
        else
            // For DELETE the identity columns are populated; for I/U the
            // post-image columns are. Try post-image first, fall back to
            // identity.
            let cols =
                match root.["columns"] with
                | :? JsonArray as a when a.Count > 0 -> columnsToMap a
                | _ ->
                    match root.["identity"] with
                    | :? JsonArray as a -> columnsToMap a
                    | _ -> Dictionary<_, _>()
            let eidOpt =
                tryColumnString cols "event_id"
                |> Option.bind (fun s ->
                    match Guid.TryParse(s) with
                    | true, g -> Some g
                    | _ -> None)
            let cidOpt =
                tryColumnString cols "conv_id"
                |> Option.bind (fun s ->
                    match Guid.TryParse(s) with
                    | true, g -> Some g
                    | _ -> None)
            match eidOpt, cidOpt with
            | Some eid, Some cid ->
                let seq =
                    match tryColumnString cols "seq" with
                    | Some s ->
                        match Int64.TryParse(
                                s,
                                NumberStyles.Integer,
                                CultureInfo.InvariantCulture) with
                        | true, n -> n
                        | _ -> -1L
                    | None -> -1L
                let kind = tryColumnString cols "kind" |> Option.defaultValue "unknown"
                let payload =
                    tryColumnString cols "payload" |> Option.defaultValue "null"
                let occurred =
                    match tryColumnString cols "occurred_at" with
                    | Some raw ->
                        match DateTime.TryParse(
                                raw,
                                CultureInfo.InvariantCulture,
                                DateTimeStyles.AssumeUniversal ||| DateTimeStyles.AdjustToUniversal) with
                        | true, dt -> dt
                        | _ -> DateTime.UtcNow
                    | None -> DateTime.UtcNow
                Some {
                    EventId    = eid
                    Seq        = seq
                    Kind       = kind
                    ConvId     = cid
                    Payload    = payload
                    OccurredAt = occurred
                    Source     = PgWalSrc
                }
            | _ ->
                logger.LogDebug(
                    "pg-wal: ignored row without event_id/conv_id: {Json}",
                    jsonLine)
                None
    with ex ->
        logger.LogWarning(
            ex,
            "pg-wal: failed to parse wal2json line: {Json}",
            jsonLine)
        None

let private pollOnce
        (logger: ILogger)
        (conn: NpgsqlConnection)
        (slotName: string)
        (subject: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (ct: CancellationToken)
        : Task<int> =
    task {
        use cmd =
            new NpgsqlCommand(
                "SELECT data \
                   FROM pg_logical_slot_get_changes(@slot, NULL, NULL, \
                                                    'add-tables', 'public.fsws_events', \
                                                    'format-version', '2')",
                conn)
        cmd.Parameters.AddWithValue("slot", slotName) |> ignore
        let mutable rows = 0
        use! reader = cmd.ExecuteReaderAsync(ct)
        while! reader.ReadAsync(ct) do
            let data = reader.GetString(0)
            match parseChangeRow logger data with
            | Some evt ->
                Interlocked.Increment(&counter.contents) |> ignore
                subject.OnNext(evt)
                rows <- rows + 1
            | None -> ()
        return rows
    }

let private pollLoop
        (logger: ILogger)
        (connectionString: string)
        (slotName: string)
        (pollInterval: TimeSpan)
        (subject: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (ct: CancellationToken)
        : Task =
    task {
        let mutable backoffMs = 500
        while not ct.IsCancellationRequested do
            try
                use conn = new NpgsqlConnection(connectionString)
                do! conn.OpenAsync(ct)
                logger.LogInformation(
                    "pg-wal: polling slot {Slot} every {Ms} ms",
                    slotName,
                    int pollInterval.TotalMilliseconds)
                backoffMs <- 500
                while not ct.IsCancellationRequested do
                    let! _rows = pollOnce logger conn slotName subject counter ct
                    do! Task.Delay(pollInterval, ct)
            with
            | :? OperationCanceledException -> ()
            | ex ->
                logger.LogWarning(
                    ex,
                    "pg-wal: poll cycle failed; retrying in {Ms} ms",
                    backoffMs)
                try
                    do! Task.Delay(backoffMs, ct)
                with :? OperationCanceledException -> ()
                let jitter = Random.Shared.Next(0, 250)
                backoffMs <- min 15000 (backoffMs * 2) + jitter
    }

let start
        (logger: ILogger)
        (connectionString: string)
        (slotName: string)
        (pollInterval: TimeSpan)
        : PgWalHandle =
    let subject = new Subject<UnifiedEvent>()
    let counter = ref 0L
    let cts = new CancellationTokenSource()
    let _task =
        pollLoop logger connectionString slotName pollInterval subject counter cts.Token
    {
        Events = subject :> IObservable<UnifiedEvent>
        Stop = fun () ->
            task {
                cts.Cancel()
                subject.OnCompleted()
                subject.Dispose()
            } :> Task
        DeliveredCount = fun () -> Volatile.Read(&counter.contents)
        SlotName = slotName
    }
