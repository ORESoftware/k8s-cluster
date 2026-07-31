# Authentication and storage security boundary

Last reviewed: July 27, 2026.

## Decision

Sonus Auris uses three deliberately separate security layers:

1. **Supabase Auth is the upstream identity provider.** It owns sign-in methods,
   email confirmation, provider OAuth, access-token issuance, and the `auth.uid()`
   context used by Supabase Data API row-level security.
2. **shared-auth is the cross-product identity/session broker.** A verified
   Supabase token may be exchanged for a short-lived shared-auth access token and
   rotating refresh session. The shared-auth Postgres schema is authoritative for
   the OreSoftware-wide principal id, provider link, roles, and refresh-session
   revocation. It never stores or mirrors Supabase passwords.
3. **The Sonus backend owns Sonus application data.** Its RDS role may read/write
   only Sonus tables and object metadata. It does not receive ownership of the
   `shared_auth` schema. Supabase service-role credentials are restricted to the
   explicit Auth Admin operation needed for account deletion and never authorize
   normal data access.

Do not collapse these layers into one database role or one long-lived API key.
Do not create a second Sonus principal merely because a shared-auth and Supabase
identity carry the same email address.

## Current verified properties

The backend already:

- pins Supabase `iss` and `aud`, validates expiry, allows only explicit signing
  algorithms, checks JWK algorithm/use, and caches JWKS with bounded refresh
  frequency and a single-flight lock;
- discards an email claim unless Supabase marked it verified, while continuing to
  identify the account by `sub`;
- namespaces external subjects as `supabase:<sub>`;
- keeps opaque device tokens separate from browser/account JWTs;
- forwards the caller's Supabase token plus only the publishable key to the Data
  API, leaving owner authorization to RLS;
- uses the service-role key only for the explicit Supabase Auth user deletion
  step;
- uses parameter-bound SQL, bounded Postgres pools, presigned object transfers,
  and short-lived evidence URLs.

The Flutter client has an opt-in live two-user test that proves owner reads,
cross-user write rejection, and owner-spoof rejection against the deployed RLS
policies. That test must use only the public/publishable key.

## shared-auth migration path

Direct Supabase verification remains supported during migration. The end state is
one canonical shared-auth principal while Supabase remains the sign-in provider.

1. Register the Sonus Supabase project in `AUTH_SUPABASE_PROJECTS` by project ref,
   issuer, and names of runtime secret variables. Never place credential values
   inline in the registry JSON and never inject a Supabase account-management
   token into the serving Deployment.
2. After a normal Supabase sign-in, exchange the access token at
   `POST /shared-auth/auth/exchange`. Store the returned refresh token only in
   secure OS storage and send it only to refresh/logout endpoints.
3. Put shared-auth in the cluster gateway for browser/account routes using its
   `/auth/verify` auth-request endpoint, or consume the reviewed shared-auth guard
   library. Do not invent unsigned identity headers.
4. Map the shared-auth `sub` to a Sonus account through an explicit identity-link
   table containing authority, tenant, and provider subject. Preserve the current
   `supabase:<sub>` link during the transition. Never auto-link by email.
5. During a dual-authority window, use the shared-auth library's bounded race and
   its `degraded` versus `unauthenticated` distinction. A network failure is not
   proof that a credential is invalid, but privileged work still fails closed.
6. After every active account has a verified shared principal and rollback has
   been exercised, make shared-auth mandatory for account/browser routes. Device
   bearer tokens remain Sonus-specific and continue to map to the canonical Sonus
   account id.

A direct source dependency on `shared-auth-lib/rust` is not currently portable in
a standalone checkout because it references the sibling
`shared-auth-interfaces` repository by path. Consume it from the pinned shared-auth
monorepo checkout, or publish/package the two crates together before adding a Git
dependency here. Do not copy-paste a second JWT/JWKS implementation merely to
avoid that packaging step.

## Supabase controls

Production must enforce all of the following:

- asymmetric signing keys and the project JWKS endpoint; retain legacy HS256 only
  during a documented rotation window;
- exact allowed redirect URLs and no wildcard production origins;
- email confirmation for any email-based workflow;
- short access-token lifetime, refresh-token rotation/reuse protection, MFA for
  operators, and rate limits on sign-in/recovery/OTP endpoints;
- RLS enabled on every table exposed through Data API, with `user_id default
  auth.uid()` and owner predicates in both `USING` and `WITH CHECK` where writes
  are allowed;
- no direct `anon` table privileges for owner-scoped tables;
- no authorization decisions from mutable `raw_user_meta_data`; roles/claims must
  come from server-controlled app metadata or shared-auth Postgres;
- service-role/secret keys only in server secret stores, never Flutter defines,
  web bundles, GitHub artifacts, screenshots, logs, or test fixtures.

Run:

```sh
export SONUS_SUPABASE_DATABASE_URL='postgresql://...?...sslmode=verify-full'
scripts/audit_auth_storage.sh
```

The read-only audit reports missing tables, disabled RLS, unsafe owner defaults,
missing owner policies, anonymous privileges, and exposed `SECURITY DEFINER`
functions without a fixed `search_path`.

## Sonus RDS controls

Use a dedicated application login role. It must not be superuser, `BYPASSRLS`,
`CREATEROLE`, `CREATEDB`, or replication-capable. Revoke `CREATE` on schema
`public` from `PUBLIC`; grant the app role only the Sonus schema/table/sequence
operations required by reviewed queries. Require TLS and configure bounded
`statement_timeout`, `lock_timeout`, and
`idle_in_transaction_session_timeout` at the role/database layer.

Run both audits with short-lived operator DSNs:

```sh
export SONUS_SUPABASE_DATABASE_URL='postgresql://...?...sslmode=verify-full'
export SONUS_RDS_DATABASE_URL='postgresql://...?...sslmode=verify-full'
export SONUS_AUDIT_WARNINGS_AS_ERRORS=1
scripts/audit_auth_storage.sh
```

Never use these operator DSNs in the app Deployment. Production uses a narrower
runtime role and the audit is performed from a controlled operator job.

## Deletion and incident handling

Account deletion spans Sonus RDS, object stores, shared-auth sessions/provider
links, and Supabase Auth. Treat it as an idempotent, observable workflow with a
stable deletion id. Revoke sessions/tokens first, prevent new uploads, delete
objects and metadata with retry/fencing, then delete the external Auth user.
Record only ids, states, counts, and errors—never access tokens, refresh tokens,
service keys, audio content, or presigned URLs.

If any signing, service-role, R2, database, or store credential appears in a log,
artifact, generated file, branch, or pull request, rotate it rather than merely
deleting the evidence.
