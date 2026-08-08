# Organization `.github` relationship publication

This runbook covers the bounded publisher that creates or reconciles each
approved organization's public `.github` repository and publishes its
repository-relationship registry.

## Source and scope

The trusted implementation lives in `ORESoftware/k8s-cluster`. Publication
must run from the current `main` commit after its pull-request checks pass.
The fixed organization allowlist is defined in the publication workflow and
must contain exactly 36 unique organizations.

Each organization receives these managed relationship artifacts:

- `architecture/repository-relationships.json`
- `architecture/repository-relationships.schema.json`
- `architecture/REPOSITORY_RELATIONSHIPS.md`
- a managed discovery block in `README.md`
- the same managed discovery block in `profile/README.md`

Existing unmanaged content outside the publisher's bounded markers is
preserved.

## Security boundary

The publisher accepts an owner credential only through the workflow's
one-time RSA-OAEP-SHA256 challenge. Never place a plaintext credential in a
workflow input, issue comment, repository secret, artifact, log, or tracked
file.

The workflow must reject publication unless all of these conditions hold:

1. The trusted repository, issue, actor, comment author, and exact trigger
   match the bounded workflow contract.
2. The workflow source is fetched from the current trusted `main` commit.
3. The credential resolves to the expected owner identity.
4. The owner has active administrator membership in every allowlisted
   organization.
5. Pre-write and post-write privacy checks find no private repository names
   or private relationship edges in public output.
6. The final report contains exactly one verified result for each of the 36
   organizations.

## Publication sequence

1. Merge the validated relationship publisher to `main`.
2. Post the exact bounded trigger on the designated operations issue.
3. Read the public key and nonce from the newly generated challenge comment.
4. Encrypt the owner credential locally for that nonce and post only the
   workflow-defined ciphertext response marker.
5. Confirm the workflow verifies all 36 organizations and publishes its
   sanitized completion report.
6. Independently sample the generated JSON, schema, Markdown, and profile
   discovery blocks in representative organizations.

## Failure handling

Treat a timeout, membership mismatch, privacy failure, incomplete report, or
unexpected organization count as a failed publication. Do not weaken the
allowlist, identity checks, privacy checks, or exact-result-count assertion to
force completion. Reconcile the underlying access or inventory problem and
run a new one-time challenge.
