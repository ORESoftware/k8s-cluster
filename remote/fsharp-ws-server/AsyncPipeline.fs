module OresSoftware.Dd.FsWs.AsyncPipeline

open System.Threading.Tasks
open OresSoftware.Dd.FsWs.PipelineStages

/// Native F# `task { }` implementation of the same five-stage pipeline.
///
/// No reactive runtime, no graph compiler — `parse` / `validate` run synchronously
/// on the caller thread, the two enrichment lookups are dispatched as plain
/// `Task.Run` workitems on the ThreadPool and awaited with `Task.WhenAll`, then
/// `score` / `serialize` run synchronously again. This is the .NET equivalent of
/// async.java's callback-style `Asyncc.Parallel` combinator — direct dispatch onto
/// the underlying executor, no per-message graph allocation.
let processFrame (inputFrame: string) : Task<string> =
    task {
        let parsed    = parse inputFrame
        let validated = validate parsed

        let aTask = Task.Run(fun () -> enrichLookupA validated)
        let bTask = Task.Run(fun () -> enrichLookupB validated)
        let! both = Task.WhenAll(aTask, bTask)

        let scored = score validated both.[0] both.[1]
        return serialize scored
    }
