# Shared Auth realms and federated customer SSO

Status: accepted target contract; server/schema implementation is tracked by [DEN-2193](https://linear.app/denman/issue/DEN-2193/shared-auth-server-add-realm-isolation-and-federated-customer).

Parent architecture: [DEN-2189](https://linear.app/denman/issue/DEN-2189/shared-auth-isolate-admin-and-customer-authentication-on-separate) and [`shared-auth-server.rs#1`](https://github.com/shared-auth/shared-auth-server.rs/issues/1).

Infrastructure contract: [`shared-auth-infra#6`](https://github.com/shared-auth/shared-auth-infra/issues/6).

## Decision

Run the same Shared Auth codebase as two independent security realms:

```text
shared-auth-admin     -> admin Supabase project     -> admin-auth PostgreSQL RDS
shared-auth-customer  -> customer Supabase project  -> customer-auth PostgreSQL RDS
```

The codebase may share libraries, interfaces, and release engineering. The deployments do not share runtime authority.

This document extends the provider-neutral authority model in [`DESIGN.md`](DESIGN.md) and the two-Supabase-project boundary in [`fiducia-dual-auth.md`](fiducia-dual-auth.md). Where an older document assumes one Shared Auth deployment or one Postgres authority for both customer and operator traffic, this realm-isolation contract is the target production topology.

## Realm invariants

Admin and customer realms have different values for:

- issuer URL;
- accepted audience/client registry;
- asymmetric signing-key source and JWKS;
- cookie name, domain, path, and encryption/signing material;
- PostgreSQL DSN and database roles;
- Supabase project and provider credentials;
- service-to-service introspection credential;
- recovery, MFA, session-lifetime, and step-up policy;
- service account, network policy, secret path, KMS key, and deployment identity;
- audit stream, dashboards, alerts, and SLOs.

Startup and deployment validation must fail closed when an admin/customer pair reuses a realm identifier, issuer, signing-key reference, cookie namespace, DSN, Supabase project reference, OAuth client, introspection credential, or recovery secret.

No availability fallback crosses the realm boundary. Customer traffic never uses Admin Auth RDS or admin keys, and admin traffic never uses Customer Auth RDS or customer keys.

## Authority model

### Supabase

The realm-specific Supabase project is an upstream authentication provider. It may verify password, passwordless, social, OTP, or other configured provider ceremonies.

Shared Auth does not copy or independently edit:

- Supabase password hashes;
- raw Supabase refresh tokens;
- provider secrets;
- the complete Supabase `auth` schema.

A verified provider token is input evidence. Shared Auth revalidates issuer, algorithm, signature, audience, expiry, and provider-specific policy before resolving a provider-neutral principal.

### Shared Auth

Each realm owns its own:

- canonical principals and lifecycle state;
- immutable provider identity links;
- Shared Auth sessions and rotating refresh-token digests;
- devices, revocation, assurance, recovery, and audit;
- OAuth/OIDC clients and token policy for that realm;
- realm-specific coarse grants where explicitly part of the Shared Auth contract.

The admin and customer principal namespaces are intentionally separate. An employee may have both an admin principal and a customer principal, but any relationship is explicit, one-way or bounded as required, and audited. Customer authentication cannot create, enroll, recover, or unlock an admin principal.

### Product services

Product databases remain authoritative for:

- organizations, workspaces, and tenants;
- application-local roles and memberships;
- resource permissions;
- billing tenants, financial grants, and subscriptions;
- domain records and product audit.

Shared Auth answers who authenticated, through which realm/provider/client/session, with what assurance, and whether the Shared Auth session remains acceptable. It does not become the universal product-authorization database.

Consumers never query Shared Auth tables directly or perform cross-database joins. They use signed tokens, authenticated introspection, bounded events, or controlled just-in-time local profile creation.

## Customer identity model

The customer realm uses one stable customer principal plus explicit per-application enrollment.

```sql
create table principals (
  id uuid primary key,
  status text not null,
  created_at timestamptz not null
);

create table provider_identities (
  id uuid primary key,
  principal_id uuid not null references principals(id),
  provider_tenant text not null,
  provider_subject text not null,
  verified_at timestamptz,
  unique (provider_tenant, provider_subject)
);

create table applications (
  id uuid primary key,
  application_key text not null unique,
  status text not null
);

create table application_accounts (
  application_id uuid not null references applications(id),
  principal_id uuid not null references principals(id),
  status text not null,
  created_at timestamptz not null,
  primary key (application_id, principal_id)
);

create table oauth_clients (
  id uuid primary key,
  application_id uuid not null references applications(id),
  client_id text not null unique,
  client_type text not null,
  redirect_uris jsonb not null,
  audiences jsonb not null,
  allowed_scopes jsonb not null,
  status text not null
);

create table sessions (
  id uuid primary key,
  principal_id uuid not null references principals(id),
  client_id text not null,
  device_id uuid,
  auth_time timestamptz not null,
  assurance_level text not null,
  revoked_at timestamptz
);
```

This is an ownership sketch, not startup DDL. The exact schema is declarative, reviewed with existing database definitions, and applied outside the server.

### Provider linking

The canonical link key is `(provider_tenant, provider_subject)` after cryptographic verification. Email and phone attributes are mutable verified attributes, not canonical account-linking keys.

Explicit identity linking requires:

- recent reauthentication;
- proof for the identities being linked where applicable;
- replay-safe intent and expiry;
- an auditable link/unlink operation;
- no change to the stable `principal_id` merely because an email changes.

Email equality alone never links principals.

## Applications and application accounts

An `application` represents a product boundary. An `application_account` represents one global customer principal's enrollment in that product.

The same principal may have:

```text
App A: active
App B: invited
App C: disabled
```

Disabling App C does not disable App A or App B. Disabling the global principal blocks every customer application.

### Enrollment policies

- **Just-in-time enrollment:** approved public first-party products may create an active application account during first successful login.
- **Consent-gated enrollment:** applications require explicit customer consent before activation.
- **Invitation/tenant-gated enrollment:** private or B2B products require an invitation or product-owned tenant membership before activation.
- **Pre-provisioned enrollment:** an approved product workflow creates the application account before first login through an authenticated, idempotent service path.

The customer realm authenticates the principal and enforces the application-account state. The product still resolves its local organization/workspace/resource permissions.

## OAuth/OIDC client contract

Each web, mobile, desktop, CLI, BFF, or service surface is registered explicitly. Example client IDs may include:

```text
sonus-auris-web
memebank-mobile
quaestor-console
fiducia-customer
streempilot-desktop
hypesiege-web
```

Every client record has:

- exact redirect URIs;
- public/confidential client type;
- allowed grant/response modes;
- PKCE requirements;
- exact token audiences;
- allowed scopes;
- access/refresh/session lifetimes;
- consent and application-enrollment policy;
- owner and status;
- mobile/desktop callback or loopback restrictions where applicable.

Wildcard redirect URIs, ambient client discovery, shared client secrets across applications, and default cross-application audiences are prohibited.

## Federated customer login flow

### App A first login

1. App A generates state, nonce, and PKCE values and redirects to the customer realm.
2. The customer realm authenticates through the customer Supabase project or another approved customer provider.
3. Shared Auth resolves or creates the provider-neutral customer principal.
4. Shared Auth creates or validates the App-A application account under the configured enrollment policy.
5. Shared Auth creates its own central customer-realm session.
6. App A receives an authorization code bound to App A, its redirect URI, PKCE challenge, requested scopes, and transaction state.
7. App A exchanges the code and receives an App-A-specific token.
8. App A validates exact issuer, audience, expiry, authorized party/client, session, and scopes.
9. App A loads its product-local tenant/resource authorization.

### App B after App A

1. App B starts its own authorization request with its own state, nonce, PKCE, redirect URI, audience, and scopes.
2. The customer realm may reuse the existing central customer login session, subject to freshness/step-up policy.
3. Shared Auth independently creates or validates the App-B application account and consent/invitation policy.
4. App B receives a new App-B-specific code and token.
5. App B rejects App-A tokens; App A rejects App-B tokens.

The realms share a customer login ceremony, not bearer tokens, cookies, product roles, or database access.

## Token contract

Customer application access tokens include only bounded claims required by the published interface, such as:

- `iss` — exact customer realm issuer;
- `sub` — stable customer principal ID;
- `aud` — exact target application/API audience;
- `azp` or equivalent authorized client identifier;
- `sid` — Shared Auth session identifier/revocation handle;
- `iat`, `exp`, and optionally `nbf`;
- assurance claims such as `auth_time`, `aal`, `acr`, and bounded `amr`;
- approved scopes;
- provider provenance where required for audit, never as product tenancy.

A product role, organization membership, billing tenant, or resource permission is not inferred from provider project metadata, email domain, or global customer enrollment.

Admin tokens use the admin issuer, admin audiences/clients, admin session namespace, and admin policy. Customer and admin token validators use disjoint trust configuration.

## Cookie and browser-session contract

Admin and customer cookies are distinct in name, domain/path, key material, lifetime, SameSite policy, and application surface.

A customer cookie is not sent to or accepted by an admin host. An admin cookie is not sent to or accepted by customer products. A shared parent-domain cookie is prohibited when it would make the browser transmit customer and admin session material to the same origin set.

Authorization requests still use state, nonce, PKCE, redirect allowlists, CSRF defenses, and bounded transaction lifetimes even when a central customer login session already exists.

## Request validation and database load

Normal product requests validate Shared Auth access tokens locally with cached JWKS:

```text
signature
-> exact issuer
-> exact audience
-> expiry/not-before
-> authorized client
-> required scopes/assurance
-> product-local tenant/resource authorization
```

They do not call PostgreSQL or Shared Auth introspection on every request.

Authenticated, revocation-aware introspection is used for explicit high-risk actions and immediate-revocation policy. The caller uses an independent service credential; the user token remains the object being inspected. Introspection does not grant product membership or resource permission.

The access-token TTL defines the maximum revocation delay for routes using offline validation only.

## Verification and failover semantics

The reusable guard may evaluate a Shared Auth token and a realm-specific provider token in parallel, but the security decision is not an unsafe first-positive `Promise.race`.

Required semantics:

- a credential is routed only to the verifier selected by its untrusted issuer hint, and that verifier rechecks issuer, algorithm, signature, audience, and expiry;
- explicit invalid, revoked, banned, or disabled evidence denies according to the versioned decision policy;
- unavailable or timed-out authority is `degraded`, never silently valid;
- privileged actions fail closed when no authoritative acceptable result exists;
- no credential means anonymous; there is no credential-free minting path;
- a verifier that finishes after the response cannot change the authorization decision or perform unbounded background authorization;
- timestamps alone cannot resurrect revoked state;
- read repair is an authenticated, idempotent event or controlled reconciliation flow, not a consumer's last-writer-wins database update.

Shared Auth owns its principal/session/revocation model. Supabase is not a byte-for-byte source-of-truth mirror for Shared Auth tables.

## Admin realm requirements

The admin realm adds stricter policy rather than merely an `is_admin` flag on a customer account:

- dedicated workforce/provider allowlist;
- mandatory strong MFA/assurance for interactive operators;
- shorter sessions and refresh policy;
- fresh step-up for destructive, financial, security, recovery, and privilege changes;
- privileged audit with actor, session, client, assurance, request/correlation, and outcome;
- explicit operator invitation/provisioning and offboarding;
- audited break-glass path with limited scope and lifetime;
- no customer-provider fallback;
- no automatic customer-to-admin account promotion;
- no shared recovery ceremony or customer support bypass.

## Migration contract

For each legacy source:

1. classify credential/provider, Shared Auth, and product-local data ownership;
2. establish a stable `legacy_user_id -> principal_id` mapping;
3. migrate verified provider tenant/subject links and lifecycle state;
4. create application accounts under an explicit enrollment policy;
5. register exact clients, redirects, audiences, scopes, and PKCE requirements;
6. exchange existing sessions only through a reviewed protocol or let them expire and require reauthentication;
7. never copy raw refresh tokens or Supabase password hashes;
8. preserve product organizations, memberships, roles, billing grants, and resource permissions in the product database;
9. run shadow comparison without authorizing from an untrusted shadow result;
10. remove legacy auth writers/queries/secrets only after observation and rollback windows.

Customer migration precedes the separate admin migration. Operator accounts reauthenticate and reenroll MFA/devices as required by admin policy rather than being copied automatically from customer state.

## Server/schema implementation checklist

- [ ] add an explicit bounded realm profile rather than implicit host-name inference;
- [ ] validate realm/issuer/key/cookie/DSN/provider/client invariants at startup;
- [ ] define global customer principals and per-application accounts declaratively;
- [ ] define exact OAuth/OIDC client registration and PKCE policy;
- [ ] issue exact-audience application tokens and reject cross-application tokens;
- [ ] keep admin and customer principal/session namespaces disjoint;
- [ ] preserve authenticated introspection and local JWKS validation contracts;
- [ ] prevent email-only identity linking;
- [ ] prevent provider metadata from becoming product tenancy/authorization;
- [ ] publish schema, OpenAPI, interfaces, and migration compatibility changes;
- [ ] add unit/integration/formal obligations for realm and client isolation;
- [ ] link executable browser/API/database/outage/load evidence from [`shared-auth-e2e#12`](https://github.com/shared-auth/shared-auth-e2e/issues/12).

## Acceptance

The server/schema track remains open until:

- admin/customer configuration reuse fails closed;
- customer and admin tokens/cookies/clients are mutually rejected;
- App A/App B receive distinct audiences and reject one another's tokens;
- global-principal and application-account operations are deterministic and idempotent;
- provider linking never relies on email equality alone;
- product authorization remains product-owned;
- ordinary product requests do not require an auth-database query;
- migration fixtures contain no raw refresh tokens or Supabase password hashes;
- exact schema, interfaces, server, client, and E2E revisions are linked in GitHub and Linear.

This documentation commit defines the contract only; it does not claim the realm/schema implementation or production migration is complete.
