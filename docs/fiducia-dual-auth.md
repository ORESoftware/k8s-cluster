# Fiducia dual-auth contract

Fiducia has two human-facing security planes and they must not share a Supabase
project:

| Plane | Application | Supabase registry name | Local data |
|---|---|---|---|
| Customer | `fiducia-cloud/fiducia-customer.rs` | `fiducia-customer` | customer Postgres schema/session observations |
| Operator | `fiducia-cloud/fiducia-admin.rs` | `fiducia-admin` | isolated admin Postgres/operator registry |

Each application continues to receive its own process-local `SUPABASE_URL` and
`SUPABASE_PUBLISHABLE_KEY`. The Kubernetes Secret that supplies those variables
must be different for each workload. Shared Auth accepts both issuers through one
provider registry and reissues a provider-neutral Shared Auth session.

## Provider registry

`AUTH_SUPABASE_PROJECTS` contains metadata only. Credential values stay in
separate environment variables extracted from the secret store.

```json
[
  {
    "name": "fiducia-customer",
    "project_ref": "<customer-project-ref>",
    "publishable_key_env": "AUTH_SUPABASE_FIDUCIA_CUSTOMER_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_FIDUCIA_CUSTOMER_SECRET_KEY"
  },
  {
    "name": "fiducia-admin",
    "project_ref": "<admin-project-ref>",
    "publishable_key_env": "AUTH_SUPABASE_FIDUCIA_ADMIN_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_FIDUCIA_ADMIN_SECRET_KEY"
  }
]
```

The two project refs and derived issuers must differ. A token is routed using its
unverified issuer only to select a verifier; the selected verifier rechecks the
issuer, audience, algorithm, signature, and expiry. A customer-signed token can
therefore never authenticate through the admin verifier, or vice versa.

## Dual-auth behavior

A Shared Auth JWT is verified locally against Shared Auth JWKS. When the caller
instead presents a Supabase access token, consumers use `shared-auth-lib` to race:

1. exchange the provider token at `POST /auth/exchange`, then introspect the new
   Shared Auth token; and
2. verify the provider token directly with that application's Supabase project.

The first successful, policy-compliant arm wins. One unavailable arm does not
abort the other. Two definite invalid results are unauthenticated; any unresolved
outage/timeout combination is degraded and must fail closed for privileged work.
A request with no Shared Auth JWT and no provider credential remains anonymous;
there is no credential-free token minting path.

## Postgres authority and Supabase mirroring

Supabase remains the external identity provider and retains its built-in auth
schema and defaults. Shared Auth does not copy password hashes. It owns the
provider-neutral authorization and session model in Postgres:

- `shared_auth.principals` — canonical Shared Auth users;
- `shared_auth.provider_identities` — immutable provider/tenant/subject links;
- `shared_auth.sessions` — hashed rotating refresh sessions and assurance state;
- `shared_auth.roles` — local exact roles used in reissued JWTs;
- passwordless and MFA challenge/factor tables.

A verified Supabase identity is upserted into the provider link and principal,
then local roles and session state are loaded from Postgres before Shared Auth
issues its JWT. Matching email addresses alone never link two provider identities.

## Delivery integrations

SendGrid and Twilio remain optional and independently fail closed:

- SendGrid delivers magic links and six-digit email OTPs through the Mail Send API.
- Twilio Verify starts and checks SMS second-factor challenges; Shared Auth never
  stores the SMS code.
- partial integration configuration is rejected at startup; fully absent
  configuration disables only that endpoint family.

Unit contract tests use local mock endpoints to assert authentication headers,
request paths, payload/form fields, accepted statuses, and provider-error handling
without sending real email or SMS.

## Verification

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

`tests/fiducia_contract.rs` proves two-project issuer routing, cross-project
signature rejection, and the required Postgres users/sessions/roles/provider-link
schema contract.
