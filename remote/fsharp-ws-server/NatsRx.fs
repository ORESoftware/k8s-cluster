module OresSoftware.Dd.FsWs.NatsRx

open System
open System.Collections.Generic
open System.Globalization
open System.Reactive.Linq
open System.Reactive.Subjects
open System.Text
open System.Text.Json.Nodes
open System.Threading
open System.Threading.Tasks
open Microsoft.Extensions.Logging
open NATS.Client.Core
open OresSoftware.Dd.FsWs.PgSchema

/// NATS transport, lifted into Rx.
///
/// The new `NATS.Client.Core` (v2.x of the rewritten NATS.Net) hands every
/// subscription back as an `IAsyncEnumerable<NatsMsg<T>>`. We iterate that
/// asynchronously and pump every message through a `Subject<UnifiedEvent>`
/// so the fan-in graph in PresenceFanIn can compose it with PgListen /
/// PgWal / PgOutbox using the same operators.
///
/// Subject layout:
///
///   fsws.events.published — every event the F# server publishes (whether
///                            originated locally via /ws/rx-publish or
///                            forwarded from the DB write-path) lands here.
///                            Other consumers (the Gleam server, a future
///                            audit-log sink, etc.) can subscribe.
///   fsws.events.echo.*    — used by /ws/rx-nats-echo to demonstrate a NATS
///                            round-trip: WS frame in → publish → re-receive
///                            via subscription → WS frame out.

let publishedSubject = "fsws.events.published"
let echoSubjectPrefix = "fsws.events.echo."

type NatsHandle = {
    Events:        IObservable<UnifiedEvent>
    PublishEvent:  UnifiedEvent -> Task
    PublishRaw:    string -> string -> Task
    SubscribeRaw:  string -> IObservable<NatsMsg<byte array>>
    Stop:          unit -> Task
    DeliveredCount: unit -> int64
    PublishedCount: unit -> int64
}

// JsonNode's indexer throws InvalidOperationException when the underlying
// node isn't a JsonObject (e.g. a published JSON string or array). Guard
// every access so a malformed inbound NATS frame just bails out cleanly
// instead of cascading a stack trace into the subscription loop.
let private tryGuid (root: JsonNode) (key: string) : Guid option =
    match root with
    | :? JsonObject as obj ->
        match obj.[key] with
        | null -> None
        | v ->
            match Guid.TryParse(v.ToString()) with
            | true, g -> Some g
            | _ -> None
    | _ -> None

let private tryString (root: JsonNode) (key: string) (fallback: string) : string =
    match root with
    | :? JsonObject as obj ->
        match obj.[key] with
        | null -> fallback
        | v ->
            let s = v.ToString()
            if isNull s then fallback else s
    | _ -> fallback

let private tryInt64 (root: JsonNode) (key: string) (fallback: int64) : int64 =
    match root with
    | :? JsonObject as obj ->
        match obj.[key] with
        | null -> fallback
        | v ->
            match Int64.TryParse(
                    v.ToString(),
                    NumberStyles.Integer,
                    CultureInfo.InvariantCulture) with
            | true, n -> n
            | _ -> fallback
    | _ -> fallback

let private tryDateTime (root: JsonNode) (key: string) (fallback: DateTime) : DateTime =
    match root with
    | :? JsonObject as obj ->
        match obj.[key] with
        | null -> fallback
        | v ->
            match DateTime.TryParse(
                    v.ToString(),
                    CultureInfo.InvariantCulture,
                    DateTimeStyles.AssumeUniversal ||| DateTimeStyles.AdjustToUniversal) with
            | true, dt -> dt
            | _ -> fallback
    | _ -> fallback

let private msgToUnified
        (logger: ILogger)
        (msg: NatsMsg<byte array>)
        : UnifiedEvent option =
    try
        let body =
            match msg.Data with
            | null -> ""
            | b -> Encoding.UTF8.GetString(b)
        if String.IsNullOrWhiteSpace(body) then None
        else
            let root = JsonNode.Parse(body)
            if isNull root then None
            else
                match tryGuid root "event_id", tryGuid root "conv_id" with
                | Some eid, Some cid ->
                    Some {
                        EventId    = eid
                        Seq        = tryInt64 root "seq" -1L
                        Kind       = tryString root "kind" "unknown"
                        ConvId     = cid
                        Payload    = body
                        OccurredAt = tryDateTime root "occurred_at" DateTime.UtcNow
                        Source     = NatsSrc
                    }
                | _ ->
                    logger.LogDebug(
                        "nats-rx: ignored message without event_id/conv_id on {Subject}: {Body}",
                        msg.Subject, body)
                    None
    with ex ->
        logger.LogWarning(
            ex,
            "nats-rx: failed to parse message on {Subject}",
            msg.Subject)
        None

