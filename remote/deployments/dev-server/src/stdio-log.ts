type SeverityText = 'INFO' | 'WARN' | 'ERROR';

type AttributeValue = string | number | boolean | null;
type Attributes = Record<string, AttributeValue | undefined>;

type WritableSink = {
  write(chunk: string): unknown;
};

export type ProcessEventSource = {
  on(event: string, listener: (...args: unknown[]) => void): unknown;
  off(event: string, listener: (...args: unknown[]) => void): unknown;
};

type StructuredLogInput = {
  severityText: SeverityText;
  body: string;
  eventName: string;
  serviceName: string;
  serviceNamespace?: string;
  scopeName?: string;
  attributes?: Attributes;
  traceId?: string;
  spanId?: string;
};

export type ProcessLogBridgeOptions = {
  serviceName: string;
  serviceNamespace?: string;
  scopeName?: string;
  stdout?: WritableSink;
  stderr?: WritableSink;
  processEvents?: ProcessEventSource;
  nowMs?: () => number;
};

const severityNumbers: Record<SeverityText, number> = {
  INFO: 9,
  WARN: 13,
  ERROR: 17,
};

export function structuredLogLine(input: StructuredLogInput, nowMs = Date.now): string {
  const attributes = stripUndefined(input.attributes ?? {});
  return JSON.stringify({
    schema: 'dd.log.v1',
    time_unix_nano: String(BigInt(nowMs()) * 1_000_000n),
    severity_text: input.severityText,
    severity_number: severityNumbers[input.severityText],
    body: input.body,
    resource_service_name: input.serviceName,
    resource_service_namespace: input.serviceNamespace ?? 'remote-dev',
    scope_name: input.scopeName ?? input.serviceName,
    event_name: input.eventName,
    ...(input.traceId ? { trace_id: input.traceId } : {}),
    ...(input.spanId ? { span_id: input.spanId } : {}),
    ...(Object.keys(attributes).length ? { attributes } : {}),
  });
}

export function writeStructuredLog(
  sink: WritableSink,
  input: StructuredLogInput,
  nowMs = Date.now,
): void {
  sink.write(`${structuredLogLine(input, nowMs)}\n`);
}

export function installProcessLogBridge(options: ProcessLogBridgeOptions): () => void {
  const processEvents = options.processEvents ?? process;
  const stdout = options.stdout ?? process.stdout;
  const stderr = options.stderr ?? process.stderr;
  const serviceNamespace = options.serviceNamespace ?? 'remote-dev';
  const scopeName = options.scopeName ?? options.serviceName;
  const nowMs = options.nowMs ?? Date.now;

  const warningListener = (warning: unknown): void => {
    const error = warning instanceof Error ? warning : new Error(String(warning));
    writeStructuredLog(
      stderr,
      {
        severityText: 'WARN',
        body: error.message,
        eventName: 'node.process.warning',
        serviceName: options.serviceName,
        serviceNamespace,
        scopeName,
        attributes: {
          'exception.name': error.name,
          'exception.stacktrace': error.stack,
        },
      },
      nowMs,
    );
  };

  const infoListener = (payload: unknown): void => {
    const normalized = normalizeInfoPayload(payload);
    writeStructuredLog(
      stdout,
      {
        severityText: 'INFO',
        body: normalized.body,
        eventName: normalized.eventName,
        serviceName: options.serviceName,
        serviceNamespace,
        scopeName,
        attributes: normalized.attributes,
      },
      nowMs,
    );
  };

  processEvents.on('warning', warningListener);
  processEvents.on('info', infoListener);

  return () => {
    processEvents.off('warning', warningListener);
    processEvents.off('info', infoListener);
  };
}

function normalizeInfoPayload(payload: unknown): {
  body: string;
  eventName: string;
  attributes: Attributes;
} {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    return {
      body: String(payload),
      eventName: 'node.process.info',
      attributes: {},
    };
  }

  const record = payload as Record<string, unknown>;
  const body = stringField(record.body) ?? stringField(record.message) ?? stringField(record.text);
  const eventName = stringField(record.eventName) ?? stringField(record.event_name);
  return {
    body: body ?? 'process info',
    eventName: eventName ?? 'node.process.info',
    attributes: objectAttributes(record.attributes),
  };
}

function objectAttributes(value: unknown): Attributes {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: Attributes = {};
  for (const [key, item] of Object.entries(value)) {
    if (typeof item === 'string' || typeof item === 'number' || typeof item === 'boolean' || item === null) {
      out[key] = item;
    }
  }
  return out;
}

function stringField(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function stripUndefined(attributes: Attributes): Record<string, AttributeValue> {
  const out: Record<string, AttributeValue> = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (value !== undefined) {
      out[key] = value;
    }
  }
  return out;
}
