#[path = "../checkout_service.rs"]
mod checkout_service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    checkout_service::run().await
}
