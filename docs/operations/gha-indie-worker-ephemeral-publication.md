# Publishing the GHA continuity repositories with an ephemeral owner credential

This one-use broker publishes two public repositories in the `gha-indie-worker`
organization from one immutable `ORESoftware/k8s-cluster` source commit:

- `gha-clone-server.rs` from `remote/deployments/gha-clone-server-rs`;
- `gha-indie-worker.rs` from `remote/deployments/build-server-rs`.

The split preserves the established boundary: the clone server parses a bounded,
fail-closed workflow subset and coordinates runs; the indie worker executes only
operator-reviewed fixed profiles against immutable source revisions. Native
GitHub Actions semantics remain GitHub-hosted Actions and ARC.

## Credential boundary

The workflow is registered on trusted `main` and accepts only an exact draft,
same-repository, owner-created, one-commit carrier containing one seven-line
non-executable marker. It never checks out or executes carrier-controlled code.

For each execution, the runner creates a fresh 3072-bit RSA key pair and posts
only the public key plus a random nonce. The operator sends the already-disclosed
GitHub token as RSA-OAEP-SHA256/MGF1-SHA256 ciphertext. Plaintext exists only in
the runner process and temporary directory, is masked immediately, and is erased
on exit. The broker validates the token as `ORESoftware` with active organization
admin membership before any mutation.

## Publication guarantees

The broker reads an exact source commit through GitHub's commit/tree/blob APIs,
rejects truncated trees, submodules, symlinks, and nonstandard file modes, and
verifies every source Git blob identity before recreating it. Each target receives
a reviewed import commit containing all source bytes and executable modes plus
one `SOURCE_PROVENANCE.md` file. A one-file bootstrap commit exists only because
GitHub cannot create the first ref in a completely empty repository; the import
commit immediately removes that bootstrap file through a no-force fast-forward.

Publication is create-only and no-force. An existing `main` is accepted only when
it already equals the deterministic import commit; divergent history fails closed.
The workflow verifies ownership, public visibility, default branch, exact main SHA,
and the provenance file. It also configures topics and attempts review-protected
`main` branches, reporting any repository-plan or permission limitation explicitly.

The execution carrier is closed after successful live verification. Rotate the
credential immediately afterward because its plaintext was previously disclosed in
chat, even though the broker never commits or stores it.
