import { randomBytes } from "node:crypto";

import {
  annotateError,
  snapshotRequestContext,
  type SerializedRequestContext,
} from "./request-context.js";
import { contextFetch } from "./wrapped-fetch.js";

type AttributeValue = string | number | boolean | undefined;
type SpanStatus = { code: "ok" | "error"; message?: string };

export type TelemetrySpan = {
  setAttribute(key: string, value: AttributeValue): void;
  recordException(err: Error): void;
  setStatus(status: SpanStatus): void;
};

const serviceName = process.env.OTEL_SERVICE_NAME ?? "dd-dev-server-api";
const otlpEndpoint = process.env.OTEL_EXPORTER_OTLP_ENDPOINT ?? null;
const otlpTraceUrl = otlpEndpoint
  ? `${otlpEndpoint.replace(/\/$/, "")}/v1/traces`
  : null;

let telemetryEnabled = false;

class ExplicitSpan implements TelemetrySpan {
  readonly traceId = randomBytes(16).toString("hex");
  readonly spanId = randomBytes(8).toString("hex");
  readonly startTimeUnixNano = unixNanoNow();
  readonly attributes = new Map<string, Exclude<AttributeValue, undefined>>();
  readonly exceptions: Error[] = [];
  status: SpanStatus = { code: "ok" };
  endTimeUnixNano = this.startTimeUnixNano;

  constructor(readonly name: string) {}

  setAttribute(key: string, value: AttributeValue): void {
    if (value === undefined) {
      return;
    }
    this.attributes.set(key, value);
  }

  recordException(err: Error): void {
    this.exceptions.push(err);
  }

  setStatus(status: SpanStatus): void {
    this.status = status;
  }

  end(): void {
    this.endTimeUnixNano = unixNanoNow();
  }
}

export function initTelemetry(): void {
  telemetryEnabled = Boolean(otlpTraceUrl);
}

export async function withSpan<T>(
  name: string,
  attributes: Record<string, AttributeValue>,
  work: (span: TelemetrySpan) => Promise<T>,
): Promise<T> {
  const span = new ExplicitSpan(name);
  for (const [key, value] of Object.entries(attributes)) {
    span.setAttribute(key, value);
  }
  // Seed span with whatever ALS request context is active at span
  // creation time. Snapshots again on success/failure below so any
  // late-bound fields (taskId/threadId set inside the handler after
  // withSpan started) are still captured.
  applyRequestContextAttributes(span, snapshotRequestContext());

  try {
    const result = await work(span);
    applyRequestContextAttributes(span, snapshotRequestContext());
    span.setStatus({ code: "ok" });
    return result;
  } catch (err) {
    applyRequestContextAttributes(span, snapshotRequestContext());
    const error = err instanceof Error ? err : new Error(String(err));
    annotateError(error);
    span.recordException(error);
    span.setStatus({ code: "error", message: error.message });
    throw err;
  } finally {
    span.end();
    if (telemetryEnabled) {
      void exportSpan(span);
    }
  }
}

// Exported for unit tests; also useful if you ever want to stamp
// request-context attributes onto a span from a custom code path.
export function applyRequestContextAttributes(
  span: TelemetrySpan,
  ctx: SerializedRequestContext | null,
): void {
  if (!ctx) return;
  span.setAttribute("dd.request.id", ctx.requestId);
  if (ctx.route) span.setAttribute("dd.request.route", ctx.route);
  if (ctx.method) span.setAttribute("dd.request.method", ctx.method);
  if (ctx.threadId) span.setAttribute("dd.request.thread_id", ctx.threadId);
  if (ctx.taskId) span.setAttribute("dd.request.task_id", ctx.taskId);
  if (ctx.userId) span.setAttribute("dd.request.user_id", ctx.userId);
  if (ctx.provider) span.setAttribute("dd.request.provider", ctx.provider);
  for (const [key, value] of Object.entries(ctx.extra)) {
    span.setAttribute(`dd.request.extra.${key}`, value);
  }
}

export async function shutdownTelemetry(): Promise<void> {
  telemetryEnabled = false;
}

function unixNanoNow(): bigint {
  return BigInt(Date.now()) * 1_000_000n;
}

function otlpValue(value: Exclude<AttributeValue, undefined>) {
  if (typeof value === "boolean") {
    return { boolValue: value };
  }
  if (typeof value === "number") {
    return Number.isInteger(value) ? { intValue: String(value) } : { doubleValue: value };
  }
  return { stringValue: value };
}

async function exportSpan(span: ExplicitSpan): Promise<void> {
  if (!otlpTraceUrl) {
    return;
  }

  const attributes = Array.from(span.attributes.entries()).map(([key, value]) => ({
    key,
    value: otlpValue(value),
  }));

  for (const exception of span.exceptions) {
    attributes.push({
      key: "exception.message",
      value: { stringValue: exception.message },
    });
    if (exception.stack) {
      attributes.push({
        key: "exception.stacktrace",
        value: { stringValue: exception.stack },
      });
    }
  }

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1500);

  try {
    await contextFetch(otlpTraceUrl, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        resourceSpans: [
          {
            resource: {
              attributes: [
                {
                  key: "service.name",
                  value: { stringValue: serviceName },
                },
                {
                  key: "service.namespace",
                  value: { stringValue: "remote-dev" },
                },
              ],
            },
            scopeSpans: [
              {
                scope: { name: serviceName },
                spans: [
                  {
                    traceId: span.traceId,
                    spanId: span.spanId,
                    name: span.name,
                    kind: 2,
                    startTimeUnixNano: span.startTimeUnixNano.toString(),
                    endTimeUnixNano: span.endTimeUnixNano.toString(),
                    attributes,
                    status: {
                      code: span.status.code === "error" ? 2 : 1,
                      message: span.status.message ?? "",
                    },
                  },
                ],
              },
            ],
          },
        ],
      }),
      signal: controller.signal,
    });
  } catch {
    // Best effort. Metrics and logs still cover the runtime if traces drop.
  } finally {
    clearTimeout(timeout);
  }
}
