# DEN-3843 sustainable-bike publication trigger

state: publish
target: ORESoftware/sustainable-bike
tracking: DEN-3843
request-id: den-3843-e70575fe53c9

This is the single-use, exact-branch publication request. The validation workflow
contains no repository credential. Its successful completion is consumed by the
trusted default-branch `workflow_run` dispatcher, which starts the reviewed
AWS-backed repository creator.
