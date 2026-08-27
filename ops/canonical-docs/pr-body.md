## Summary

Publish the Canonical Cloud business plan and establish the public documentation repository baseline required by DEN-1049.

- preserve the substantive 1,200-line business plan;
- separate current evidence, management plans, customer-specific results, and independently delivered assurance outcomes;
- add claims, repository, security, contribution, and publication-review boundaries;
- record an intentional no-license-grant policy pending DEN-628;
- establish canonical lowercase `agents.md` instructions with minimal alternate pointers;
- add pinned, read-only documentation CI and dependency-free validation.

## Semantic publication history

The repository is created empty. Deterministic `main` commit `86fb7c44ac88f2f4e5f9ff314c50cac736f63789` contains only the bootstrap README and `.gitignore`. Feature head `07da928d1b80aeca10c8d29daa26a967be1748dd` adds the reviewed documentation baseline. This preserves a real review boundary rather than landing the business plan directly on the default branch.

Public visibility does not silently select an open-source license. `LICENSE-POLICY.md` records no license grant while DEN-628 remains unresolved.

## Exact-head validation

Reviewed head: `07da928d1b80aeca10c8d29daa26a967be1748dd`.

- source archive digest and bounded file inventory: passed;
- deterministic commit and tree reproduction: passed;
- documentation contract and 11 hermetic tests: passed;
- no-force publication and exact-head merge guards: passed;
- protected GitHub profile rejects personal access tokens.

Refs DEN-1049
Related: DEN-319, DEN-621, DEN-628, DEN-127
