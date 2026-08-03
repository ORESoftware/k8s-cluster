# Meta Agent repository publication with an ephemeral credential challenge

This is an emergency bootstrap path for the first real repository in the
`meta-agents-demo` organization. It exists because an empty organization cannot
receive source through an ordinary repository PR, while the connected GitHub
App currently exposes repository reads but not repository creation.

The owner credential is **never committed**, added to a GitHub Actions secret,
placed in a workflow input, written to an artifact, or copied into Linear. A
trusted `pull_request_target` workflow on `ORESoftware/k8s-cluster` generates an
ephemeral RSA key pair. Only the public key and a random nonce are posted to the
execution carrier. The operator replies with RSA-OAEP-SHA256 ciphertext. The
private key and decrypted value exist only in the runner's temporary directory
and process memory and are destroyed on exit.

## Trust boundary

The trusted workflow accepts only a draft, same-repository, ORESoftware-owned
one-commit carrier with one four-line non-executable marker. The marker pins the
target repository, the carrier's trusted `main` parent, the reviewed bundle
digest, and the protocol version. Current `main` must be identical to or ahead
of that parent; PR-controlled code is never checked out or executed.

Before generating an RSA key or requesting any owner credential, the broker
runs a source preflight with the ordinary workflow-scoped read token. The
preflight reconstructs the sealed source, verifies the bundle and publisher
digests, verifies the exact two-ref inventory, and proves `git bundle verify`
succeeds inside a repository context. A challenge is posted only after that
read-only preflight succeeds.

After decryption, the workflow verifies the credential identifies exactly
`ORESoftware` and has active admin membership in `meta-agents-demo` before any
repository mutation. It then creates the public repository idempotently, pushes
only the reviewed `main` and feature refs without force, verifies the live
metadata and SHAs, and opens the normal implementation PR.

## Immutable source reconstruction

A raw `git fetch <sha>` is not a reliable archival interface: an existing Git
object may be readable through GitHub while no longer being advertised as a
fetchable ref. The canonical verifier therefore reconstructs only the sealed
inputs through a commit/tree/blob API snapshot of the exact `SOURCE_SHA`.

The implementation uses a bounded non-recursive tree walk rather than one
recursive root-tree request:

1. read the exact Git commit object and require its returned SHA to match;
2. read the root tree and resolve the one `scripts` subtree;
3. resolve `critical-org-fleet`, then its `assets` subtree and exact publisher
   blob;
4. read the small `assets` tree and select only lexically ordered
   `meta.part*` blobs;
5. read each part with the workflow-scoped token and concatenate its decoded
   file bytes;
6. decode the resulting sealed text into the binary Git bundle;
7. read the exact publisher blob from the same bounded tree path;
8. retain the bundle and publisher SHA-256 checks before any mutation.

Every tree response is required to be complete. Missing, duplicate, wrong-type,
or truncated path entries fail closed. The decrypted owner credential is not
used to retrieve source; it remains reserved for exact identity and
organization authorization plus target publication.

The asset path has two base64 layers by design. GitHub's blob response base64
encodes each tracked file, and each `meta.part*` file contains a segment of the
base64-encoded Git bundle. The verifier first removes the GitHub transport layer
into one ordered byte stream, then decodes that stream into the binary bundle
whose pinned SHA-256 is verified. Unit tests construct a real two-ref Git
bundle, split it into multiple sealed parts, expose those parts through a fake
Git Database API, and require exact reconstruction.

Bundle verification must run inside an initialized repository. `git bundle
verify` consults repository state even for a self-contained bundle. The verifier
creates a temporary bare repository, verifies the bundle there, enumerates its
heads, and rejects any ref or SHA beyond the exact reviewed inventory.

## CI proof

The focused broker contract performs two complementary checks on every broker,
verifier, test, or runbook change:

- unit and structural tests for trust boundaries, bounded API traversal,
  two-layer decoding, truncated-tree rejection, exact-ref rejection, memory-only
  credentials, no-force behavior, target verification, and carrier cleanup;
- a live read-only source preflight using `${{ github.token }}` that reconstructs
  and verifies the actual DEN-1057 bundle and publisher.

This prevents another owner-credential cycle from being used merely to discover
a source retrieval or bundle verification defect.

## Rotation and audit

Because a credential was disclosed in chat, rotate the credential immediately
after the repository and exact refs are verified. The execution carrier,
challenge, encrypted response, workflow run, target repository metadata, exact
ref SHAs, implementation PR, and Linear issue comments form the audit trail.
The ciphertext is not reusable because each execution has a fresh private key
and nonce.

## Linear ownership

DEN-1057 owns the canonical `meta-agent-control-plane.rs` repository and its
implementation review. DEN-1058 owns the isolated repository-bootstrap
activation and cleanup. DEN-319 owns the broader missing-repository publication
system. None is complete until live GitHub reads prove the repository, default
branch, visibility, exact refs, tests, and review PR.
