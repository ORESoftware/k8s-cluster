# Repository relocation to Fanwaave

The canonical home for this service is:

```text
https://github.com/fanwaave/push-notification-server.rs
```

The transfer source is:

```text
https://github.com/ORESoftware/push-notification-server.rs
```

Move the repository by direct GitHub ownership transfer. Do not replace the transfer with a fork, archive-and-copy operation, or newly initialized repository: those approaches split repository identity and can lose issues, pull requests, releases, stars, redirects, and other history.

The immutable GitHub repository ID observed before transfer is `1314172992`. Verify the same ID after transfer.

## Preconditions

- The metadata migration from PR #31 is present on `main`.
- `fanwaave/push-notification-server.rs` does not already exist.
- The operator has repository administration permission on the source and permission to create repositories in `fanwaave`.
- Required branch-protection, ruleset, Actions, environment, secret, webhook, deploy-key, package, and deployment settings have been inventoried.
- No automation is about to recreate the legacy repository path.

## Transfer

Use GitHub repository settings, or an authenticated GitHub CLI session with administrative access:

```bash
gh api --method POST \
  repos/ORESoftware/push-notification-server.rs/transfer \
  -f new_owner=fanwaave
```

GitHub processes the transfer asynchronously. Do not create `ORESoftware/push-notification-server.rs` again while the redirect is required.

## Verification

Confirm ownership and immutable identity:

```bash
gh api repos/fanwaave/push-notification-server.rs \
  --jq '{full_name, id, default_branch, archived, disabled}'
```

Expected repository ID:

```text
1314172992
```

Then verify:

1. `main`, tags, releases, issues, pull requests, discussions, wiki content, and Actions history are present.
2. The connected GitHub app can read and write the repository under `fanwaave`.
3. Destination organization rulesets and required checks apply as intended.
4. Repository and environment secrets, variables, approvals, webhooks, deploy keys, and package permissions are usable.
5. CI runs successfully from the destination namespace.
6. Production deployment references, service catalogs, submodules, manifests, badges, and documentation use the Fanwaave URL.
7. The legacy URL redirects to the Fanwaave repository.
8. The authoritative project registries resolve `fanwaave/push-notification-server.rs` through the Fanwaave owner context.

## Existing clones

HTTPS:

```bash
git remote set-url origin https://github.com/fanwaave/push-notification-server.rs.git
```

SSH:

```bash
git remote set-url origin git@github.com:fanwaave/push-notification-server.rs.git
```

Fetch and verify after changing the remote:

```bash
git fetch --prune origin
git remote -v
git rev-parse origin/main
```

## Redirect safety

Do not create another repository named `push-notification-server.rs` under `ORESoftware` after the transfer. Reusing the former path can break GitHub's redirect and split callers between unrelated repositories.

## Completion criteria

The relocation is complete only when the repository is owned by `fanwaave`, still has repository ID `1314172992`, required CI passes under the destination organization, production references use the new URL, project registries resolve the new owner, and no active consumer depends on the legacy path except through GitHub's redirect.
