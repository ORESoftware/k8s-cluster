# Cliptown

Welcome to the Cliptown organization!

Cliptown is dedicated to improving the clipboard experience by remembering old clipboard items, allowing for powerful search capabilities, syncing across devices (macOS, iOS, Android, Windows, Linux, and Web), and enabling long-term pinning of items.

## Architecture

This organization uses several repositories to manage the project:

- [cliptown-monorepo](https://github.com/cliptown/cliptown-monorepo): General documentation and overarching project tracking.
- [cliptown-rust-backend.rs](https://github.com/cliptown/cliptown-rust-backend.rs): The Rust API server (SeaORM, Postgres).
- [cliptown-flutter](https://github.com/cliptown/cliptown-flutter): The Flutter apps for Desktop and Mobile.
- [cliptown-clients](https://github.com/cliptown/cliptown-clients): SDKs for Dart, Rust, and TypeScript.
- [cliptown-interfaces](https://github.com/cliptown/cliptown-interfaces): Shared interface definitions (Proto/OpenAPI).
- [cliptown-infra](https://github.com/cliptown/cliptown-infra): Kubernetes infrastructure manifests (App of Apps).

## Authentication

Authentication is handled via Supabase. We utilize a 6-digit PIN as the primary authentication method, with biometric fallbacks (thumbprint, voice recognition). Sessions expire every 10 days by default (configurable up to 20 days).
