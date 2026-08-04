# Documentation index

This directory documents the contracts, provider adapters, deployment boundaries, and operational procedures for `push-notification-server.rs`.

## Start here

- [Provider and app-store interoperability](provider-interop.md) — end-to-end Android/FCM, Apple/APNs, Expo, SendGrid, and Twilio setup, testing, receipts, and troubleshooting.
- [Contracts v1](contracts-v1.md) — canonical `PushJob` and `PushOutcome` wire contracts.
- [HTTP ingestion v1](http-ingestion-v1.md) — authenticated HTTP routes and request behavior.
- [NATS ingestion v1](nats-ingestion-v1.md) — durable push-job ingestion, Ack/NAK, retries, and dead letters.
- [Contact delivery v1](contact-delivery-v1.md) — separate email/SMS `ContactJob` and `ContactOutcome` contracts.

## Provider-specific references

- [APNs](apns.md)
- [Expo](expo.md)
- [Web Push](web-push.md)
- [SendGrid and Twilio audit](sendgrid-twilio-audit.md)
- [Provider canaries](provider-canaries.md)

## Architectural boundaries

- Google Play and Apple App Store distribute and sign mobile applications. They do not deliver notifications.
- FCM, APNs, Expo, and Web Push deliver push notifications.
- SendGrid delivers transactional email.
- Twilio delivers SMS.
- Supabase/Postgres stores identities, endpoints, preferences, jobs, attempts, receipts, suppressions, and outbox state. It is not a delivery provider.
- `shared-auth` and Supabase validate identity and authorization. Provider callbacks use provider-specific signature verification rather than user JWTs.
