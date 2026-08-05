#[tokio::main]
async fn main() {
    dd_meshy_client::cli::exit_on_error(dd_meshy_client::cli::run_from_env().await);
}
