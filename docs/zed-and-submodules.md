# Zed and Git-submodule interoperability

`zed-monorepo` is itself a dependency-free Zed package envelope and uses Git
submodules as its reviewed integration inventory. The two mechanisms follow a
strict single-owner rule.

## Ownership rules

- A repository may appear in `.zpkg.toml` or `.gitmodules`, never both.
- GitHub HTTPS, HTTP, SCP-style SSH, `ssh://`, and `git://` URLs are normalized
  before identities are compared.
- `zed-cli` and `zed-infra` are intentionally absent from both mechanisms.
  The CLI consumes package layers independently; infrastructure owns deployment
  state outside this source-composition repository.
- The retained `apps/zed-interfaces` gitlink is intentional because both Rust
  services currently use the sibling path dependency `../zed-interfaces`.
- Every retained gitlink is pinned to an exact reviewed commit. `branch = main`
  is update metadata only and does not weaken the checked-in pin.

## Commands

```sh
bash scripts/validate-zed-submodules.sh
bash scripts/zed-install-with-submodules.sh
python3 scripts/check-portfolio-inventory.py
```

The guarded installer validates, delegates to `zed install --git-submodules`,
and validates again. The committed lock contains only the dependency-free lock
format header; it must not claim resolved artifacts that a real resolver did
not produce.

When a submodule becomes Zed-owned, migrate it atomically: add the dependency,
remove its `.gitmodules` section and gitlink, update the inventory, then run both
validators. Do not leave a compatibility period with dual ownership.
