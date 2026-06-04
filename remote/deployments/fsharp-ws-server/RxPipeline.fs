module OresSoftware.Dd.FsWs.RxPipeline

open System
open System.Reactive.Linq
open System.Reactive.Threading.Tasks
open System.Threading.Tasks
open OresSoftware.Dd.FsWs.PipelineStages

/// Rx.NET implementation of the five-stage pipeline.
///
/// Each call to `process` materialises a tiny Observable graph for a single input
/// frame. That is deliberately the *worst-case* Rx pattern (the analogue of
/// `Source.single → runWith` in Akka Streams) — it pays the materialisation cost
/// per message rather than amortising it across a long-running observable. The
/// akka-ws-server readme has the long-form discussion of why; the short version
/// is that this is the only shape that keeps the function signature
/// `string -> Task<string>` so a WS handler can call it the same way it calls
/// the native-async pipeline.
///
/// For idiomatic long-running Rx usage (one Observable graph per WS connection,
/// `Subject.OnNext` per frame, `Observable.Throttle` / `Buffer` / etc.) materialise
/// the graph once at connect and feed it through the lifetime of the socket. That
/// pattern doesn't compose into a `string -> Task<string>` boundary, so it isn't
/// what this benchmark measures.
let processFrame (inputFrame: string) : Task<string> =
    Observable
        .Return(inputFrame)
        // parse / validate run synchronously on the caller thread — these are
        // microseconds and don't need to leave the originating WS worker.
        .Select(fun s -> parse s)
        .Select(fun n -> validate n)
        // enrich: fan out into two simulated downstream lookups, run both on
        // the default (thread-pool) scheduler, then zip the results back into
        // a tuple. This is the Rx equivalent of Akka Streams' `Broadcast →
        // Zip` and async.java's `Asyncc.Parallel`.
        .SelectMany(fun validated ->
            let a = Observable.Start(fun () -> enrichLookupA validated)
            let b = Observable.Start(fun () -> enrichLookupB validated)
            Observable
                .Zip(a, b, fun lookupA lookupB -> (validated, lookupA, lookupB)))
        .Select(fun (validated, lookupA, lookupB) -> score validated lookupA lookupB)
        .Select(fun scored -> serialize scored)
        // `ToTask` materialises the single-element observable into a Task so the
        // WS handler can await it. Materialisation happens here — the
        // observable graph above is just a plan until this call.
        .ToTask()
