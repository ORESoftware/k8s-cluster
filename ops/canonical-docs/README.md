# Canonical Docs publication carrier

This directory contains the exact, credential-free source archive reviewed for
DEN-1049. `manifest.json` binds the archive bytes, safe file inventory,
deterministic bootstrap and feature commits, trees, and business-plan digest.

The carrier is published only from trusted `ORESoftware/k8s-cluster` `main` by
`ops-publish-canonical-docs-20260804.yml`. The protected SSM host uses its
existing GitHub CLI OAuth profile. The publisher rejects classic and
fine-grained personal access tokens, never prints credentials, creates the
public repository empty, pushes the two exact deterministic refs without
force, waits for the source repository's documentation workflow, and merges
only the exact reviewed feature head.

No repository-administration credential is stored in this repository, GitHub
Actions, workflow inputs, logs, issues, or Linear.
