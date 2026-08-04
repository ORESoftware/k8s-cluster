#[path = "../executor_router_service.rs"]
mod executor_router_service;

#[tokio::main]
async fn main() {
    executor_router_service::run().await;
}
