@external(erlang, "lambda_nats", "start")
fn start_erlang_nats() -> Nil

@external(erlang, "lambda_nats", "publish")
fn publish_erlang_nats(subject: String, payload: String) -> Result(Nil, Nil)

pub fn start() -> Nil {
  start_erlang_nats()
}

pub fn publish(subject: String, payload: String) -> Result(Nil, Nil) {
  publish_erlang_nats(subject, payload)
}
