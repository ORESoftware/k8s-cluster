import type { FastifyInstance } from "fastify";

export interface TelemetryHandle {
  shutdown(): Promise<void>;
}

/**
 * Install the global OpenTelemetry tracer provider (OTLP/HTTP -> in-cluster
 * collector), W3C propagator and AsyncLocalStorage context manager.
 */
export function initTelemetry(serviceName: string): TelemetryHandle;

/** Add SERVER-span hooks to the root Fastify instance (one span per request). */
export function instrumentFastify(
  fastify: FastifyInstance,
  opts?: { service?: string },
): void;

/** pino mixin: stamps `trace_id`/`span_id` of the active span onto each log line. */
export function loggerMixin(): Record<string, string>;
