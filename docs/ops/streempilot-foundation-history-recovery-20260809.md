# StreemPilot foundation-history recovery — 2026-08-09

## Outcome

GitHub Actions run [`31297720582`](https://github.com/ORESoftware/k8s-cluster/actions/runs/31297720582) published and remotely verified eight non-destructive recovery branches and eight draft archival pull requests.

Recovery branch used in every target repository:

```text
recovery/streamyard-parity-foundation-20260809
```

Aggregate classification of 44 recovered file blobs against each repository's then-current `main`:

| Classification | Files |
|---|---:|
| Already integrated exactly | 8 |
| Absent from current `main` | 24 |
| Diverged or superseded | 12 |

The semantic rule was deliberate: current product files remained authoritative. Recovered content was not copied over newer paths and was stored only below:

```text
.recovery/streamyard-parity-foundation-20260809/
```

Each recovery directory includes exact recovered blobs, a binary-safe full-index patch, original commit metadata, blob hashes, and path-by-path review notes.

## Draft pull requests

| Repository | Branch commit | Draft PR |
|---|---|---|
| `StreemPilot/sp-api` | `d6fd35d3edf4a0995877197176b85a91de9e0ff1` | [#4](https://github.com/StreemPilot/sp-api/pull/4) |
| `StreemPilot/sp-cli` | `68e43c0f0cbef64426840abf71d224c4b745c563` | [#10](https://github.com/StreemPilot/sp-cli/pull/10) |
| `StreemPilot/sp-infra` | `182b97542337406c0606b3d28490d26aabb12c7d` | [#7](https://github.com/StreemPilot/sp-infra/pull/7) |
| `StreemPilot/sp-interfaces` | `b4a539bfd91a87867755799be0a5c6b2281fc3b1` | [#19](https://github.com/StreemPilot/sp-interfaces/pull/19) |
| `StreemPilot/sp-sync` | `4c3f3673b77ac68ef25401115f3daf2be2187309` | [#6](https://github.com/StreemPilot/sp-sync/pull/6) |
| `StreemPilot/sp-web-dioxus` | `b24e4620a26bb94e2a6edc485284a684e2bc52c7` | [#9](https://github.com/StreemPilot/sp-web-dioxus/pull/9) |
| `StreemPilot/sp-web-leptos` | `e2a9084dd03b2ff929a4986895676217394c6736` | [#9](https://github.com/StreemPilot/sp-web-leptos/pull/9) |
| `StreemPilot/sp-web-mash` | `79af90056397082577468200051b5e9860d50e8d` | [#15](https://github.com/StreemPilot/sp-web-mash/pull/15) |

## Safety properties

- No force push was used.
- No default branch was changed.
- Existing product paths were not modified.
- An existing recovery branch with a different tree would have caused a fail-closed stop.
- The owner credential was transferred only as one-time RSA-OAEP-SHA256 ciphertext, masked in the runner, and removed from the carrier comment after use.
- The runner recorded `privateKeyStored: false` and `plaintextCredentialStored: false` in the sanitized completion record.
- Carrier payload and encrypted-response comments were sanitized after publication.

The workflow's GitHub run conclusion is `failure` only because its low-privilege workflow token could not edit issue comments during the final cleanup step. Repository publication, branch/PR verification, and the sanitized completion record had already succeeded. Cleanup was then completed through the connected GitHub application.

## Review guidance

These PRs are archival and intentionally remain draft. Reviewers should not mechanically copy a `snapshot/` tree into the product root. Any useful invariant should be ported through a new, path-specific change based on current `main`, preserving the newer architecture and tests.
