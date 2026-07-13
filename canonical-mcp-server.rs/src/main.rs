//! canonical-mcp: an MCP server for operating the canonical.cloud stack.
//!
//! Serves over stdio. `main` is bootstrap only; the tools live in
//! [`server`] and [`tools`].

mod server;
mod tools;

use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = server::CanonicalMcp::new()?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
