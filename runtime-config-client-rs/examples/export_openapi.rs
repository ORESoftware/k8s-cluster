use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (_, openapi) = dd_runtime_config_client::router_and_openapi();
    let mut json = serde_json::to_string_pretty(&openapi)?;
    json.push('\n');
    io::stdout().write_all(json.as_bytes())?;
    Ok(())
}
