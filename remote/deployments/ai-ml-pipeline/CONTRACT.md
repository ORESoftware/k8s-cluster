# FastAPI/Pydantic contract migration

DEN-466 is being delivered in reviewable stages so the executable contract can be proven before the host-mounted EC2 deployment changes runtime dependencies.

## Implemented in this slice

- `src/dd_ai_ml_pipeline_api.py` provides a FastAPI application factory around the existing `PipelineApp` domain model.
- Pydantic v2 request and response models drive request parsing, response serialization, validation, and OpenAPI schemas.
- Stable operation IDs, bounded error models, explicit service authentication, and `x-dd-visibility`/`x-dd-auth` metadata are attached at route definition time.
- `/openapi.json` and `/api/docs.json` serve the same canonical fail-closed public contract.
- `/docs/api` and `/api/docs` render the public contract.
- `src/export_openapi.py` emits deterministic public and internal OpenAPI 3.1 artifacts without starting NATS, runtime-config registration, telemetry exporters, or other background work.
- The public artifact prunes internal paths, service-auth metadata, and internal-only schemas.
- CI proves runtime route/OpenAPI parity, deterministic bytes, local `$ref` resolution, public/internal separation, authentication, validation errors, and docs alias equality.
- Runtime response validation confirms `rewardEstimate` is a scalar confidence/reward value, while `transitionModel` remains the structured transition collection.

## Generated NATS dependency in contract-only CI

Production continues to import `dd_nats_subject_defs` from the generated source-of-truth package under `remote/libs/nats/subject-defs/generated/python`. A normal pull-request checkout may not materialize that separate library gitlink, so the focused contract workflow prepends `tests/shims` to `PYTHONPATH`. The shim contains only the six deployed NATS subject/queue defaults, is asserted by the tests, and is never copied into the runtime image.

## Local validation

```bash
cd remote/deployments/ai-ml-pipeline
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -r requirements-dev.txt
PYTHONPATH=tests/shims:src python -m pytest -q tests/test_api_contract.py
PYTHONPATH=tests/shims:src python src/export_openapi.py --check
```

To run the native adapter locally with the generated package available:

```bash
export SERVER_AUTH_SECRET='local-development-secret'
export NATS_URL='nats://127.0.0.1:4222'
PYTHONPATH=src python src/dd_ai_ml_pipeline_api.py
```

## Deployment boundary

This slice does **not** switch the current host-mounted EC2 manifest from `src/dd_ai_ml_pipeline.py`. The Kubernetes deployment uses a bare `python:3.12-slim` image and therefore cannot safely assume FastAPI/Uvicorn are present. Runtime cutover belongs in the next stage after the first-party image is built and loaded, startup/readiness behavior is exercised, and rollback to the stdlib handler is documented.

The legacy generated scanner remains in place until that runtime cutover and the fleet-wide SDK gate are proven. The new application factory and contract tests are the authoritative candidate contract, not yet a claim that the live pod has migrated.
