mod config;
mod database;
mod docs;
mod server;
mod telemetry;

#[tokio::main]
async fn main() {
    server::run().await;
}
