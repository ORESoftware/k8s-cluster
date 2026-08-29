# Current organization `.github` governance publisher bundle

This directory stores the reviewed, **non-secret** publisher bundle used for the bounded 71-organization governance rollout tracked by `ORESoftware/k8s-cluster#1222`.

- Archive SHA-256: `9d4eda8c53d5dd1000615cd1b2e0e10d54f3ef47a1d086c1fc87498fc154eefe`
- Base64 parts: `6` (`part-000.b64` through `part-005.b64`)
- Allowed archive files: `MANIFEST.sha256`, the publisher, and its focused test module
- Credential content: none

The workflow concatenates the parts in lexical order, removes line endings, decodes the archive, verifies the pinned digest and member allowlist, verifies the internal manifest, compiles the publisher, and runs the focused tests before it creates an ephemeral encrypted credential recipient.
