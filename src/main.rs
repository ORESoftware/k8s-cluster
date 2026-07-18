//! 3FA web app — Supabase login + TOTP enrollment demo.
//!
//! MASH stack: maud (HTML) + axum (HTTP) + htmx (interactivity). SeaORM is the
//! house ORM, but this v1 is deliberately database-less — see readme.md.

#[tokio::main]
async fn main() {
    if let Err(error) = threefa_web_server::run().await {
        tracing::error!(error = %error, "3FA web server stopped with a fatal error");
        std::process::exit(1);
    }
}
