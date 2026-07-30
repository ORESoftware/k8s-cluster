# SendGrid and Twilio audit

## Scope

This audit compared the standalone service with the legacy implementation in `k8s-cluster/remote/deployments/dd-email-sms-contact-rs` and hardened the reusable behavior without restoring the old mixed push implementation.

## Legacy findings

The legacy SendGrid/Twilio paths provided a useful proof of integration, but they were not sufficient as the canonical delivery boundary:

1. Every HTTP 2xx response was reported as a generic success, with no explicit distinction between provider acceptance and final delivery.
2. Failed upstream response bodies were returned to callers after only length truncation; provider messages can contain recipient data or other sensitive request fragments.
3. Email and SMS requests had no shared versioned contract, normalized outcome classes, or non-reversible recipient fingerprint.
4. SendGrid supported only direct HTML/text messages and did not model dynamic templates, EU regional API selection, or sandbox validation.
5. Twilio supported only Account SID/Auth Token plus a fixed `From` number; it did not support API Key rotation, Messaging Services, status callbacks, or message validity periods.
6. Provider configuration was not isolated from the push contract, creating a risk that future fallbacks would widen the push API.
7. Immediate API responses were not paired with a signature-verification plan for SendGrid Event Webhook and Twilio status callbacks.

## Hardened implementation

The standalone service now adds optional contact lanes with these controls:

- separate versioned `ContactJob` and `ContactOutcome` contracts
- fail-closed authenticated single and batch HTTP routes
- strict recipient/content/provider matching
- server-controlled verified SendGrid sender and Twilio sender identity
- recipient fingerprints instead of raw addresses/numbers in outcomes
- bounded provider error codes and safe details; no raw upstream body echo
- SendGrid explicit-content and dynamic-template modes
- SendGrid global/EU API regions and sandbox mode
- Twilio Auth Token or API Key authentication
- Twilio Messaging Service or approved E.164 `From` number
- optional HTTPS Twilio status callback and 1–36000 second validity period
- retry/throttle/permanent/invalid-target classification
- partial or ambiguous provider configuration fails startup
- mock-provider tests and process-level authenticated HTTP tests

## Deliberate boundaries

The first contact PR does not add attachments, arbitrary email headers, BCC, bulk recipient lists, marketing subscription groups, MMS media, WhatsApp, or producer-controlled senders. Those features need explicit authorization, content-size accounting, compliance policy, and tenant-specific provider configuration.

It also does not claim final delivery from the SendGrid or Twilio create-message response. A follow-up must ingest signed callbacks into a durable delivery-event contract:

- SendGrid Signed Event Webhook or OAuth-verified event ingestion
- Twilio `X-Twilio-Signature` verification over the exact public callback URL and request parameters
- replay/idempotency controls
- recipient-safe event persistence
- bounce/block/unsubscribe and failed/delivered state transitions

## Migration requirement

`dd-email-sms-contact-rs` remains the legacy source during migration. It must not continue owning push delivery after cutover. Email/SMS producers may dual-publish during a measured shadow period, then delegate to the standalone contact routes or durable contact subjects once callback reconciliation and idempotency are complete.
