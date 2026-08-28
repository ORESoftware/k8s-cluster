# DEN-3843 sustainable-bike publication trigger

state: idle
target: ORESoftware/sustainable-bike
tracking: DEN-3843
request-id: den-3843-000000000000

This marker is inert on `main`. A same-repository owner PR from the exact branch
`agent/den-3843-publication-trigger` may change `state` to `publish` and set a
unique 12-hex request ID. The validated workflow completion dispatches the
trusted default-branch repository creator; no repository credential is exposed
to the trigger PR.
