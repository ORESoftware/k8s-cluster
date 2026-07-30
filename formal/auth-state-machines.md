# Authentication state-machine verification

This repository uses two complementary verification layers:

1. `tests/auth_state_model.rs` is a dependency-free, exhaustive bounded model
   of refresh rotation, session revocation, access-token lifetime, and signing
   key overlap/retirement.
2. `tests/postgres.rs` exercises the real transactional session implementation,
   including two concurrent attempts to consume the same refresh token.

The bounded model is intentionally small enough to run on every pull request.
It explores every distinct state reachable within eight transitions. When an
invariant fails, the test prints the shortest action trace found by breadth-first
search. Increase `MAX_DEPTH` locally when changing the protocol, but keep the CI
bound deterministic.

## Prioritized inventory

| Priority | Machine | Current verification |
|---|---|---|
| P0 | Refresh rotation and replay rejection | bounded model plus concurrent Postgres regression |
| P0 | Session revocation and access-token rejection | bounded model plus Postgres active-session checks |
| P0 | Signing-key rotation, overlap, and retirement | bounded model plus JWKS/token unit tests |
| P0 | Issuer and audience isolation | JWT verifier unit and integration tests |
| P1 | Login, token issuance, logout, and account disablement | HTTP and Postgres integration tests |
| P1 | Step-up authentication and device enrollment | AAL integration tests; expand the model when device state is persisted |
| P1 | Role and tenant authorization | policy and provider-identity tests |
| P2 | Passwordless recovery and provider outage behavior | HTTP tests; add a model once recovery state is persisted on `main` |

## Safety invariants

- A revoked or expired session never accepts an access token.
- A consumed refresh generation cannot be used again, including concurrently.
- Every issued access token expires no later than the absolute session deadline.
- Tokens signed by a retired key are rejected; the active signing key is always
  trusted.
- Rotating a key keeps the previous key trusted during the overlap window.
- Authentication does not grant product roles; authorization remains a
  separate policy decision.

## Liveness properties

- An active, unexpired session can rotate its current refresh token.
- After signing-key rotation, an active session can refresh and receive a token
  signed by the new key.
- During the overlap window, an otherwise valid token signed by the previous
  key remains verifiable.

The model treats storage and cryptography as atomic protocol actions. The
Postgres and JWT tests are therefore required counterparts: they verify that
the implementation actually supplies the atomicity and validation assumed by
the model.
