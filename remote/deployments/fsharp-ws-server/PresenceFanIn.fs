module OresSoftware.Dd.FsWs.PresenceFanIn

open System
open System.Collections.Concurrent
open System.Reactive.Concurrency
open System.Reactive.Linq
open System.Reactive.Subjects
open System.Threading
open Microsoft.Extensions.Logging
open OresSoftware.Dd.FsWs.PgSchema

/// Fan-in graph: PgListen + PgWal + PgOutbox + NatsRx + a local
/// "write-path injection" subject merge into one hot
/// `IObservable<UnifiedEvent>`.
///
/// Why an Rx graph rather than four `Subscribe` blocks each writing to a
/// per-WS-connection queue:
///
///   * Single dedup point. The same logical event lands in our process up
///     to four times (LISTEN/NOTIFY ~ms, WAL ~poll interval, outbox ~poll
///     interval, NATS ~ms). The dedup cache here, applied *before* the
///     fan-out, ensures every WS subscriber sees each event exactly once.
///
///   * Single GroupBy. Most subscribers only care about a subset of conv
///     ids. Grouping the merged stream by `conv_id` and then throttling
///     each group independently means a high-fan-out conv doesn't starve
///     low-fan-out ones, and the operator chain is materialised once per
///     process rather than per WS subscriber.
///
///   * Single hot observable via `Publish().RefCount()`. Every WS
///     subscriber attaches and detaches as it wants; the upstream merge
///     stays alive as long as ≥1 subscriber is connected.
///
/// Dedup posture:
///
///   * Key: `event_id`. The four sources all populate it from
///     `fsws_events.event_id`, and `fsws_publish_event` enforces uniqueness
///     in the DB. Two events with the same id are by definition the same
///     logical event, just delivered by different paths.
///   * Cache: bounded (default 8192 entries) + per-entry TTL (default
///     60 s). The lookup is O(1) via `ConcurrentDictionary`; eviction is
///     a periodic sweep on the same tick the SSE stats feed uses.
///   * First-source-wins: the source label of whichever delivery hit the
///     dedup cache first is what propagates downstream. That's what
///     `/v1/rx-stats/sources` counts when broken out by source.

type FanInOptions = {
    /// Max entries in the dedup cache; once exceeded, oldest are evicted
    /// on the next sweep. Tune higher if you have very bursty traffic.
    DedupCapacity:   int
    /// How long an event id stays in the cache. Should be > max
    /// expected re-delivery skew between the four sources (typically a
    /// few seconds; we default to 60).
    DedupTtl:        TimeSpan
    /// Throttle window applied *per conv_id group*. Within a hot group,
    /// only the most-recent event in the window is emitted. Set to
    /// `TimeSpan.Zero` to disable throttling.
    PerGroupThrottle: TimeSpan
    /// How often to sweep the dedup cache.
    SweepInterval:   TimeSpan
}

let defaultOptions = {
    DedupCapacity    = 8192
    DedupTtl         = TimeSpan.FromSeconds(60.0)
    PerGroupThrottle = TimeSpan.FromMilliseconds(25.0)
    SweepInterval    = TimeSpan.FromSeconds(5.0)
}

type FanInHandle = {
    /// The hot, deduped, throttled, grouped-flat output. Subscribe and
    /// filter by `ConvId` to get a per-client feed.
    Events:              IObservable<UnifiedEvent>
    /// Inject an event into the fan-in graph from a local code path
    /// (today: /ws/rx-publish after the DB write succeeds). Same dedup
    /// semantics as remote sources — duplicates are silently dropped.
    Inject:              UnifiedEvent -> unit
    DedupHitCount:       unit -> int64
    DedupMissCount:      unit -> int64
    DedupCurrentSize:    unit -> int
    Stop:                unit -> unit
}

/// Internal dedup record — small enough that we keep it inline in the
/// dictionary value rather than separating into a struct.
type private DedupEntry = {
    InsertedAt: DateTime
    Source:     EventSource
}

