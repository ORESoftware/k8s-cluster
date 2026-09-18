# Repo-local ores-lint overrides. Never overwritten by the rollout script.
#
# COST WARNING: this repo has 91 crates. A full `sh .ores-lint/lint.sh` compiles
# all of them and is a coffee-break operation, not a pre-commit one. The npm
# prebuild/prepublishOnly hooks are warn-only so they cannot block, but if that
# latency is unwelcome locally, set ORES_LINT_SKIP_RUST=1 here and let CI carry
# the Rust half.
ORES_LINT_MAX_EXAMPLES=10

# Reconcilers legitimately index into slices they just bounds-checked, so the
# stricter library lints used elsewhere in the fleet are deliberately absent.
