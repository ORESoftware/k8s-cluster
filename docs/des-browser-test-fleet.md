# DES browser test fleet

The `discrete-event-systems-test` organization owns black-box browser verification for the canonical `discrete-event-systems/des-web.rs` server.

## Repositories

- `des-web-playwright-e2e` verifies canonical pages, the catalog API, health/readiness, mounted-path links, 404 behavior, and browser-visible hardening headers.
- `des-web-puppeteer-e2e` independently verifies Chromium navigation, content, API health, mounted-path boundaries, and retained screenshot evidence.
- `.github` documents the test organization and its ownership boundary.

## Execution

GitHub Actions targets the authenticated public gateway at `https://98.90.186.114/des/`. The gateway credential is stored only as the `DES_GATEWAY_AUTH` Actions secret in each test repository.

Each test repository also contains a bounded `.github/workflows/gha-indie-worker.yml` workflow. That workflow contains only pinned setup actions and static Node/browser commands, so the `gha-indie-worker` planner can consume it at an exact merged commit SHA. Its default target is the cluster-local `dd-des-web.default.svc.cluster.local:8130` service, avoiding public gateway credentials.

Live indie-worker execution remains controlled by `BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED`; planning and immutable workflow publication are safe while execution is disabled.

## Publishing and tracking

`.github/workflows/ops-bootstrap-des-browser-test-fleet.yml` runs only from trusted `main`, uses AWS OIDC plus SSM, resolves GitHub and gateway credentials on the protected host, creates missing repositories, configures repository secrets, and creates or reuses GitHub Projects for both organizations.

Code changes in the generated test repositories are delivered through reviewable pull requests and squash merges. Linear mirrors the GitHub tracking under the Discrete Event Systems project. GitHub remains the source for code, CI evidence, and repository-specific follow-up issues.
