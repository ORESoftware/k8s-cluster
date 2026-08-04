# Repository relocation to Fanwaave

The canonical home for this service is:

```text
https://github.com/fanwaave/push-notification-server.rs
```

The repository must move by direct GitHub ownership transfer from
`ORESoftware/push-notification-server.rs`. Do not replace the transfer with a
fork, archive-and-copy operation, or a new repository initialized from a source
snapshot: those approaches lose or split repository identity and operational
history.

## Transfer sequence

1. Merge the relocation-preparation pull request after validating its final
   head, including any automated `Cargo.lock` normalization commit.
2. Confirm `fanwaave/push-notification-server.rs` is still unused.
3. Transfer `ORESoftware/push-notification-server.rs` to the `fanwaave`
   organization through GitHub repository settings or the repository-transfer
   API.
4. Confirm the transferred repository retains `main`, tags, releases, issues,
   pull requests, Actions history, environments, webhooks, deploy keys, and
   security configuration.
5. Confirm the GitHub app installation for `fanwaave` can read and write the
   transferred repository.
6. Review organization-level rulesets, branch protection, required checks,
   Actions permissions, secrets, variables, package permissions, and
   environment approvals. Organization inheritance can change after a
   transfer even when repository data is preserved.
7. Update deployment manifests, submodules, package metadata, status badges,
   service catalogs, Linear links, and downstream clones to the Fanwaave URL.
8. Verify the former ORESoftware URL redirects to the Fanwaave repository.

## Existing clones

Update the canonical remote after the transfer:

```bash
git remote set-url origin https://github.com/fanwaave/push-notification-server.rs.git
```

SSH users should use:

```bash
git remote set-url origin git@github.com:fanwaave/push-notification-server.rs.git
```

## Redirect safety

Do not create another repository named `push-notification-server.rs` under
`ORESoftware` after the transfer. Reusing the old path can break GitHub's
redirect and split callers between two unrelated repositories.

## Completion criteria

The relocation is complete only when the repository is owned by `fanwaave`,
the default branch is healthy, required CI passes under the destination
organization, production deployment references use the new URL, and no active
consumer depends on the former owner path except through GitHub's redirect.
