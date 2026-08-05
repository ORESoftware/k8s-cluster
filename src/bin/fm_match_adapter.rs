//! JSON-lines adapter that replays the formal match lifecycle against production Rust state.

use serde::{Deserialize, Serialize};
use soccer_engine::soccer::lifecycle::{MatchAction, MatchProjection, SoccerRealtimeSession};
use std::error::Error;
use std::io::{self, BufRead, Write};

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    op: String,
    #[serde(default)]
    events: Vec<MatchAction>,
}

#[derive(Debug, Serialize)]
struct ReplayResponse {
    schema_version: u8,
    line: usize,
    #[serde(rename = "final")]
    final_state: MatchProjection,
    trace: Vec<MatchProjection>,
}

fn run() -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    for (offset, raw) in stdin.lock().lines().enumerate() {
        let line_number = offset + 1;
        let raw = raw?;
        if raw.trim().is_empty() {
            continue;
        }

        let request: ReplayRequest = serde_json::from_str(&raw)?;
        if request.op != "replay" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("line {line_number}: supported op is replay"),
            )
            .into());
        }

        let mut session = SoccerRealtimeSession::formal_fixture();
        let trace = session.replay(request.events);
        let final_state = trace
            .last()
            .copied()
            .expect("replay always includes the initial state");
        let response = ReplayResponse {
            schema_version: 1,
            line: line_number,
            final_state,
            trace,
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
    }

    writer.flush()?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fm_match_adapter: {error}");
        std::process::exit(2);
    }
}
