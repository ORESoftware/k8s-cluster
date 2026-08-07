# Repository copy to Fanwaave

The canonical product home for this service is:
# Repository relocation to Fanwaave

The canonical home for this service is:

```text
https://github.com/fanwaave/push-notification-server.rs
```

The source repository remains available at:

```text
https://github.com/ORESoftware/push-notification-server.rs
```

The approved migration is now an explicit repository copy rather than an ownership transfer. The source is not deleted, archived, force-pushed, or rewritten. The destination becomes an independently reviewable Fanwaave repository with copied branches and tags plus its own migration pull request.

The source GitHub repository ID is `1314172992`. The Fanwaave copy receives a new repository ID because it is a distinct repository.

## Automated publisher

The default branch contains `.github/workflows/copy-to-fanwaave.yml`. It is deliberately fail-closed and runs only when the `ORESoftware` owner posts this exact comment on issue #33:

```text
/copy-to-fanwaave
```

The workflow:

1. binds the exact source `main` SHA and verifies source repository ID `1314172992`;
2. starts GitHub CLI OAuth device authorization for the repository owner, without requiring or storing a PAT;
3. verifies that the authenticated account is `ORESoftware` and an active Fanwaave administrator;
4. creates public repository `fanwaave/push-notification-server.rs` when it is absent;
5. copies all source branches and tags without deleting source refs;
6. verifies destination `main` equals the bound source `main`;
7. creates branch `agent/establish-fanwaave-home` in the destination;
8. commits migration provenance and opens a Fanwaave pull request;
9. comments the destination repository ID, source SHA, and pull-request URL on issue #33.

The temporary OAuth token and GitHub CLI configuration are removed at workflow exit. Device authorization comments are also deleted after successful authorization.

## Copy guarantees

- No source branch or tag is deleted.
- No force push is used.
- An existing nonempty destination `main` must exactly match the bound source `main`, otherwise the run fails closed.
- Destination repository settings enable issues, squash merges, and merge commits; projects, wikis, and rebase merges are disabled.
- Source workflows, code, documentation, branches, and tags are copied through Git history.
- GitHub issues, historical pull requests, releases, discussions, stars, watchers, secrets, environments, webhooks, deploy keys, and Actions run history remain associated with the source unless migrated separately.

## Existing clones

After the copy is established, new product work should use Fanwaave as `origin` and may retain ORESoftware as a read-only `upstream`:

```bash
git remote rename origin upstream
git remote add origin https://github.com/fanwaave/push-notification-server.rs.git
git fetch --prune origin upstream
git remote -v
```

SSH users may use:

```bash
git remote add origin git@github.com:fanwaave/push-notification-server.rs.git
```

## Post-copy verification

1. Confirm `fanwaave/push-notification-server.rs` exists and is public.
2. Confirm destination `main` equals the source SHA recorded by the workflow.
3. Confirm source branches and tags are present in the destination.
4. Review and merge the destination migration pull request only after its CI passes.
5. Confirm the connected GitHub app can read and write the Fanwaave repository.
6. Review Fanwaave organization rulesets, branch protection, Actions permissions, secrets, variables, environments, webhooks, deploy keys, packages, and deployment approvals.
7. Update deployment manifests, service catalogs, submodules, badges, Linear links, and downstream clones to the Fanwaave URL.
8. Merge the prepared coordinator and project-registry routing pull requests after the destination repository is live.

## Completion criteria

The copy is complete when the Fanwaave repository exists, its copied `main` matches the bound source SHA, the destination migration pull request is green and merged, destination GitHub App access is verified, project registries resolve the Fanwaave owner, and active product development targets the Fanwaave repository.
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
