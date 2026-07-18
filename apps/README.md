# apps

Git submodules — one per application/service repo, each pinned to an exact
commit (the superproject's gitlink). `.gitmodules` sets `branch = main` for
every entry so `scripts/pin-submodules.sh main` fast-forwards and re-pins.

**Never edit code in here.** Change the sibling top-level checkout of the same
repo, push it, then update the pin. Two apps exist only as submodules with no
sibling checkout: `fiducia-customer.rs` and `fiducia-marketing.web` — those are
edited in place (their submodule checkout is their working copy).
