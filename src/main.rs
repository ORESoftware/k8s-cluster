//! 3FA zero-knowledge sync server entrypoint.

#[tokio::main]
async fn main() {
    threefa_backend::apply_cli_flags().unwrap_or_else(|error| {
        eprintln!("invalid command-line configuration: {error}");
        std::process::exit(2);
    });
    if let Err(error) = threefa_backend::run().await {
        tracing::error!(error = %error, "3FA sync server stopped with a fatal error");
        std::process::exit(1);
    }
}
