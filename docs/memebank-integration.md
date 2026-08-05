# MemeBank API and SDK integration contract

Status: DEN-1526 cross-product boundary.

MemeBank is a companion image and meme catalog. MemeBank owns image ingestion, source synchronization, OCR, visual tagging, semantic retrieval, collections, and sharing policy. ClipTown owns generic clipboard history, encrypted transfer storage, retention, synchronization, and transfer acknowledgement state.

The canonical MemeBank-to-ClipTown integration is a versioned HTTPS API consumed through official ClipTown SDKs. It is deployable when only one product has a native application on a device, when neither native application is installed, and from server, browser, desktop, or automation clients that possess an authorized shared-auth session. Product interoperability must never depend on phone co-installation.

## Non-negotiable product boundary

The integration must not:

- probe whether MemeBank or ClipTown is installed;
- invoke a MemeBank or ClipTown custom URL scheme or mobile deep link;
- use local IPC, loopback ports, platform intents, pasteboard listeners, shared extensions, or app groups as an app-to-app transport;
- background-monitor the clipboard to infer activity in the other product;
- read the other product's private database;
- share object-store credentials, provider credentials, refresh tokens, encryption keys, or service-wide bypass tokens;
- fall back to clipboard handoff when the ClipTown API is unavailable.

Native **Copy** in either product remains an independent operating-system feature. It may place ordinary bytes, files, or user-authorized standard URLs on the system clipboard for the user to paste anywhere. It is not the MemeBank-to-ClipTown integration transport, does not prove that the other application is installed, and is not a prerequisite for API interoperability.

## Shared-auth mediation

Shared-auth is the only authentication and assurance boundary used by the integration. MemeBank never calls a 3FA backend, imports a 3FA client, validates a 3FA proof, receives a factor-specific header, or branches on whether a factor application is installed.

A TOTP, passkey, email OTP, SMS OTP, recovery ceremony, or ceremony initiated through 3FA is visible to the products only through signed, revocation-aware shared-auth claims such as `sub`, `sid`, `aud`, `azp`, `scope`, `aal`, `acr`, `amr`, and `auth_time`.

For each ClipTown operation, MemeBank asks shared-auth for a short-lived delegated token with:

- authorized party/client ID `memebank-api`;
- audience `cliptown-api`;
- exactly one permitted scope;
- the original subject and active session provenance;
- a new token identifier and non-recursive delegation provenance;
- an expiry capped by both policy and the parent token.

The supported scopes are:

| Operation | Required scope | Recent LOA2 |
|---|---|---|
| Create transfer | `cliptown:memebank:write` | Required |
| List transfers | `cliptown:memebank:read` | Not required by this contract |
| Get transfer | `cliptown:memebank:read` | Not required by this contract |
| Acknowledge transfer | `cliptown:memebank:write` | Required |
| Cancel transfer | `cliptown:memebank:delete` | Required |

Shared-auth rejects unconfigured client/audience/scope tuples, inactive or revoked sessions, recursive delegation, stale assurance for sensitive scopes, and scope widening. ClipTown still authorizes each request against the delegated subject and resource owner; a valid delegated token is not a service-wide bypass.

MemeBank does not need to know whether the accepted authentication method originated in 3FA. It may apply a bounded shared-auth `amr` policy, but it must not accept raw OTP codes, seeds, factor proofs, or 3FA-specific tokens.

## Versioned transfer API

The canonical interface is defined in `cliptown-interfaces` and consumed through `cliptown-clients`. Implementations must use the official SDK rather than duplicating endpoints, redirect behavior, payload bounds, cursor rules, idempotency handling, or error mapping.

Version 1 exposes the following operations:

- `POST /v1/integrations/memebank/transfers` — create an idempotent transfer;
- `GET /v1/integrations/memebank/transfers` — list transfers using an opaque cursor;
- `GET /v1/integrations/memebank/transfers/{transfer_id}` — get one transfer;
- `POST /v1/integrations/memebank/transfers/{transfer_id}/ack` — acknowledge, ignore, or reject idempotently;
- `DELETE /v1/integrations/memebank/transfers/{transfer_id}` — cancel a transfer.

Clients reject unsupported major contract versions. Unknown additive response fields are ignored only where the versioned interface permits them. Cursors remain opaque, identifiers and media types are bounded, redirects are rejected, success and error bodies are size-limited, and retryable failures map to deterministic SDK errors.

## Ciphertext-only payload boundary

ClipTown stores and transports opaque encrypted transfer envelopes. Cleartext media and sensitive metadata are encrypted before the API request. The public envelope may carry only bounded routing and integrity fields such as contract version, direction, source item identifier, media type, content digest, content length, cipher algorithm, nonce, ciphertext, associated-data hash, key identifier, and expiry.

The following values are forbidden in transfer payloads, metadata, logs, traces, metrics, queue messages, or error bodies:

- bearer or refresh tokens;
- OTP seeds or codes;
- passkey assertions or factor proofs;
- encryption keys or plaintext key envelopes;
- provider or object-store credentials;
- durable private object URLs;
- presigned upload or download URLs;
- plaintext OCR, captions, embeddings, private filenames, or private source metadata.

Encryption keys remain on authorized end devices or inside an explicitly governed key service. ClipTown's transfer API is not a key escrow service.

## Idempotency, ownership, and state

Create and acknowledgement operations require caller-owned idempotency keys. Replaying the same key with the same normalized request returns the original result; reusing it with a different request fails deterministically. Transfer state changes follow the interface state machine and are applied atomically.

Every operation is scoped to the delegated subject and resource owner. Listing and cursors must not cross users or tenants. Cancellation and acknowledgement must be rejected after incompatible terminal transitions. Expiry is enforced server-side and does not rely on a client clock alone.

## Availability and deployment independence

MemeBank and ClipTown may be deployed, upgraded, tested, or temporarily unavailable independently. When the ClipTown API or official SDK is unavailable, the integration reports a bounded unavailable error and preserves the user's MemeBank data for a later explicit retry. It does not discover a local application, open a deep link, start a loopback server, or silently substitute native clipboard export.

An end-to-end release requires all of the following:

1. merged shared-auth delegation server and SDK support;
2. merged ClipTown interface and official SDK support;
3. a ClipTown backend implementation with token verification, ownership, persistence, idempotency, state, and expiry enforcement;
4. a MemeBank adapter using the official shared-auth and ClipTown SDKs;
5. cross-product conformance tests with no mobile applications installed.

Until the backend implementation exists and passes those tests, the interface and SDK work is a contract preview rather than a deployable integration.

## Required negative tests

Cross-product CI must prove rejection of:

- direct 3FA dependencies, 3FA endpoints, factor-specific headers, or app-presence checks;
- wrong `aud`, `azp`, client ID, scope, issuer, subject, or ownership;
- inactive or revoked parent sessions;
- missing or stale LOA2 for write and delete operations;
- recursive delegation and widened scopes;
- replay with an idempotency-key/body mismatch;
- unsupported contract versions, malformed cursors, invalid identifiers, redirects, and oversized bodies;
- cleartext secret fields or durable private URLs in metadata;
- dependence on either mobile application being installed;
- API failure followed by local IPC, deep-link, or clipboard fallback.

Material changes require coordinated pull requests in shared-auth, `cliptown-interfaces`, `cliptown-clients`, the ClipTown backend, MemeBank, and this monorepo documentation.
