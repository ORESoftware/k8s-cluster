# Organization `.github` relationship publication

This runbook covers the bounded publisher that reconciles each organization in
the current certified fleet and publishes a repository-relationship registry
from its public `.github` repository.

## Source and scope

The trusted implementation lives in `ORESoftware/k8s-cluster`. Publication
must run from the current `main` commit after its pull-request checks pass.
The authoritative fleet comes from
`scripts/ops/bootstrap_current_org_dotgithub_repositories.py` and currently
contains exactly 62 unique organizations. The personal `ORESoftware` account
is not part of that fleet.

Every organization must already have a public, non-archived `.github`
repository. The relationship publisher writes or reconciles these managed
artifacts in each one:

- `architecture/repository-relationships.json`
- `architecture/repository-relationships.schema.json`
- `architecture/REPOSITORY_RELATIONSHIPS.md`
- a managed discovery block in `README.md`
- the same managed discovery block in `profile/README.md`

Existing unmanaged content outside the publisher's bounded markers is
preserved, except that an existing README or profile line containing an exact
private repository identity is replaced with a generic withholding notice.
This remediation removes an existing public disclosure without copying the
private identity into logs or reports. Newly generated managed content is not
silently redacted: publication fails if that content contains a private
identity.

## Security boundary

The publisher accepts an owner credential only through the workflow's
one-time RSA-OAEP-SHA256 challenge. Never place a plaintext credential in a
workflow input, issue comment, repository secret, artifact, log, or tracked
file.

The workflow must reject publication unless all of these conditions hold:

1. The trusted repository, issue, actor, comment author, and exact trigger
   match the bounded workflow contract.
2. The workflow source is fetched from the current trusted `main` commit.
3. The credential resolves to a syntactically valid authenticated GitHub
   account; the login is kept out of public reports.
4. That authenticated account has active administrator membership in every
   organization in the current 62-organization fleet. The membership proof,
   rather than a hard-coded account name, is the authorization boundary.
5. Every public `.github` repository already exists and passes identity,
   visibility, and archival checks.
6. Pre-write and post-write privacy checks find no private repository names or
   private relationship edges in public output.
7. The final report contains exactly one verified result for each of the 62
   organizations, in the canonical fleet order.

## Publication sequence

1. Merge the validated current-fleet relationship publisher to `main`.
2. Post the exact bounded trigger on the designated operations issue.
3. Read the public key and nonce from the newly generated challenge comment.
4. Encrypt the owner credential locally for that nonce and post only the
   workflow-defined ciphertext response marker. The trigger and response
   comment author remain fixed to `ORESoftware`; the encrypted credential may
   belong to any account that proves active administrator membership across
   the complete fleet.
5. Confirm the workflow verifies all 62 organizations and publishes its
   sanitized completion report.
6. Independently sample the generated JSON, schema, Markdown, and profile
   discovery blocks in representative product and test organizations.

## Failure handling

Treat a timeout, membership mismatch, missing `.github` repository, privacy
failure, incomplete report, changed fleet order, or unexpected organization
count as a failed publication. Do not weaken the allowlist, identity checks,
privacy checks, or exact-result-count assertion to force completion. Reconcile
the underlying access or inventory problem and run a new one-time challenge.
