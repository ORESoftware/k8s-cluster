"""Contract-test fallback for generated NATS subject constants.

Production imports ``dd_nats_subject_defs`` from the generated source-of-truth
package under ``remote/libs/nats/subject-defs/generated/python``. A normal
pull-request checkout may not materialize that separate library gitlink, so the
focused ai-ml-pipeline contract job places this test-only module before ``src``
on ``PYTHONPATH``. Keep these values aligned with the generated schema and the
Kubernetes deployment; never package this shim into the runtime image.
"""

ML_DEAD_LETTER_SUBJECT = "dd.remote.ml.deadletter"
ML_FEATURES_SUBJECT = "dd.remote.ml.features"
RUNTIME_EVENTS_SUBJECT = "dd.remote.events"
TELEMETRY_MDP_SUBJECT = "dd.remote.telemetry.mdp"
TELEMETRY_RAW_QUEUE_GROUP = "dd-ai-ml-pipeline"
TELEMETRY_RAW_SUBJECT = "dd.remote.telemetry.raw"
