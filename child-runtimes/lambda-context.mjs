const DEFAULT_MEMORY_MB = 512;
const DEFAULT_CPU_MILLIS = 1_000;
const DEFAULT_EPHEMERAL_STORAGE_MB = 512;
const DEFAULT_MAX_RUN_MS = 30_000;
const MAX_MAX_RUN_MS = 300_000;

function integer(value, fallback, minimum, maximum) {
  const parsed = Number.parseInt(String(value ?? ''), 10);
  return Number.isSafeInteger(parsed) && parsed >= minimum && parsed <= maximum
    ? parsed
    : fallback;
}

function optionalPositiveInteger(value, maximum = 1_000) {
  if (value === null || value === undefined || value === '') {
    return null;
  }
  const parsed = Number.parseInt(String(value), 10);
  return Number.isSafeInteger(parsed) && parsed >= 1 && parsed <= maximum ? parsed : null;
}

function stringMap(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return Object.freeze({});
  }
  return Object.freeze(
    Object.fromEntries(
      Object.entries(value)
        .filter(([key, item]) => key.length > 0 && typeof item === 'string')
        .map(([key, item]) => [key, item]),
    ),
  );
}

function definitionConfiguration(definition) {
  const metadata =
    definition?.metaData && typeof definition.metaData === 'object'
      ? definition.metaData
      : {};
  const stored =
    metadata.runtimeConfig && typeof metadata.runtimeConfig === 'object'
      ? metadata.runtimeConfig
      : {};
  const maxConcurrency = optionalPositiveInteger(
    stored.maxConcurrency ?? definition?.maxConcurrency,
  );
  const provisionedConcurrency = integer(
    stored.provisionedConcurrency ?? definition?.provisionedConcurrency,
    0,
    0,
    maxConcurrency ?? 1_000,
  );
  return Object.freeze({
    memoryMb: integer(stored.memoryMb, DEFAULT_MEMORY_MB, 128, 10_240),
    cpuMillis: integer(stored.cpuMillis, DEFAULT_CPU_MILLIS, 100, 6_000),
    ephemeralStorageMb: integer(
      stored.ephemeralStorageMb,
      DEFAULT_EPHEMERAL_STORAGE_MB,
      512,
      10_240,
    ),
    maxConcurrency,
    provisionedConcurrency,
    architecture: stored.architecture === 'arm64' ? 'arm64' : 'x86_64',
  });
}

function invocationDeadline(definition, envelope, clock) {
  const supplied = Number(envelope?.deadlineMs);
  if (Number.isSafeInteger(supplied) && supplied >= clock()) {
    return supplied;
  }
  const maxRunMs = integer(
    definition?.maxRunMs,
    DEFAULT_MAX_RUN_MS,
    1_000,
    MAX_MAX_RUN_MS,
  );
  return clock() + maxRunMs;
}

/**
 * Build the function-visible execution context.
 *
 * Scintilla keeps its own stable names while exposing the familiar AWS Lambda
 * and Google Cloud Functions fields. Nested capability/configuration objects are
 * frozen so user code cannot mutate the definition observed by a later warm
 * invocation in the same process.
 */
export function createLambdaContext({
  definition,
  envelope,
  browserAutomation,
  actorSession,
  dispatchContainerPool,
  clock = Date.now,
}) {
  const invocationId = String(envelope?.invocationId || '');
  const functionName = String(definition?.slug || envelope?.slug || '');
  const revisionNumber = Number.isSafeInteger(definition?.revisionNumber)
    ? definition.revisionNumber
    : null;
  const functionVersion = revisionNumber === null ? '$LATEST' : String(revisionNumber);
  const configuration = definitionConfiguration(definition);
  const environment = stringMap(definition?.env ?? definition?.environment);
  const deadlineMs = invocationDeadline(definition, envelope, clock);
  const release = Object.freeze({
    mode: definition?.releaseMode || 'latest',
    alias: definition?.alias || null,
    revisionId: definition?.revisionId || null,
    revisionNumber,
    routingVersion: definition?.routingVersion || null,
    definitionDigest: definition?.definitionDigest || null,
  });
  const capabilities = Object.freeze({
    browserAutomation: Boolean(browserAutomation),
    browserEngines: browserAutomation
      ? Object.freeze(['playwright', 'puppeteer'])
      : Object.freeze([]),
    durableActor: actorSession !== null,
    immutableRevision: Boolean(definition?.revisionId),
    responseStreaming: true,
    asynchronousInvocation: true,
    reservedConcurrency: configuration.maxConcurrency !== null,
    provisionedConcurrency: configuration.provisionedConcurrency > 0,
  });

  return {
    id: definition?.id,
    invocationId,
    requestId: invocationId,
    awsRequestId: invocationId,
    eventId: invocationId,
    slug: functionName,
    functionName,
    functionVersion,
    invokedFunctionArn: `scintilla:function:${functionName}:${functionVersion}`,
    memoryLimitInMB: configuration.memoryMb,
    architecture: configuration.architecture,
    logGroupName: `/scintilla/functions/${functionName}`,
    logStreamName: `${functionVersion}/${invocationId}`,
    deadlineMs,
    getRemainingTimeInMillis: () => Math.max(0, deadlineMs - clock()),
    environment,
    env: environment,
    configuration,
    release,
    containerPool: Object.freeze({
      dispatch: dispatchContainerPool,
      request: dispatchContainerPool,
    }),
    capabilities,
    meta: {
      runtime: definition?.runtime,
      labels: definition?.labels,
      metaData: definition?.metaData,
      releaseMode: release.mode,
      revisionId: release.revisionId,
      revisionNumber: release.revisionNumber,
      alias: release.alias,
      ...(envelope?.meta || {}),
    },
  };
}

export const lambdaContextDefaults = Object.freeze({
  memoryMb: DEFAULT_MEMORY_MB,
  cpuMillis: DEFAULT_CPU_MILLIS,
  ephemeralStorageMb: DEFAULT_EPHEMERAL_STORAGE_MB,
  maxRunMs: DEFAULT_MAX_RUN_MS,
});
