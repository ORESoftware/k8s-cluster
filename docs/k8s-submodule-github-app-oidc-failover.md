# K8s submodule GitHub App bootstrap: AWS OIDC role recovery

Linear: DEN-1095, DEN-1537

The trusted-main GitHub App bootstrap reaches the protected administration host
through AWS Systems Manager. Its first post-merge run validated the selector and
the exact 32-repository allowlist, but AWS rejected the selected role with
`sts:AssumeRoleWithWebIdentity`. Because the earlier workflow used one
first-nonempty expression, an incompatible role could shadow another already
approved repository role.

## Bounded role selection

The bootstrap now tries at most five existing, approved role slots in this
order:

1. `K8S_SUBMODULE_BOOTSTRAP_ROLE_ARN` — an optional dedicated role for this
   trusted-main workflow;
2. `AWS_ROLE_TO_ASSUME`;
3. `REMOTE_DEV_AWS_ROLE_TO_ASSUME`;
4. `AWS_OIDC_ROLE_ARN`;
5. `AWS_ECR_ROLE_ARN`.

Each attempt:

- uses the same immutable `aws-actions/configure-aws-credentials` revision;
- runs only after every earlier configured slot failed or was absent;
- clears any current AWS credentials before assuming the next role;
- masks the AWS account ID;
- uses short-lived OIDC credentials only; and
- exposes only the non-secret slot name when an attempt succeeds.

The workflow fails before SSM or GitHub secret mutation unless one role succeeds.
No role ARN, AWS credential, App ID, private key, or token is written to logs or
artifacts.

## Trust requirement

A successful role must trust the exact protected workflow identity. The expected
GitHub OIDC subject is the main branch of this repository:

```text
repo:ORESoftware/k8s-cluster:ref:refs/heads/main
```

The role must also permit the narrowly required SSM operations for the protected
administration instance. The administration host, not the workflow runner,
retrieves candidate App material and validates the same App across the complete
allowlist before writing repository-level Actions secrets.

## Security boundaries retained

This recovery does not:

- use a personal access token for private submodule checkout;
- accept a credential through `workflow_dispatch` input;
- expose secrets to pull-request workflow code;
- weaken the exact repository allowlist;
- skip repository-restriction or `contents:read` checks; or
- authorize a benchmark when secret hydration did not complete.

Long-lived user credentials are not revoked or modified. The selector's
short-lived GitHub App installation tokens retain their existing cleanup and
revocation behavior after validation or use.

## Verification sequence

1. Merge the reviewed failover change through a pull request.
2. Inspect the trusted-main bootstrap run and record only the successful role
   slot, workflow run, commit, and generic outcome.
3. Require schema-v2 evidence for all exact allowlisted repositories.
4. Verify that `K8S_SUBMODULE_APP_ID` and
   `K8S_SUBMODULE_APP_PRIVATE_KEY` exist by name without reading their values.
5. Rerun the current Scintilla native amd64/arm64 benchmark.
6. Require both jobs to execute private checkouts, concrete runtime probes,
   TJSV parity, OCI builds, architecture checks, and cold-start measurements on
   the same immutable candidate commit.

If all approved role slots are rejected, leave the bootstrap and dependent
benchmark red. Correct the AWS trust policy through the owner-controlled
infrastructure path rather than introducing a PAT fallback.
