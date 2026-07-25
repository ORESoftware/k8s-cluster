//! 3FA zero-knowledge sync server entrypoint.

#[tokio::main]
async fn main() {
    if let Err(error) = threefa_backend::run().await {
        tracing::error!(error = %error, "3FA sync server stopped with a fatal error");
        std::process::exit(1);
    }
}
