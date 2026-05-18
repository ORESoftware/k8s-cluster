module OresSoftware.Dd.FsWs.RxAdvanced

open System
open System.Reactive.Concurrency
open System.Reactive.Linq
open System.Reactive.Subjects
open System.Threading
open OresSoftware.Dd.FsWs.PipelineStages

/// Long-running, hot, *per-process* Rx machinery. This module is everything
/// the boring per-message `/ws/rx` endpoint deliberately doesn't do — it
/// shows what Rx.NET actually buys you when you commit to its model instead
/// of fighting it.
///
/// What lives here:
///
///   - `RxStats`         : a process-wide hot observable of live counters
///                         (open WS connections, msgs/bytes in & out, uptime).
///                         Backed by `BehaviorSubject` + a 1 Hz ticker +
///                         a 120-element `ReplaySubject` rolling history,
///                         consumed by /v1/rx-stats, /v1/rx-stats/history,
///                         and /sse/rx-stats.
///
///   - `RxStreamFlow`    : per-WS-connection long-running pipeline. Subject
///                         on the left, IObservable on the right, the
///                         five-stage operator chain materialised *once*
///                         at connect — exactly the shape Akka Streams /
///                         async-java would call "right" and that the
///                         per-message `/ws/rx` endpoint deliberately fights.
///
///   - `RxWindowFlow`    : same input pipeline, but the output goes through
///                         `Buffer(200ms, 16)` so the reply is one batched
///                         frame per window. Demonstrates time-based
///                         batching — try writing that in plain `task { }`.
///
///   - `RxThrottleFlow`  : same input pipeline, but `Throttle(50ms)` on the
///                         output — flood the socket and you only get the
///                         last reply per 50 ms of silence. Classic
///                         keystroke-debounce shape.
///
///   - `RxSampleFlow`    : same input pipeline, but emits at most the latest
///                         completed result every 100 ms. This is the "live
///                         dashboard" shape: steady updates under load without
///                         letting every upstream event resize the browser UI.

// ----------------------------------------------------------------------------
// RxStats — process-wide live counters fed through an Rx hot pipeline.
// ----------------------------------------------------------------------------

[<CLIMutable>]
type StatsSnapshot = {
    openConnections: int
    messagesIn:  int64
    messagesOut: int64
    bytesIn:  int64
    bytesOut: int64
    uptimeMs: int64
    tickAtMs: int64
}

module RxStats =

    let mutable private openConns   = 0
    let mutable private msgInCount  = 0L
    let mutable private msgOutCount = 0L
    let mutable private bytesInTot  = 0L
    let mutable private bytesOutTot = 0L
    /// Monotonic — sidesteps any DateTime/wall-clock edge cases (the
    /// previous `DateTime.UtcNow` version surfaced an `uptimeMs: -1` at
    /// module-init time due to sub-millisecond rounding).
    let private uptimeWatch = System.Diagnostics.Stopwatch.StartNew()

    let connectionOpened () =
        Interlocked.Increment(&openConns) |> ignore

    let connectionClosed () =
        let after = Interlocked.Decrement(&openConns)
        if after < 0 then
            Interlocked.Exchange(&openConns, 0) |> ignore

    let messageIn (bytes: int) =
        Interlocked.Increment(&msgInCount) |> ignore
        Interlocked.Add(&bytesInTot, int64 bytes) |> ignore

    let messageOut (bytes: int) =
        Interlocked.Increment(&msgOutCount) |> ignore
        Interlocked.Add(&bytesOutTot, int64 bytes) |> ignore

    let snapshot () : StatsSnapshot =
        { openConnections = Volatile.Read(&openConns)
          messagesIn      = Volatile.Read(&msgInCount)
          messagesOut     = Volatile.Read(&msgOutCount)
          bytesIn         = Volatile.Read(&bytesInTot)
          bytesOut        = Volatile.Read(&bytesOutTot)
          uptimeMs        = uptimeWatch.ElapsedMilliseconds
          tickAtMs        = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() }

    /// Hot, shared, latest-cached observable of `StatsSnapshot`. New
    /// subscribers see the current value immediately (BehaviorSubject
    /// semantics), then receive a fresh snapshot once per second.
    let private liveSubject = new BehaviorSubject<StatsSnapshot>(snapshot ())

    let live : IObservable<StatsSnapshot> = liveSubject :> _

    /// 120-element rolling window. A late HTTP client hitting
    /// `/v1/rx-stats/history` synchronously gets the last ~120 seconds.
    let history = new ReplaySubject<StatsSnapshot>(120)

    /// Long-lived background pump driving both `live` and `history`. Bound
    /// to a module-private let so the subscription is rooted for the
    /// process lifetime; otherwise the GC would eventually reclaim it.
    let private _tickerSub : IDisposable =
        Observable
            .Interval(TimeSpan.FromSeconds(1.0), TaskPoolScheduler.Default)
            .Select(fun _ -> snapshot ())
            .Subscribe(fun snap ->
                liveSubject.OnNext(snap)
                history.OnNext(snap))


// ----------------------------------------------------------------------------
// The shared 5-stage Rx body used by every long-running pipeline below.
// Identical work to `RxPipeline.processFrame` and `AsyncPipeline.processFrame`
// — only the surrounding orchestration shape is what's varying.
//
// Per-message error handling: each input frame gets wrapped in its own inner
// observable subgraph, and `.Catch` converts any exception into a JSON error
// frame. This is critical for a long-running Subject — if any single message
// threw past the outer pipeline it would `OnError` the whole stream and tear
// the connection down. The Catch boundary keeps the connection alive across
// bad inputs the way the per-message `/ws/rx` endpoint does naturally.
// ----------------------------------------------------------------------------

