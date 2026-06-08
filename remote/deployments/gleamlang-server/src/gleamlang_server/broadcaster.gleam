import gleam/erlang/process
import gleam/int
import gleam/list
import gleam/otp/actor

const dedupe_ttl_ms = 300_000

@external(erlang, "gleamlang_server_env", "json_message_id")
fn json_message_id(payload: String) -> Result(String, Nil)

@external(erlang, "gleamlang_server_env", "now_ms")
fn now_ms() -> Int

pub type StreamMessage {
  StreamJson(payload: String)
}

pub type MetricsSnapshot {
  MetricsSnapshot(
    subscribers: Int,
    ticks: Int,
    http_requests: Int,
    ws_messages: Int,
    nats_messages: Int,
  )
}

pub type Message {
  Subscribe(process.Subject(StreamMessage))
  Unsubscribe(process.Subject(StreamMessage))
  RecordHttpRequest
  RecordWsMessage
  BroadcastJson(payload: String)
  GetSnapshot(process.Subject(MetricsSnapshot))
  EmitTick
}

type SeenMessage {
  SeenMessage(id: String, expires_at_ms: Int)
}

type State {
  State(
    control_subject: process.Subject(Message),
    subscribers: List(process.Subject(StreamMessage)),
    seen_messages: List(SeenMessage),
    sequence: Int,
    http_requests: Int,
    ws_messages: Int,
    nats_messages: Int,
    interval_ms: Int,
  )
}

pub fn start(
  named_as name: process.Name(Message),
  interval_ms interval_ms: Int,
) -> actor.StartResult(process.Subject(Message)) {
  actor.new_with_initialiser(5000, fn(control_subject) {
    schedule_next_tick(control_subject, interval_ms)

    actor.initialised(State(
      control_subject: control_subject,
      subscribers: [],
      seen_messages: [],
      sequence: 0,
      http_requests: 0,
      ws_messages: 0,
      nats_messages: 0,
      interval_ms: interval_ms,
    ))
    |> actor.returning(control_subject)
    |> Ok
  })
  |> actor.named(name)
  |> actor.on_message(handle_message)
  |> actor.start
}

fn handle_message(
  state: State,
  message: Message,
) -> actor.Next(State, Message) {
  let State(
    control_subject: control_subject,
    subscribers: subscribers,
    seen_messages: seen_messages,
    sequence: sequence,
    http_requests: http_requests,
    ws_messages: ws_messages,
    nats_messages: nats_messages,
    interval_ms: interval_ms,
  ) = state

  case message {
    Subscribe(subscriber) -> {
      let deduped =
        list.filter(subscribers, fn(existing) { existing != subscriber })
      actor.continue(State(
        control_subject: control_subject,
        subscribers: [subscriber, ..deduped],
        seen_messages: seen_messages,
        sequence: sequence,
        http_requests: http_requests,
        ws_messages: ws_messages,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))
    }

    Unsubscribe(subscriber) ->
      actor.continue(State(
        control_subject: control_subject,
        subscribers: list.filter(subscribers, fn(existing) {
          existing != subscriber
        }),
        seen_messages: seen_messages,
        sequence: sequence,
        http_requests: http_requests,
        ws_messages: ws_messages,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))

    RecordHttpRequest ->
      actor.continue(State(
        control_subject: control_subject,
        subscribers: subscribers,
        seen_messages: seen_messages,
        sequence: sequence,
        http_requests: http_requests + 1,
        ws_messages: ws_messages,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))

    RecordWsMessage ->
      actor.continue(State(
        control_subject: control_subject,
        subscribers: subscribers,
        seen_messages: seen_messages,
        sequence: sequence,
        http_requests: http_requests,
        ws_messages: ws_messages + 1,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))

    BroadcastJson(payload) ->
      case should_broadcast(payload, seen_messages) {
        #(False, next_seen_messages) ->
          actor.continue(State(
            control_subject: control_subject,
            subscribers: subscribers,
            seen_messages: next_seen_messages,
            sequence: sequence,
            http_requests: http_requests,
            ws_messages: ws_messages,
            nats_messages: nats_messages,
            interval_ms: interval_ms,
          ))

        #(True, next_seen_messages) -> {
          list.each(subscribers, fn(subscriber) {
            process.send(subscriber, StreamJson(payload))
          })
          actor.continue(State(
            control_subject: control_subject,
            subscribers: subscribers,
            seen_messages: next_seen_messages,
            sequence: sequence,
            http_requests: http_requests,
            ws_messages: ws_messages,
            nats_messages: nats_messages + 1,
            interval_ms: interval_ms,
          ))
        }
      }

    GetSnapshot(reply_to) -> {
      process.send(
        reply_to,
        MetricsSnapshot(
          subscribers: list.length(subscribers),
          ticks: sequence,
          http_requests: http_requests,
          ws_messages: ws_messages,
          nats_messages: nats_messages,
        ),
      )
      actor.continue(State(
        control_subject: control_subject,
        subscribers: subscribers,
        seen_messages: seen_messages,
        sequence: sequence,
        http_requests: http_requests,
        ws_messages: ws_messages,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))
    }

    EmitTick -> {
      let next_sequence = sequence + 1
      let payload = tick_payload(next_sequence, list.length(subscribers))
      list.each(subscribers, fn(subscriber) {
        process.send(subscriber, StreamJson(payload))
      })
      schedule_next_tick(control_subject, interval_ms)
      actor.continue(State(
        control_subject: control_subject,
        subscribers: subscribers,
        seen_messages: seen_messages,
        sequence: next_sequence,
        http_requests: http_requests,
        ws_messages: ws_messages,
        nats_messages: nats_messages,
        interval_ms: interval_ms,
      ))
    }
  }
}

fn should_broadcast(
  payload: String,
  seen_messages: List(SeenMessage),
) -> #(Bool, List(SeenMessage)) {
  let now = now_ms()
  let active_seen =
    list.filter(seen_messages, fn(message) { message.expires_at_ms > now })

  case json_message_id(payload) {
    Ok(message_id) -> {
      case list.any(active_seen, fn(message) { message.id == message_id }) {
        True -> #(False, active_seen)
        False -> #(True, [
          SeenMessage(id: message_id, expires_at_ms: now + dedupe_ttl_ms),
          ..active_seen
        ])
      }
    }
    Error(_) -> #(True, active_seen)
  }
}

fn schedule_next_tick(
  control_subject: process.Subject(Message),
  interval_ms: Int,
) -> Nil {
  let _ = process.send_after(control_subject, interval_ms, EmitTick)
  Nil
}

fn tick_payload(sequence: Int, subscriber_count: Int) -> String {
  "{\"type\":\"tick\",\"sequence\":"
  <> int.to_string(sequence)
  <> ",\"subscribers\":"
  <> int.to_string(subscriber_count)
  <> "}"
}
