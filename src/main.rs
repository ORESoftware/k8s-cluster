//! Thin process entry point for the Sonus Auris backend.

#[tokio::main]
async fn main() {
    dd_sound_recorder_rs::run().await;
}
