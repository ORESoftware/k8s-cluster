# Security policy

## Supported version

The default branch is the only supported source version until tagged release
channels are published. Deployed services must use immutable image digests or
commits selected by the GitOps configuration.

## Reporting a vulnerability

Do not open a public issue or include credentials, tokens, personal data, or
exploit details in logs. From this repository's **Security** tab, open a private
draft security advisory and include:

- the affected repository, revision, and deployment surface;
- reproduction steps using synthetic credentials;
- expected and observed authentication or authorization behavior;
- impact, known mitigations, and whether active exploitation is suspected.

If private advisory reporting is unavailable, contact an organization owner
privately and ask them to open the advisory. Rotate any credential that may
have been exposed through the runtime secret manager; never paste it into the
advisory.

Maintainers will acknowledge a complete report within two business days,
coordinate remediation privately, and publish a sanitized advisory after
affected deployments and artifacts have been updated.

## Security boundaries

Postgres is authoritative for shared users, sessions, roles, and revocation.
External providers such as Supabase are additional authentication authorities,
not sources of product authorization. Redis is disposable acceleration only.
Provider and service keys are runtime-injected secrets. Telemetry must exclude
raw credentials and high-cardinality identity data.
