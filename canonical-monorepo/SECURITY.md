# Security policy

## Scope

`canonical-monorepo` is a git superproject: it stores submodule pins and shared
config, not application source. Vulnerabilities in an app belong in that app's
own repo (`apps/<app>` → its upstream). Issues in the superproject itself
(CI, scripts, submodule wiring, leaked secrets) belong here.

## Reporting

Please report suspected vulnerabilities privately — do **not** open a public
issue for anything exploitable. Use GitHub's private
["Report a vulnerability"](https://github.com/canonical-cloud/canonical-monorepo/security/advisories/new)
flow, or email `security@canonical.cloud`. Include the affected repo/commit and
a minimal reproduction. Expect an acknowledgement within a few business days.

## Handling secrets

- Never commit real secrets. Only `.env.example` (placeholder values) is tracked;
  everything matching `.env*` is gitignored.
- `scripts/audit-repo-state.sh` scans the superproject and every submodule for
  conflict markers and tracked secret-looking values (private keys, GitHub PATs,
  AWS keys). CI runs it as a blocking gate on every push and PR.
- If a secret is committed anywhere, treat it as compromised: rotate it first,
  then scrub history.
