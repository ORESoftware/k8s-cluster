# Frontend reactive adoption policy

Status: accepted for bounded pilots

Decision date: 2026-08-23

Tracking: [ORESoftware/k8s-cluster#1401](https://github.com/ORESoftware/k8s-cluster/issues/1401), [DEN-3926](https://linear.app/denman/issue/DEN-3926/adopt-rxjs-rxdart-and-rxts-across-frontend-apps-using-post-2025-best)

## Decision

Native event and stream primitives remain the default for frontend code. Adopt a
ReactiveX dependency only when an application has a concrete multi-source
orchestration problem whose cancellation, ordering, retry, or backpressure contract
is clearer and more testable as a reactive pipeline.

For approved TypeScript pilots, use `rxjs` 7.8.2, the production `latest` release as
of this decision. RxJS 9 is available only through the prerelease `next` channel and
is not approved for fleet rollout. Revisit the version after a stable release and a
separate migration review.

For approved Dart and Flutter pilots, use `rxdart` 0.28.0 as extensions over Dart's
native `Stream` and `StreamController` APIs. RxDart is not a replacement stream type.

Do not adopt the npm package named `rxts`. The name in the source request was
ambiguous. The registry artifact is an unrelated, single-maintainer 0.x package whose
latest release is 0.4.0 from 2023-01-11. In this portfolio, “reactive TypeScript” means
TypeScript using the maintained `rxjs` package; it does not mean a separate RxTS
dependency.

Primary package evidence, retrieved 2026-08-23:

- [RxJS installation guide](https://rxjs.dev/guide/installation)
- [RxJS npm registry metadata](https://registry.npmjs.org/rxjs)
- [RxJS upstream repository](https://github.com/ReactiveX/rxjs)
- [RxDart package metadata](https://pub.dev/packages/rxdart)
- [RxDart API documentation](https://pub.dev/documentation/rxdart/latest/)
- [`rxts` npm registry metadata](https://registry.npmjs.org/rxts)

## Why adoption is bounded

Reactive libraries materially help when the domain has several concurrent event
sources and explicit temporal semantics. They also make ownership less obvious if
operators, multicasting, and retry are introduced without a lifecycle model. A
dependency-only rollout would therefore increase bundle size and leak risk without
improving correctness.

The decision boundary is:

| Prefer native primitives | Prefer reviewed Rx |
| --- | --- |
| One event source and one owner | Two or more sources must be merged, switched, zipped, or raced |
| One request with `AbortSignal` or one Dart `Future` | A newer request must cancel or supersede an older request |
| A component-local value or formal reducer | Time-windowing, debouncing, or bounded buffering is domain behavior |
| A short, linear async operation | Retry and backoff are stateful, observable product behavior |
| Framework state already owns teardown | A shared hot source has multiple consumers with different lifetimes |

Rx is prohibited for:

- hiding domain states inside an untyped operator chain;
- replacing an explicit reducer, state machine, or durable queue;
- wrapping one promise, future, DOM event, or Chrome event with no composition need;
- implicit retries of writes or other non-idempotent operations;
- unbounded replay, buffering, concurrency, or reconnect loops;
- process-global subjects used as an informal service locator;
- telemetry implemented by monkey-patching framework, timer, network, or logging APIs;
- importing from unstable internal package paths;
- introducing `rxts` or a prerelease RxJS release into production applications.

## Required state model

Every approved pipeline must name its states and transitions before operators are
selected. The minimum connection-oriented model is:

```text
idle -> connecting -> open -> degraded -> retry-wait -> connecting
  |         |          |          |            |
  +---------+----------+----------+------------+-> closed
```

Applications may add domain states, but they must not infer correctness from
subscription timing. Durable state, acknowledgement, authorization, and conflict
resolution remain in their existing formal reducers, stores, or protocol state
machines. A WebSocket, SSE source, notification channel, or subject is a wake or
presentation mechanism unless its protocol explicitly says otherwise.

## Ownership and teardown

Each pipeline has exactly one lifecycle owner. That owner creates the root
subscription and exposes an idempotent `close` or `dispose` operation. The owner must
tear down all child subscriptions, sockets, timers, controllers, isolates, workers,
and pending retry tasks.

TypeScript requirements:

- Return an explicit teardown function or owned `Subscription` from adapters.
- Bridge application cancellation through `AbortSignal` where requests already use
  it; abort and unsubscribe are separate cleanup steps unless the adapter proves they
  are coupled.
- Use `takeUntil` only with an owner-scoped close signal that completes. Do not use a
  global “destroy” subject.
- Use `finalize` for owned cleanup and test that it runs on completion, error,
  cancellation, and supersession.
- `share` or `shareReplay` requires a written hot/cold decision. `shareReplay` must
  have a finite buffer and a reviewed reset/ref-count policy; an immortal cached
  subscription is prohibited.

Dart and Flutter requirements:

- Store every `StreamSubscription` and await cancellation during `dispose`.
- Close every owned `StreamController` or RxDart `Subject` exactly once.
- Prefer operators on native `Stream`; use a subject only when a reviewed hot-source
  or latest-value contract is required.
- A widget may consume a stream, but service lifetime must not be inferred from widget
  rebuilds.
- Background isolates and operating-system jobs must use durable state and explicit
  wake contracts; an in-memory stream does not survive process termination.

## Cancellation, errors, and retry

Cancellation is a typed outcome, not an error to retry. Superseded search, route,
session, or request work must stop before its result can update UI state. Closing an
owner must prevent every later emission and reconnect.

Error handling is placed at a boundary that can make a product decision:

- decode and validation errors identify bounded metadata, never payload content;
- authorization and policy failures fail closed and do not reconnect indefinitely;
- retry is allowed only for classified transient failures and idempotent operations;
- retries use exponential backoff with bounded jitter, a maximum delay, and either a
  maximum attempt count or a durable external wake;
- successful readiness resets the retry attempt; socket construction alone does not;
- reconnect loops stop on logout, credential rotation, route teardown, disposal, or a
  terminal protocol response;
- fallback transports have one owner, so WebSocket and SSE cannot both remain active
  after a transition.

Production backoff uses real time. Tests inject a scheduler, clock, delay function, or
timer factory so retry, debounce, cancellation, and timeout behavior is deterministic
and completes without wall-clock sleeps.

## Ordering, concurrency, and backpressure

The chosen operator must match the declared domain rule:

| Domain rule | TypeScript example | Dart example |
| --- | --- | --- |
| New input supersedes old work | `switchMap` with abort teardown | `switchMap` over a cancellable adapter |
| Preserve every item in order | `concatMap` | `asyncExpand` or a reviewed concat operator |
| Ignore new work while one run owns the resource | `exhaustMap` | an explicit single-flight gate |
| Bounded parallel reads | `mergeMap` with explicit concurrency | an explicit bounded worker pool |
| Combine current independent state | `combineLatest` | `Rx.combineLatest*` |

No pipeline may use unbounded `mergeMap`, replay, collection, or buffering on an
untrusted or indefinite source. UI event coalescing must state whether the first,
latest, or all events in the window are retained. Write pipelines must preserve the
existing idempotency and acknowledgement rules.

## Observability

Instrumentation is explicit at the owned pipeline boundary. It may report:

- state transitions and the transition reason code;
- subscription and owner counts;
- retry attempt and selected delay bucket;
- queue depth, dropped/coalesced count, and bounded latency histograms;
- cancellation, completion, and terminal error categories;
- package version and pilot identifier.

It must not report message bodies, audio, OTPs, credentials, authorization headers,
URLs containing secrets, stable device identifiers, or raw exception payloads. Logs
and metrics must remain bounded. A pilot must prove that closing the owner returns its
active-subscription and active-timer gauges to zero.

## Pilot gate

The rollout begins with reference implementations and two application/runtime
pilots. A dependency is not added elsewhere until these rows have review evidence.

| Pilot | Initial decision | Required evidence |
| --- | --- | --- |
| `opto-sync/opto-sync-clients` TypeScript web and service-worker surfaces | Accept as the RxJS reference implementation; merged PRs [#23](https://github.com/opto-sync/opto-sync-clients/pull/23) and [#24](https://github.com/opto-sync/opto-sync-clients/pull/24) already establish HTTP-authoritative reactive orchestration | Add/confirm deterministic scheduler and finalizer tests, bounded bundle measurement, subscription/timer zero-after-close evidence, and redacted transition telemetry |
| `sonus-auris/sonus-auris-ui.dart` presence and device-event lifecycle | Accept as the Flutter application pilot because it already has RxDart and many owned WebSocket, device, and audio streams | Consolidate only a lifecycle boundary with explicit ownership; prove cancellation, no reconnect after dispose, deterministic backoff, zero live timers/subscriptions, and no recording/privacy regression |
| `messaging-intel/msgint-chrome-extension-app` consent lifecycle | Reject dependency insertion at inventory time; one Chrome storage source already has an explicit owner and teardown | Keep native events. Reconsider only if multiple independent sources require temporal composition; retain this rejection as evidence that the rollout is not fleet-wide |

The old standalone `fiducia-customer-ui.web` is not a pilot even though it contains
WebSocket/SSE fallback logic: its repository is deprecated and forbids new feature
work. Any future Fiducia reactive decision belongs in the canonical Rust MASH/HTMX
application boundary.

## Pilot measurements

Each accepting pilot PR records a before/after table in its description or checked-in
evidence file:

| Measurement | Required method |
| --- | --- |
| Production bundle | Same clean production build and compression command before and after; report raw and gzip bytes for changed entry points |
| Startup | At least 20 comparable local or CI samples; report median and p95, environment, and command |
| Lifecycle | Deterministic test showing close/dispose prevents emissions/reconnects and returns owned subscription/timer counts to zero |
| Cancellation | Test a superseded or aborted operation and assert the stale result cannot mutate the state machine |
| Scheduler | Virtual/fake time test for retry/backoff/debounce/timeout transitions |
| Observability | Test bounded transition metadata and explicit absence of domain payloads and secrets |

Bundle or startup regression is not automatically rejected, but it must be explained
by a demonstrated correctness gain. A pilot with no demonstrated gain removes the
dependency and records the negative result.

## Rollout gate

There is no fleet matrix until both accepting pilots are reviewed. After review, a
candidate repository must document:

1. the concrete multi-source or temporal problem;
2. the owner and terminal teardown path;
3. named state transitions and the durable-state boundary;
4. cancellation, retry, ordering, and backpressure semantics;
5. deterministic tests and bundle/startup evidence;
6. redacted, bounded observability;
7. why native streams/events or the framework state model are insufficient.

If any item is missing, keep the native implementation. Approval applies to the
reviewed boundary, not automatically to every frontend in the same organization.