let private escapeStr (s: string) : string =
    if isNull s then ""
    else s.Replace("\\", "\\\\").Replace("\"", "\\\"")

let private perMessageErrorFrame (ex: exn) : string =
    let cause = if isNull ex.InnerException then ex else ex.InnerException
    sprintf "{\"ok\":false,\"error\":\"%s: %s\"}"
        (cause.GetType().Name)
        (escapeStr cause.Message)

let private rxFiveStages (inbound: IObservable<string>) : IObservable<string> =
    inbound.SelectMany(fun input ->
        // Per-input inner subgraph. `Observable.Return(input)` is a cold
        // observable that emits once and completes; the rest of the chain
        // hangs off of it. SelectMany subscribes to this inner observable
        // per outer emission, so every frame gets its own enrichment fan-out.
        let body : IObservable<string> =
            Observable
                .Return(input)
                // parse / validate are cheap and synchronous; keep them on
                // whatever scheduler the inbound OnNext arrived on. No
                // reason to bounce to the thread pool just to allocate a
                // JsonNode.
                .Select(fun s -> parse s)
                .Select(fun n -> validate n)
                // enrich: fan-out → fan-in. Each lookup is ~1-4 ms of
                // simulated I/O so we schedule both on the
                // TaskPoolScheduler. This is the .NET equivalent of Akka
                // Streams' `mapAsync(2)` or async.java's `Asyncc.Parallel`.
                .SelectMany(fun validated ->
                    let a =
                        Observable.Start(
                            (fun () -> enrichLookupA validated),
                            TaskPoolScheduler.Default)
                    let b =
                        Observable.Start(
                            (fun () -> enrichLookupB validated),
                            TaskPoolScheduler.Default)
                    Observable.Zip(
                        a, b,
                        fun lookupA lookupB ->
                            struct (validated, lookupA, lookupB)))
                .Select(fun (struct (validated, lookupA, lookupB)) ->
                    score validated lookupA lookupB)
                .Select(fun scored -> serialize scored)
                .Select(fun out -> sprintf "{\"ok\":true,\"result\":%s}" out)
        body.Catch(fun (ex: exn) ->
            Observable.Return(perMessageErrorFrame ex)))


// ----------------------------------------------------------------------------
// Three concrete long-running pipelines that differ only in the *output side*.
// ----------------------------------------------------------------------------

module RxStreamFlow =

    /// Plain long-running shape: one reply per input frame, but the operator
    /// chain is built once at connect rather than once per message. This is
    /// the apples-to-apples *correct* Rx usage — the per-message `/ws/rx`
    /// endpoint deliberately picks the worst-case `Observable.Return →
    /// runWith` pattern for benchmark-comparison parity with Akka Streams.
    let pipeline : IObservable<string> -> IObservable<string> =
        rxFiveStages


module RxWindowFlow =

    /// `Buffer(timespan, count)` — emit a list whenever 200 ms has passed
    /// since the last emit OR 16 items have piled up, whichever fires first.
    /// The result of a 100 msg/s flood is one batched frame every 200 ms
    /// containing the last 16-ish results.
    ///
    /// In plain `task { }` you'd need an out-of-band timer + an
    /// `IDisposable.Reset()` + a lock + a queue. In Rx it's one operator.
    let pipeline (inbound: IObservable<string>) : IObservable<string> =
        (rxFiveStages inbound)
            .Buffer(TimeSpan.FromMilliseconds(200.0), 16)
            .Where(fun batch -> batch.Count > 0)
            .Select(fun batch ->
                let items = String.concat "," batch
                sprintf "{\"window\":\"200ms|16\",\"count\":%d,\"items\":[%s]}"
                    batch.Count items)


module RxThrottleFlow =

    /// `Throttle(timespan)` — emit the *last* value only after the source
    /// has been quiet for the specified duration. Classic keystroke-debounce
    /// shape: if a client floods at 100 msg/s the throttle never fires; once
    /// they pause for 50 ms it emits the most recent reply.
    ///
    /// (For "at most once per N ms regardless of activity" use `Sample(N)`
    /// instead. We deliberately picked `Throttle` here because debounce is
    /// the more interesting Rx-native shape for WS demos.)
    let pipeline (inbound: IObservable<string>) : IObservable<string> =
        (rxFiveStages inbound)
            .Throttle(TimeSpan.FromMilliseconds(50.0))


module RxSampleFlow =

    /// `Sample(timespan)` — regardless of how busy the inbound stream is,
    /// emit at most the latest completed result once per tick. This is the
    /// Rx-native "operator UI telemetry" shape: the client can flood the
    /// socket, but a dashboard or terminal pane only repaints at 10 Hz.
    let pipeline (inbound: IObservable<string>) : IObservable<string> =
        (rxFiveStages inbound)
            .Sample(TimeSpan.FromMilliseconds(100.0))
            .Select(fun frame ->
                sprintf "{\"sample\":\"100ms\",\"item\":%s}" frame)


// ----------------------------------------------------------------------------
// JSON serializer for StatsSnapshot. Hand-rolled because System.Text.Json
// source-gen would require either reflection (slow + AOT-hostile) or a
// JsonSerializerContext (extra ceremony for a 7-field record).
// ----------------------------------------------------------------------------

let serializeSnapshot (s: StatsSnapshot) : string =
    System.String.Format(
        System.Globalization.CultureInfo.InvariantCulture,
        "{{\"openConnections\":{0},\"messagesIn\":{1},\"messagesOut\":{2},\"bytesIn\":{3},\"bytesOut\":{4},\"uptimeMs\":{5},\"tickAtMs\":{6}}}",
        s.openConnections, s.messagesIn, s.messagesOut,
        s.bytesIn, s.bytesOut, s.uptimeMs, s.tickAtMs)
