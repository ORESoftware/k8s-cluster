# DEN-3838: Elenkos Rykshaw Flutter bootstrap handoff

This review-only handoff carries the exact source archive for local commit
`e86e8ab84008deba79b2ae262b829f3dbe0d532b` while the GitHub App installation
for `elenkos-systems` is unavailable to this automation surface.

## Integrity

- Intended repository: `elenkos-systems/elenkos-rykshaw-flutter`
- Source commit: `e86e8ab84008deba79b2ae262b829f3dbe0d532b`
- Source tree: `1418c0aa71af055dedc603da5a55743ec73414ca`
- Archive SHA-256: `a2d27b7b2934a8f6925d148204a5e0b3433cc56876f47371869cfccdbff2a16a`
- Linear: DEN-3837 / DEN-3838

The archive was generated with `git archive`, so it excludes `.git`, ignored
runtime configuration, signing material, and decrypted environment files.

## What the validation workflow proves

The pull-request workflow verifies the archive digest, materializes the source,
installs Flutter 3.47.1 from an immutable setup-action commit, then runs the
repository's policy validator, Python platform-patcher test, Dart formatting,
Flutter analysis, Flutter tests, and a debug Android build. It uploads only
non-secret validation evidence and the debug APK digest.

## What this handoff does not authorize

It does not create or publish a store release, enroll testers, accept store
terms, activate real rewards, custody funds, transmit money, or merge itself.
The target repository still needs an approved GitHub App installation or an
authorized protected publisher. Store workflows remain manually gated and
fail closed with real rewards disabled.
