# Canonical quote API v1

`schema/quote.schema.json` is the sole wire-contract authority for the signed-in Canonical compliance quote workflow. Services and clients consume generated bindings or prove compatibility with the golden fixtures in `fixtures/quote-v1`.

## Public surfaces

| Host | Route | Authentication |
| --- | --- | --- |
| `app.canonical.plus` | `/u/quote` and `/u/quote/{quoteId}` | Host-only Shared Auth browser cookie verified by the web origin; CSRF required for mutations. |
| `app.canonical.plus` | `/api/v1/quotes*` | Browser BFF route using the same verified cookie and CSRF contract. The web origin forwards only after verification. |
| `api.canonical.plus` | `/api/v1/quotes*` | Short-lived Shared Auth bearer token independently verified by the API origin. Browser cookies are not accepted here. |

Cloudflare is routing and defense in depth, not the authorization authority. Both Rust origins verify credentials and revocation independently.

## Internal web-to-API contract

The trusted web tier may call the private API service with exactly:

- `x-canonical-internal-token`: a dedicated web/API service credential;
- `x-canonical-subject`: the verified Shared Auth subject.

The edge strips caller-supplied versions of these headers. The API never accepts `x-canonical-user-id`, `x-canonical-user-email`, `x-canonical-service-token`, or client-supplied owner/tenant identity as authority. Contact fields in `QuoteRequest` are follow-up metadata, not authentication claims.

## REST and WebSocket routes

| Method | Path | Request | Success response |
| --- | --- | --- | --- |
| `POST` | `/api/v1/quotes` | `QuoteRequest`; `Idempotency-Key` header required | `202 QuoteSubmissionResponse` |
| `GET` | `/api/v1/quotes/{quoteId}` | — | `200 QuoteDetail` |
| `GET` | `/api/v1/quotes?limit={1..100}&cursor=...` | `QuoteListQuery` | `200 QuoteListResponse` |
| `POST` | `/api/v1/quotes/{quoteId}/retry` | empty body; `Idempotency-Key` required | `202 QuoteRetryResponse` |
| WebSocket | `/api/v1/quotes/{quoteId}/events` | authenticated upgrade | persisted `QuoteStatusEvent` messages |

The app and API hosts use the same path shapes. Authentication differs by host. Query-string bearer tokens are prohibited. Non-browser SDKs authenticate the WebSocket upgrade with the same bearer authority as REST; browser clients use the `app.canonical.plus` BFF/cookie route.

## State machine

Public quote status is exactly:

```text
queued -> analyzing -> ready
                    -> failed
failed --retry--> queued
```

Progress `stage` may additionally report `loading_context` and `validating`. `ready` and `failed` are terminal for one attempt. A retry creates later persisted events for the same quote and returns it to `queued`.

PostgreSQL and its append-only event stream are authoritative. WebSocket delivery is disposable; clients recover through `GET /api/v1/quotes/{quoteId}`.

## Identifier and framework rules

- JSON uses lowerCamelCase field names.
- Quote identifiers are UUIDs and appear as `quoteId` in every public payload.
- Framework wire names are exactly those enumerated by `QuoteRequest.frameworks`.
- `contextKey` defaults to `quote-analysis`; clients may request another bounded key, but the server chooses only from its allow-list.
- `Idempotency-Key` is 8–128 ASCII characters from `[A-Za-z0-9._:-]` and is scoped to the authenticated owner and operation.

## Privacy and output boundary

Owner and tenant identity are derived from the accepted credential. Public responses omit internal service tokens, database identifiers, prompts, raw model responses, model-attempt metadata, tenant internals, and persistence diagnostics. Gemini output is preliminary scoping assistance for human review, not an audit opinion, certification, attestation, or legal conclusion.
