# fiducia-monorepo — task runner. Run `just` to see everything.
#
# Environment secrets live in env/enc/*.env.enc, encrypted with sops + age and
# committed to this repo. See env/README.md for the workflow.

import '.just/env.just'

# Show available recipes.
default:
    @just --list
