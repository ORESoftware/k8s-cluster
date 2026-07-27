# push-notification-server.rs

Dedicated Rust delivery service for:

- Firebase Cloud Messaging HTTP v1
- Apple Push Notification service
- Expo Push
- Web Push with VAPID

This repository is the source of truth. `ORESoftware/k8s-cluster` consumes a reviewed commit as the pinned git submodule `remote/deployments/push-notification-server.rs`.

## Status

The initial service skeleton exposes:

- `GET /healthz`
- `GET /readyz`
- graceful SIGINT/SIGTERM shutdown
- environment-driven `HOST` and `PORT` configuration

Provider extraction is tracked in Linear under DEN-261. The current embedded source implementation is `ORESoftware/k8s-cluster/remote/deployments/dd-email-sms-contact-rs`.

## Local development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

Defaults to `0.0.0.0:8121`.

## Security

Never commit FCM service-account JSON, APNs `.p8` keys, VAPID private keys, Expo access tokens, production device tokens, or Web Push capability URLs. Provider credentials must come from workload identity or a Kubernetes ExternalSecret-backed secret.
