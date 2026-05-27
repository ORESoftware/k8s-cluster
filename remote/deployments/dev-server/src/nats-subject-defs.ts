// Mirror of the relevant constants from the source-of-truth
// `@dd/nats-subject-defs` (remote/libs/nats/subject-defs/schema/) for use
// inside dev-server's `rootDir`-restricted TypeScript build. The values
// here MUST match the generated lib; the
// `remote/tests/general/dev-server-nats-subject-defs.test.ts` test asserts
// equality so a schema rename surfaces as a CI failure here instead of a
// silent drift between producer and consumer subject strings.
//
// Add additional constants/formatters as dev-server starts publishing /
// subscribing to more subjects, and bump the test alongside.

/**
 * Generic runtime event bus. Every deployment publishes lifecycle, error,
 * telemetry-style events here. Default for NATS_EVENT_SUBJECT across the
 * codebase.
 */
export const RUNTIME_EVENTS_SUBJECT = 'dd.remote.events';
