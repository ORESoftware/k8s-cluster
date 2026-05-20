module OresSoftware.Dd.FsWs.PgOutbox

open System
open System.Reactive.Subjects
open System.Threading
open System.Threading.Tasks
open Microsoft.Extensions.Logging
open Npgsql
open OresSoftware.Dd.FsWs.PgSchema

/// Durable, app-controlled "I have caught up to seq N" polling. This is the
/// safety net underneath LISTEN/NOTIFY (which can miss while the connection
/// is recovering) and WAL CDC (which can miss if the slot was dropped or the
/// pod started fresh after a long downtime).
///
/// On each tick we pull every row from `fsws_events` strictly after our
/// in-memory `lastSeq` watermark. The first tick after boot pulls *nothing*
/// — we deliberately initialise `lastSeq` to the current MAX(seq) so we
/// don't replay history. If you do want a backfill, hit the `/v1/rx-stats`
/// reset endpoint (TBD) or restart with `FSWS_OUTBOX_BACKFILL=true`.
///
/// Why a separate hot Observable instead of merging in the same Subject as
/// LISTEN/NOTIFY: PresenceFanIn needs per-source counters and per-source
/// failure isolation. A NATS flake shouldn't kill the PG path; a WAL slot
/// drop shouldn't kill the outbox path.

type PgOutboxHandle = {
    Events: IObservable<UnifiedEvent>
    Stop:   unit -> Task
    DeliveredCount: unit -> int64
    LastSeq: unit -> int64
}

let private initialWatermark
        (logger: ILogger)
        (conn: NpgsqlConnection)
        (backfill: bool)
        (ct: CancellationToken)
        : Task<int64> =
    task {
        if backfill then
            logger.LogInformation(
                "pg-outbox: FSWS_OUTBOX_BACKFILL=true, starting from seq 0")
            return 0L
        else
            use cmd =
                new NpgsqlCommand("SELECT COALESCE(MAX(seq), 0) FROM fsws_events", conn)
            let! result = cmd.ExecuteScalarAsync(ct)
            let max =
                match result with
                | :? int64 as n -> n
                | :? int32 as n -> int64 n
                | _ -> 0L
            logger.LogInformation(
                "pg-outbox: watermark initialised to seq {Seq} (live-tail mode)",
                max)
            return max
    }

let private pollOnce
        (conn: NpgsqlConnection)
        (lastSeq: int64 ref)
        (subject: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (batchLimit: int)
        (ct: CancellationToken)
        : Task<int> =
    task {
        let watermark = Volatile.Read(&lastSeq.contents)
        use cmd =
            new NpgsqlCommand(
                "SELECT seq, event_id, kind, conv_id, payload::text, occurred_at \
                   FROM fsws_events \
                   WHERE seq > @after AND soft_deleted = false \
                   ORDER BY seq \
                   LIMIT @batch",
                conn)
        cmd.Parameters.AddWithValue("after", watermark) |> ignore
        cmd.Parameters.AddWithValue("batch", batchLimit) |> ignore
        let mutable rows = 0
        let mutable maxSeen = watermark
        use! reader = cmd.ExecuteReaderAsync(ct)
        while! reader.ReadAsync(ct) do
            let seq        = reader.GetInt64(0)
            let eid        = reader.GetGuid(1)
            let kind       = reader.GetString(2)
            let cid        = reader.GetGuid(3)
            let payload    = reader.GetString(4)
            let occurred   = reader.GetDateTime(5).ToUniversalTime()
            let evt = {
                EventId    = eid
                Seq        = seq
                Kind       = kind
                ConvId     = cid
                Payload    = payload
                OccurredAt = occurred
                Source     = PgOutboxSrc
            }
            Interlocked.Increment(&counter.contents) |> ignore
            subject.OnNext(evt)
            if seq > maxSeen then maxSeen <- seq
            rows <- rows + 1
        // Advance watermark only after the loop drains; if `OnNext` throws
        // mid-loop we'd otherwise drop replays.
        if maxSeen > watermark then
            Volatile.Write(&lastSeq.contents, maxSeen)
        return rows
    }

let private pollLoop
        (logger: ILogger)
        (connectionString: string)
        (pollInterval: TimeSpan)
        (batchLimit: int)
        (backfill: bool)
        (subject: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (lastSeq: int64 ref)
        (ct: CancellationToken)
        : Task =
    task {
        let mutable backoffMs = 500
        while not ct.IsCancellationRequested do
            try
                use conn = new NpgsqlConnection(connectionString)
                do! conn.OpenAsync(ct)
                let! initial = initialWatermark logger conn backfill ct
                Volatile.Write(&lastSeq.contents, initial)
                logger.LogInformation(
                    "pg-outbox: polling every {Ms} ms (batch={Batch})",
                    int pollInterval.TotalMilliseconds, batchLimit)
                backoffMs <- 500
                while not ct.IsCancellationRequested do
                    let! _rows = pollOnce conn lastSeq subject counter batchLimit ct
                    do! Task.Delay(pollInterval, ct)
            with
            | :? OperationCanceledException -> ()
            | ex ->
                logger.LogWarning(
                    ex,
                    "pg-outbox: poll failed; retrying in {Ms} ms",
                    backoffMs)
                try do! Task.Delay(backoffMs, ct)
                with :? OperationCanceledException -> ()
                let jitter = Random.Shared.Next(0, 250)
                backoffMs <- min 15000 (backoffMs * 2) + jitter
    }

let start
        (logger: ILogger)
        (connectionString: string)
        (pollInterval: TimeSpan)
        (batchLimit: int)
        (backfill: bool)
        : PgOutboxHandle =
    let subject = new Subject<UnifiedEvent>()
    let counter = ref 0L
    let lastSeq = ref 0L
    let cts = new CancellationTokenSource()
    let _task =
        pollLoop logger connectionString pollInterval batchLimit
                 backfill subject counter lastSeq cts.Token
    {
        Events = subject :> IObservable<UnifiedEvent>
        Stop = fun () ->
            task {
                cts.Cancel()
                subject.OnCompleted()
                subject.Dispose()
            } :> Task
        DeliveredCount = fun () -> Volatile.Read(&counter.contents)
        LastSeq = fun () -> Volatile.Read(&lastSeq.contents)
    }
