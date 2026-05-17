module OresSoftware.Dd.FsWs.PipelineStages

open System
open System.Text.Json
open System.Text.Json.Nodes
open System.Threading

/// The actual stage logic, shared verbatim between the Rx.NET and native-async pipeline
/// implementations. Each stage is a plain F# function that either returns a value or raises.
///
/// The point of the comparison this module exists for is that *only* the orchestration
/// differs between the two implementations — the per-stage work is identical, byte for
/// byte, so any performance / debuggability difference observed downstream is
/// attributable to the coordination library, not to the work itself.
///
/// The five stages model a realistic-ish WebSocket request pipeline:
///
///   1. `parse`     — decode an incoming text frame as JSON.
///   2. `validate`  — schema-check; reject if `id` or `payload` are missing.
///   3. `lookupA` / `lookupB` — two fan-out simulated downstream lookups (each sleeps a
///      few ms to mimic an HTTP / DB hop).
///   4. `score`     — combine the parent record with both enrichments into a score.
///   5. `serialize` — encode back to JSON text.
///
/// `poison` is included so the stack-trace comparison reliably has a failure to exercise.

/// Simulated downstream-lookup latency, ms. Kept short so unit tests stay fast.
[<Literal>]
let LOOKUP_LATENCY_MIN_MS = 1

[<Literal>]
let LOOKUP_LATENCY_MAX_MS = 4

let private randomLatencyMs () =
    // Inclusive upper bound: [min, max]
    Random.Shared.Next(LOOKUP_LATENCY_MIN_MS, LOOKUP_LATENCY_MAX_MS + 1)

let private sleepMs (ms: int) =
    if ms > 0 then Thread.Sleep ms

let parse (input: string) : JsonNode =
    if isNull input then
        raise (ArgumentNullException(nameof input))
    match JsonNode.Parse(input) with
    | null -> raise (JsonException "parse: empty JSON input")
    | node -> node

let validate (parsed: JsonNode) : JsonNode =
    if isNull parsed then
        raise (ArgumentException "validate: parsed payload is null")
    let obj = parsed.AsObject()
    if not (obj.ContainsKey "id") then
        raise (ArgumentException "validate: missing required field `id`")
    if not (obj.ContainsKey "payload") then
        raise (ArgumentException "validate: missing required field `payload`")
    parsed

let enrichLookupA (validated: JsonNode) : string =
    sleepMs (randomLatencyMs ())
    let id = validated.["id"].GetValue<string>()
    // hashCode-equivalent — System.String.GetHashCode is randomised across runs in
    // .NET, so we roll our own deterministic 16-bit hash so the output is comparable
    // across the Java/F# implementations.
    let mutable h = 0
    for ch in id do h <- h * 31 + int ch
    sprintf "lookupA[%s]=%d" id (h &&& 0xffff)

let enrichLookupB (validated: JsonNode) : string =
    sleepMs (randomLatencyMs ())
    let id = validated.["id"].GetValue<string>()
    let payloadLen = validated.["payload"].ToJsonString().Length
    sprintf "lookupB[%s]=%d" id payloadLen

/// Deliberate exception thrown from inside `score` for the stack-trace comparison.
/// Kept as a separate function so the trace shows `PipelineStages.poison` at the
/// bottom, demonstrating that whatever the coordination library does on top, the
/// *cause* of the failure is always discoverable in the trace.
let poison (validated: JsonNode) : exn =
    let id = validated.["id"].GetValue<string>()
    InvalidOperationException(sprintf "score: deliberate poison-pill id=%s" id) :> exn

let score (validated: JsonNode) (lookupA: string) (lookupB: string) : JsonNode =
    let id = validated.["id"].GetValue<string>()
    if id = "poison" then
        raise (poison validated)
    let composite = lookupA.Length * 31 + lookupB.Length
    let out = JsonObject()
    out.["id"]      <- JsonValue.Create(id)
    out.["score"]   <- JsonValue.Create(composite)
    out.["lookupA"] <- JsonValue.Create(lookupA)
    out.["lookupB"] <- JsonValue.Create(lookupB)
    out :> JsonNode

let serialize (scored: JsonNode) : string =
    scored.ToJsonString()