/// Manual async iteration over `IAsyncEnumerable<'T>` — F# 8's built-in
/// task CE doesn't have a `For` overload for `IAsyncEnumerable`, and we
/// don't want to pull `FSharp.Control.TaskSeq` just for this one loop.
let private forEachAsync<'T>
        (source: IAsyncEnumerable<'T>)
        (body: 'T -> Task)
        (ct: CancellationToken)
        : Task =
    task {
        use enumerator = source.GetAsyncEnumerator(ct)
        let mutable cont = true
        while cont && not ct.IsCancellationRequested do
            let! moveNext = enumerator.MoveNextAsync().AsTask()
            if moveNext then
                do! body enumerator.Current
            else
                cont <- false
    }

let private consumeLoop
        (logger: ILogger)
        (conn: NatsConnection)
        (subj: string)
        (target: Subject<UnifiedEvent>)
        (counter: int64 ref)
        (ct: CancellationToken)
        : Task =
    task {
        try
            let asyncEnum =
                conn.SubscribeAsync<byte array>(
                    subject = subj,
                    cancellationToken = ct)
            do! forEachAsync
                    asyncEnum
                    (fun msg ->
                        match msgToUnified logger msg with
                        | Some evt ->
                            Interlocked.Increment(&counter.contents) |> ignore
                            target.OnNext(evt)
                            Task.CompletedTask
                        | None ->
                            Task.CompletedTask)
                    ct
        with
        | :? OperationCanceledException -> ()
        | ex ->
            logger.LogWarning(
                ex,
                "nats-rx: subscription loop exited on {Subject}",
                subj)
    }

let start
        (logger: ILogger)
        (natsUrl: string)
        : Task<NatsHandle> =
    task {
        // NatsOpts in NATS.Client.Core 2.x is a C# record with init-only
        // properties. F# can use object-initialiser syntax to set them
        // without resorting to `with` cloning each field.
        let opts =
            NatsOpts(
                Url = natsUrl,
                Name = "dd-fsharp-ws-server")
        let conn = new NatsConnection(opts)
        do! conn.ConnectAsync()
        logger.LogInformation("nats-rx: connected to {Url}", natsUrl)

        let target = new Subject<UnifiedEvent>()
        let deliveredCounter = ref 0L
        let publishedCounter = ref 0L
        let cts = new CancellationTokenSource()

        let _consumer =
            consumeLoop logger conn publishedSubject target deliveredCounter cts.Token

        let subscribeRaw (subj: string) : IObservable<NatsMsg<byte array>> =
            Observable.Create<NatsMsg<byte array>>(fun (obs: IObserver<NatsMsg<byte array>>) ->
                let inner = new CancellationTokenSource()
                let _t =
                    task {
                        try
                            let asyncEnum =
                                conn.SubscribeAsync<byte array>(
                                    subject = subj,
                                    cancellationToken = inner.Token)
                            do! forEachAsync
                                    asyncEnum
                                    (fun msg ->
                                        obs.OnNext(msg)
                                        Task.CompletedTask)
                                    inner.Token
                            obs.OnCompleted()
                        with
                        | :? OperationCanceledException ->
                            obs.OnCompleted()
                        | ex ->
                            obs.OnError(ex)
                    }
                { new IDisposable with
                    member _.Dispose() =
                        inner.Cancel()
                        inner.Dispose() })

        // Envelope shape mirrors what WsRoutes.serializeUnifiedEvent emits to
        // WS clients — top-level event_id/seq/kind/conv_id/occurred_at fields,
        // with the original JSON payload spliced in verbatim. Receivers
        // (including our own subscription) can rebuild a UnifiedEvent without
        // needing access to a DB row.
        let serializeEnvelope (evt: UnifiedEvent) : string =
            let payload =
                if String.IsNullOrEmpty(evt.Payload) then "null" else evt.Payload
            System.String.Format(
                CultureInfo.InvariantCulture,
                "{{\"event_id\":\"{0}\",\"seq\":{1},\"kind\":\"{2}\",\"conv_id\":\"{3}\",\"occurred_at\":\"{4:O}\",\"payload\":{5}}}",
                evt.EventId,
                evt.Seq,
                evt.Kind.Replace("\\", "\\\\").Replace("\"", "\\\""),
                evt.ConvId,
                evt.OccurredAt.ToUniversalTime(),
                payload)

        return {
            Events = target :> IObservable<UnifiedEvent>
            PublishEvent = fun (evt: UnifiedEvent) ->
                task {
                    let envelope = serializeEnvelope evt
                    let bytes = Encoding.UTF8.GetBytes(envelope: string)
                    do! conn.PublishAsync<byte array>(
                            subject = publishedSubject,
                            data = bytes)
                    Interlocked.Increment(&publishedCounter.contents) |> ignore
                } :> Task
            PublishRaw = fun (subj: string) (body: string) ->
                task {
                    let bytes = Encoding.UTF8.GetBytes(body: string)
                    do! conn.PublishAsync<byte array>(
                            subject = subj,
                            data = bytes)
                    Interlocked.Increment(&publishedCounter.contents) |> ignore
                } :> Task
            SubscribeRaw = subscribeRaw
            Stop = fun () ->
                task {
                    cts.Cancel()
                    target.OnCompleted()
                    target.Dispose()
                    do! conn.DisposeAsync()
                } :> Task
            DeliveredCount = fun () -> Volatile.Read(&deliveredCounter.contents)
            PublishedCount = fun () -> Volatile.Read(&publishedCounter.contents)
        }
    }
