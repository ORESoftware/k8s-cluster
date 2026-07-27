# ClipTown release and distribution policy

This policy defines how ClipTown moves from CI artifacts to public distribution. It is intentionally fail-closed: an unverified channel remains blocked and exposes no public download URL.

The machine-readable status for every channel is in `release/channels.json` and is enforced by `scripts/validate-release-policy.py`.

## Versioning

ClipTown release tags use semantic versioning in the form `vMAJOR.MINOR.PATCH`.

A release candidate must identify the exact standalone-repository commits and the exact monorepo gitlinks used to build it. A moving branch name, `HEAD`, a `latest` URL, or an unpinned submodule is not a release input.

## Channel states

| State | Meaning |
| --- | --- |
| `blocked` | Prerequisites are incomplete. No public download URL is allowed. |
| `dry-run` | CI may build and retain private workflow artifacts, but publication remains disabled. |
| `verified` | The immutable public artifact and channel-specific evidence have been reviewed. |

Changing a channel to `verified` requires a dedicated PR containing the immutable URL and the evidence named in the channel record.

## Required release evidence

Every downloadable artifact must have:

- an immutable semantic-version tag;
- a release build from a locked dependency graph;
- a SHA-256 checksum;
- an SBOM covering the shipped artifact;
- a provenance attestation tied to the source commit and workflow;
- documented signing or an explicit decision that the channel cannot yet be public;
- permission and data-handling disclosures matching the shipped behavior.

Checksums alone do not substitute for platform signing. Platform signing alone does not substitute for checksums, provenance, or an SBOM.

## CLI dry runs

The CLI release workflow supports manual dispatch. A manual run builds locked Linux, macOS, and Windows archives and uploads the archives and SHA-256 files as GitHub Actions artifacts.

The GitHub Release attachment step is tag-gated. A manual dispatch therefore remains a dry run and must not create a public release.

Before a CLI channel becomes verified, the release workflow must also produce an SBOM and provenance attestation, and the platform-signing decision must be documented. The Homebrew formula remains absent until the referenced CLI archives exist at immutable URLs with reviewed checksums.

## Desktop and mobile channels

Desktop and mobile store publication remains blocked until the relevant signing identity, permission disclosure, privacy metadata, and review track have been exercised.

- macOS requires Developer ID signing, hardened runtime, notarization, and stapling verification.
- Windows requires an Authenticode and installer strategy with signature verification.
- Linux requires a reviewed package-format decision, reproducible build evidence, checksums, and an SBOM.
- iOS requires distribution signing, privacy metadata, store metadata, and a review track.
- Android requires an app-signing policy, Data safety metadata, store metadata, and an internal test track.

Signing credentials are production secrets. They must not be prerequisites for ordinary pull-request validation and must not be committed to a repository.

## Browser channel

The browser extension remains blocked from store publication while persistent encrypted sync is intentionally disabled. A future store submission must include the reviewed package, exact permission disclosure, privacy policy, and review artifacts.

The store description must not claim encryption, persistence, or synchronization behavior that is not present in the reviewed extension commit.

## Support destination

`PUBLIC_PATREON_URL` remains unset until the official ClipTown support destination is verified. An unverified funding URL must not be displayed as the long-term production default.

Verification requires a dedicated PR that records the official destination, enables link checking, and changes the machine-readable support state to `verified`.

## Production publication checklist

A production publication PR must identify:

1. the semantic version and immutable source commits;
2. all required CI and platform-test results;
3. archive names, checksums, SBOMs, and provenance files;
4. signing and verification evidence for each target platform;
5. permission, privacy, and store metadata;
6. rollback or withdrawal procedures;
7. the verified public URLs that will be enabled after publication.

The publication action itself remains a separate protected step after review. This policy PR does not create tags, releases, packages, store submissions, or support destinations.
