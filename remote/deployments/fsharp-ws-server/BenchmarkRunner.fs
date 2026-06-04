module OresSoftware.Dd.FsWs.BenchmarkRunner

open System
open System.Diagnostics
open System.Text
open System.Threading.Tasks

/// Runs both pipelines `iterations` times against the same payload and emits a JSON
/// timing summary. Matches the shape of `dd-akka-ws-server`'s `/v1/benchmark` so
/// the two services can be diffed with the same loader / dashboards.
///
/// The benchmark drives each pipeline sequentially from a single caller. That is
/// the *micro-benchmark* view: per-call overhead, not under-load behaviour. For
/// the real-load view, point a WS loadtest at `/ws/rx` and `/ws/async`.

let private percentile (sorted: int64[]) (p: float) : int64 =
    if sorted.Length = 0 then 0L
    else
        let idx = int (Math.Ceiling(p * float sorted.Length)) - 1
        let clamped = max 0 (min (sorted.Length - 1) idx)
        sorted.[clamped]

let private warmupIters (iters: int) : int =
    min 20 (max 0 (iters / 10))

let private runSync (label: string)
                    (pipeline: string -> Task<string>)
                    (payload: string)
                    (iterations: int) : Task<string> =
    task {
        let warmup = warmupIters iterations
        for _ in 1 .. warmup do
            let! _ = pipeline payload
            ()

        let samples = Array.zeroCreate<int64> iterations
        let wall = Stopwatch.StartNew()
        for i in 0 .. iterations - 1 do
            let sw = Stopwatch.StartNew()
            let! _ = pipeline payload
            sw.Stop()
            samples.[i] <- sw.ElapsedTicks
        wall.Stop()

        let tickToUs (t: int64) =
            (t * 1_000_000L) / Stopwatch.Frequency

        let asUs = Array.map tickToUs samples
        Array.sortInPlace asUs

        let p50 = percentile asUs 0.50
        let p95 = percentile asUs 0.95
        let p99 = percentile asUs 0.99
        let mx  = asUs.[asUs.Length - 1]
        let wallMs = wall.ElapsedMilliseconds
        let throughput =
            if wallMs > 0L then
                float iterations * 1000.0 / float wallMs
            else 0.0

        let sb = StringBuilder()
        sb.Append("\"")  .Append(label).Append("\":{")
          .Append("\"iterations\":").Append(iterations).Append(",")
          .Append("\"p50_us\":")     .Append(p50)       .Append(",")
          .Append("\"p95_us\":")     .Append(p95)       .Append(",")
          .Append("\"p99_us\":")     .Append(p99)       .Append(",")
          .Append("\"max_us\":")     .Append(mx)        .Append(",")
          .Append("\"wall_ms\":")    .Append(wallMs)    .Append(",")
          .Append("\"throughput_per_s\":")
          .Append(throughput.ToString("F2", System.Globalization.CultureInfo.InvariantCulture))
          .Append("}")
        |> ignore
        return sb.ToString()
    }

let runAsync (iterations: int) (payload: string) : Task<string> =
    task {
        let! rx    = runSync "rx"    OresSoftware.Dd.FsWs.RxPipeline.processFrame    payload iterations
        let! asy   = runSync "async" OresSoftware.Dd.FsWs.AsyncPipeline.processFrame payload iterations
        return sprintf "{%s,%s}" rx asy
    }
