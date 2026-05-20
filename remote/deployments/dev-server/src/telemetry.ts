import { randomBytes } from "node:crypto";

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

  try {
    const result = await work(span);
    span.setStatus({ code: "ok" });
    return result;
  } catch (err) {
    const error = err instanceof Error ? err : new Error(String(err));
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
    await fetch(otlpTraceUrl, {
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
