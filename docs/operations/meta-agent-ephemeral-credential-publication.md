# Meta Agent repository publication with an ephemeral credential challenge

This is an emergency bootstrap path for the first real repository in the
`meta-agents-demo` organization. It exists because an empty organization cannot
receive source through an ordinary repository PR, while the connected GitHub
App currently exposes repository reads but not repository creation.

The owner credential is **never committed**, added to a GitHub Actions secret,
placed in a workflow input, written to an artifact, or copied into Linear. A
trusted `pull_request_target` workflow on `ORESoftware/k8s-cluster` generates an
ephemeral RSA key pair. Only the public key and a random nonce are posted to the
execution carrier. The operator replies with RSA-OAEP-SHA256 ciphertext. Both
the OAEP digest and mask-generation digest are fixed to SHA-256
(`MGF1-SHA256`) so local encryption and runner decryption cannot depend on
version-specific OpenSSL defaults. The private key and decrypted value exist
only in the runner's temporary directory and process memory and are destroyed
on exit.

Each challenge is single-use. Never repost an old ciphertext to a later carrier
or challenge: the earlier private key has been destroyed, and the nonce and
public key no longer identify the active run.

## Trust boundary

The trusted workflow accepts only a draft, same-repository, ORESoftware-owned
one-commit carrier with one four-line non-executable marker. The marker pins the
target repository, the carrier's trusted `main` parent, the reviewed bundle
digest, and the protocol version. Current `main` must be identical to or ahead
of that parent; PR-controlled code is never checked out or executed.

Before generating an RSA key or requesting any owner credential, the broker
runs a source preflight with the ordinary workflow-scoped read token. The
preflight reconstructs the exact recovered Git history from the sealed source,
verifies the bundle and publisher digests, verifies the exact two publishable
branch refs, and proves `git bundle verify` succeeds inside an initialized
source repository. A challenge is posted only after that read-only preflight
succeeds.

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
Git Database API, and require byte-exact reconstruction.

Bundle verification must run inside an initialized source repository. `git
bundle verify` consults repository state even for a self-contained bundle. The
verifier creates a temporary bare repository and verifies the bundle there.
It classifies only `refs/heads/*` entries as publishable branches and requires
them to equal the reviewed `main` and feature map exactly. A Git bundle may also
advertise the non-pushable pseudo-ref `HEAD`; it is accepted only when it points
to one of the reviewed branch SHAs. Tags, remote-tracking refs, additional
branches, unknown pseudo-refs, or a `HEAD` pointing outside the reviewed branch
SHAs fail closed. The publisher independently pushes only the two explicit
`refs/heads/*` entries and never pushes `HEAD`.

## Trusted helper pin

The broker does not execute helper code from the carrier branch. After carrier
validation it reads `verify_meta_agent_source_snapshot.py` from the exact
trusted-main SHA returned by GitHub, requires the reviewed Git blob SHA, decodes
the helper into the runner's private temporary directory, and compiles it before
execution. Source preflight output is limited to sanitized source SHA, tree SHA,
asset count, digests, publishable branch refs, allowed auxiliary refs, and
temporary output paths.

## CI proof

The focused broker contract performs two complementary checks on every broker,
verifier, diagnostic test, fixture test, or runbook change:

- unit and structural tests for trust boundaries, bounded API traversal,
  two-layer decoding, truncated-tree rejection, branch-ref drift, auxiliary-ref
  rejection, memory-only credentials, no-force behavior, stage ordering, target
  verification, and carrier cleanup;
- a live read-only source preflight using `${{ github.token }}` that reconstructs
  and verifies the actual DEN-1057 bundle and publisher.

This prevents another owner-credential cycle from being used merely to discover
a source retrieval or bundle verification defect.

## Failure classification

The broker reports bounded stage names rather than credential material or raw
provider responses:

- `source-helper`: the trusted helper was missing, had the wrong blob SHA, or did
  not compile;
- `source-preflight`: the exact commit/tree/blob snapshot, two base64 layers,
  bundle or publisher digest, initialized repository context, publishable branch
  inventory, or auxiliary-ref policy failed;
- `challenge-bootstrap`: ephemeral RSA key or challenge creation failed;
- `await-encrypted-response`: no valid newer owner-authored ciphertext response
  arrived for the active nonce;
- `decrypt-ciphertext`: RSA-OAEP/MGF1 decryption failed for the active key;
- `validate-owner-token-shape`: plaintext was empty, whitespace-bearing, or not
  a supported GitHub token shape;
- `validate-owner-identity`: the GitHub `/user` request failed or did not resolve
  to `ORESoftware`;
- `validate-owner-membership`: the organization membership request failed or
  was not `admin:active`;
- `create-and-push-exact-repository`: the sealed publisher failed before exact
  remote verification;
- `verify-live-repository`: target owner, visibility, default branch, or refs
  did not match the reviewed contract;
- `ensure-review-pull-request`: the ordinary feature-to-main PR could not be
  created or verified.

These stages preserve fail-closed operation while making retries actionable.
They do not log the token, GitHub response body, decrypted plaintext, private
key, or credential-bearing transport configuration.

## Rotation and audit

Because a credential was disclosed in chat, rotate the credential immediately
after the bounded bootstrap attempt. Rotation is required whether publication
succeeds or fails. The execution carrier, challenge, encrypted response,
workflow run, target repository metadata, exact ref SHAs, implementation PR,
and Linear issue comments form the audit trail. The ciphertext is not reusable
because each execution has a fresh private key and nonce.

## Linear ownership

DEN-1057 owns the canonical `meta-agent-control-plane.rs` repository and its
implementation review. DEN-1058 owns the isolated repository-bootstrap
activation and cleanup. DEN-319 owns the broader missing-repository publication
system. None is complete until live GitHub reads prove the repository, default
branch, visibility, exact refs, tests, and review PR.
