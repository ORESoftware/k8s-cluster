# Testing & CI follow-ups

Notes from the 2026-07-18 browser-test work (branch `tests/browser-e2e`): what
landed, what's still thin, and how to shore it up. The remediation notes for the
shared-libs repo live in `k8s-libs-and-shared-defs/docs/remediation-followups.md`.

## What landed on `tests/browser-e2e`

A hermetic browser E2E suite under `remote/tests/browser/` — 11 scenarios, each
run under **both Puppeteer and Playwright** (22 checks), green locally:

- `service-worker.test.mjs` — the `dd-browser-drafts` offline draft cache
  (`remote/libs/browser/service-worker.js`): save/load/delete + error paths over
  the worker's real postMessage protocol.
- `func-approx-ui.test.mjs` — the `dd-func-approx` UI
  (`remote/deployments/func-approx-rs/ui.html`): shell render, dd-data-viz
  config badge, client-side sample generators, custom-JSON validation.
- `harness.mjs` (static server + engine launchers + `pollUntil`), a fixture
  host page, and `.github/workflows/browser-e2e.yml` (a `puppeteer` job and a
  `playwright` job via a `BROWSER_ENGINES` matrix).

Everything serves the real in-repo asset from a throwaway `127.0.0.1` server —
no live deployment, no DB.

## Open — high value

### 1. Reconcile the swept test files on `dev-sync`
While these tests were being written, a concurrent session ran `git add -A` and
absorbed an **earlier, partial copy** of three files (`harness.mjs`,
`fixtures/sw-host.html`, `service-worker.test.mjs`) into `dev-sync` commit
`53928d35`. This branch (`tests/browser-e2e`) holds the complete, correct set
(updated harness with `pollUntil` + `BROWSER_ENGINES`, plus the func-approx
suite, README, and CI workflow).

**Shore up:** merge `tests/browser-e2e`, then drop the partial copies from the
`53928d35` line (the branch's `harness.mjs` is a backward-compatible superset,
so the merge itself is trivial). Don't maintain both.

### 2. The live UI smokes are deployment-coupled
`remote/tests/ui-puppeteer-smoke.mjs`, `ui-playwright-smoke.mjs`,
`ui-gateway-sweep.mjs`, and `ui-walkthrough-video.mjs` target a hardcoded EC2 IP
(`54.91.17.58`). They only pass when that box is up and reachable, so they
can't gate PRs and rot silently.

**Shore up:** split "live smoke" from "hermetic E2E". Keep the live smokes as a
manually-dispatched / scheduled workflow that's allowed to fail (or gate them on
a reachable `REMOTE_DEV_BASE_URL`), and grow the hermetic suite (item 3) as the
PR-blocking check.

### 3. Broaden hermetic coverage
Only the service worker and the func-approx UI are covered. High-value additions
that can be tested backend-free (serve the asset, drive with both engines, mock
any `fetch` at the network layer via `page.route` / `setRequestInterception`):
- The MASH server UIs rendered from maud (dd-music vote updates over WebSocket,
  apostille-services, patent-filing, public-data-server) — assert structure,
  htmx attributes, and any inline client JS.
- `web-home-rs` agents/tasks client JS (the `#send-chat` flow the live smokes
  assert against, but hermetic).
- The func-approx **fit** path end-to-end: intercept the `/fit` POST, return a
  canned result, and assert `renderResult()` populates chips / expression /
  metrics. (Today only the pre-fit surface is covered.)

### 4. Wire the workflow into required checks
`browser-e2e.yml` triggers on pushes to `dev`/`main` and on PRs touching the
tests or the covered assets. Once item 1 lands, add both matrix jobs
(`browser / puppeteer`, `browser / playwright`) to the branch-protection
required checks so a red engine blocks merge.

## Open — process / tooling

### 5. Concurrent-session git hygiene
`git add -A` from one session swept another session's in-progress files into an
unrelated commit (item 1). With multiple agents/sessions in one working tree
this will recur.

**Shore up:** stage explicit paths (`git add <paths>`) rather than `-A`/`.`, or
give each session its own `git worktree` so uncommitted files never overlap. A
pre-commit reminder that flags `add -A` when unrelated untracked files are
present would also help.

### 6. Submodule pre-push guard false-positives in fresh worktrees
The repo's pre-push guard verifies every submodule gitlink is fetchable from its
remote. In a worktree where submodules aren't initialised it can't resolve them
and blocks **every** gitlink — even ones identical to the push base and already
published.

**Safe override rule:** `DD_SKIP_SUBMODULE_PUSH_GUARD=1` is safe **only** when
the commit changes no gitlinks. Verify first:

```sh
git diff <base>..HEAD --raw | grep 160000   # empty => no gitlink changes => override is safe
```

**Shore up:** teach the guard to skip gitlinks unchanged versus the push base
(or `git submodule update --init` before verifying), so a docs/test-only commit
from a worktree doesn't trip it.

## How to run locally

```sh
cd remote/tests
pnpm install
pnpm exec playwright install chromium        # Puppeteer fetches its own on install
node --test browser/service-worker.test.mjs browser/func-approx-ui.test.mjs
# one engine only (what each CI matrix job does):
BROWSER_ENGINES=playwright node --test browser/*.test.mjs
```