let start
        (logger: ILogger)
        (options: FanInOptions)
        (pgNotify: IObservable<UnifiedEvent> option)
        (pgWal:    IObservable<UnifiedEvent> option)
        (pgOutbox: IObservable<UnifiedEvent> option)
        (nats:     IObservable<UnifiedEvent> option)
        : FanInHandle =
    let cache = ConcurrentDictionary<Guid, DedupEntry>()
    let mutable hitCount = 0L
    let mutable missCount = 0L

    let injection = new Subject<UnifiedEvent>()

    // Walk the cache, drop expired entries. Cheap enough at default
    // capacity that we can do it on every sweep tick without batching.
    let sweep () =
        let now = DateTime.UtcNow
        let mutable evicted = 0
        for kv in cache do
            if (now - kv.Value.InsertedAt) > options.DedupTtl then
                let ok, _ = cache.TryRemove(kv.Key)
                if ok then evicted <- evicted + 1
        // Hard cap: if we somehow exceeded capacity (shouldn't happen
        // unless traffic far exceeds TTL window), evict by oldest first.
        if cache.Count > options.DedupCapacity then
            cache
            |> Seq.sortBy (fun kv -> kv.Value.InsertedAt)
            |> Seq.truncate (cache.Count - options.DedupCapacity)
            |> Seq.iter (fun kv ->
                cache.TryRemove(kv.Key) |> ignore
                evicted <- evicted + 1)
        if evicted > 0 then
            logger.LogDebug("fan-in: dedup sweep evicted {N} entries", evicted)

    let sweepTimer =
        Observable
            .Interval(options.SweepInterval, Scheduler.Default)
            .Subscribe(fun _ -> sweep ())

    let isFirstDelivery (evt: UnifiedEvent) : bool =
        let entry = { InsertedAt = DateTime.UtcNow; Source = evt.Source }
        let added = cache.TryAdd(evt.EventId, entry)
        if added then
            Interlocked.Increment(&missCount) |> ignore
            true
        else
            Interlocked.Increment(&hitCount) |> ignore
            false

    let sourceOrEmpty (opt: IObservable<UnifiedEvent> option) : IObservable<UnifiedEvent> =
        match opt with
        | Some s -> s
        | None   -> Observable.Empty<UnifiedEvent>()

    // The merge happens before the dedup filter so the cache "first wins"
    // applies across all four origins. Per-group throttling lives *after*
    // dedup so that throttling doesn't drop the only delivery of an event
    // before its duplicates arrive.
    let merged : IObservable<UnifiedEvent> =
        Observable.Merge(
            [|
                sourceOrEmpty pgNotify
                sourceOrEmpty pgWal
                sourceOrEmpty pgOutbox
                sourceOrEmpty nats
                injection :> IObservable<UnifiedEvent>
            |])

    let deduped : IObservable<UnifiedEvent> =
        merged.Where(fun evt -> isFirstDelivery evt)

    let perGroupShaped : IObservable<UnifiedEvent> =
        if options.PerGroupThrottle = TimeSpan.Zero then
            deduped
        else
            deduped
                .GroupBy(fun evt -> evt.ConvId)
                .SelectMany(fun (group: IGroupedObservable<Guid, UnifiedEvent>) ->
                    group.Throttle(options.PerGroupThrottle))

    // Publish + RefCount: ONE merge subscription per process, fanned out to
    // every WS subscriber. When the last subscriber drops the upstream is
    // disposed; when a new one connects it's re-attached.
    let connectable = perGroupShaped.Publish()
    let hot = connectable.RefCount()

    {
        Events = hot
        Inject = fun evt -> injection.OnNext(evt)
        DedupHitCount  = fun () -> Volatile.Read(&hitCount)
        DedupMissCount = fun () -> Volatile.Read(&missCount)
        DedupCurrentSize = fun () -> cache.Count
        Stop = fun () ->
            sweepTimer.Dispose()
            injection.OnCompleted()
            injection.Dispose()
    }
