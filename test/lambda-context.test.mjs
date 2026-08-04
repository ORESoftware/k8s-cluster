import assert from 'node:assert/strict';
import test from 'node:test';

import {
  createLambdaContext,
  lambdaContextDefaults,
} from '../child-runtimes/lambda-context.mjs';

function build(overrides = {}) {
  let now = 10_000;
  const context = createLambdaContext({
    definition: {
      id: 'function-id',
      slug: 'image-resizer',
      runtime: 'gleam',
      maxRunMs: 45_000,
      revisionId: 'revision-id',
      revisionNumber: 7,
      releaseMode: 'alias',
      alias: 'production',
      routingVersion: 12,
      env: { ASSET_BUCKET: 'images', RETRIES: '3' },
      metaData: {
        runtimeConfig: {
          memoryMb: 1_024,
          cpuMillis: 1_500,
          ephemeralStorageMb: 2_048,
          maxConcurrency: 25,
          provisionedConcurrency: 5,
          architecture: 'arm64',
        },
      },
      ...overrides.definition,
    },
    envelope: {
      invocationId: 'request-123',
      deadlineMs: 50_000,
      ...overrides.envelope,
    },
    browserAutomation: false,
    actorSession: null,
    dispatchContainerPool: async () => ({ ok: true }),
    clock: () => now,
  });
  return {
    context,
    advance: (milliseconds) => {
      now += milliseconds;
    },
  };
}

test('exposes AWS Lambda and Cloud Functions compatible identity fields', () => {
  const { context } = build();
  assert.equal(context.invocationId, 'request-123');
  assert.equal(context.requestId, 'request-123');
  assert.equal(context.awsRequestId, 'request-123');
  assert.equal(context.eventId, 'request-123');
  assert.equal(context.functionName, 'image-resizer');
  assert.equal(context.functionVersion, '7');
  assert.equal(
    context.invokedFunctionArn,
    'scintilla:function:image-resizer:7',
  );
  assert.equal(context.release.alias, 'production');
  assert.equal(context.release.routingVersion, 12);
});

test('exposes resource and concurrency configuration from revisioned metadata', () => {
  const { context } = build();
  assert.deepEqual(context.configuration, {
    memoryMb: 1_024,
    cpuMillis: 1_500,
    ephemeralStorageMb: 2_048,
    maxConcurrency: 25,
    provisionedConcurrency: 5,
    architecture: 'arm64',
  });
  assert.equal(context.memoryLimitInMB, 1_024);
  assert.equal(context.architecture, 'arm64');
  assert.equal(context.capabilities.reservedConcurrency, true);
  assert.equal(context.capabilities.provisionedConcurrency, true);
});

test('remaining time follows the supervisor deadline and never becomes negative', () => {
  const { context, advance } = build();
  assert.equal(context.getRemainingTimeInMillis(), 40_000);
  advance(15_000);
  assert.equal(context.getRemainingTimeInMillis(), 25_000);
  advance(30_000);
  assert.equal(context.getRemainingTimeInMillis(), 0);
});

test('environment is immutable and contains only string values', () => {
  const { context } = build({
    definition: {
      env: {
        VALID: 'yes',
        NUMBER: 42,
        OBJECT: { secret: true },
      },
    },
  });
  assert.deepEqual(context.environment, { VALID: 'yes' });
  assert.equal(context.env, context.environment);
  assert.equal(Object.isFrozen(context.environment), true);
  assert.throws(() => {
    context.environment.VALID = 'changed';
  }, TypeError);
});

test('latest functions receive safe runtime defaults', () => {
  const { context } = build({
    definition: {
      revisionId: null,
      revisionNumber: null,
      metaData: {},
      env: null,
    },
    envelope: { deadlineMs: null },
  });
  assert.equal(context.functionVersion, '$LATEST');
  assert.equal(context.configuration.memoryMb, lambdaContextDefaults.memoryMb);
  assert.equal(context.configuration.cpuMillis, lambdaContextDefaults.cpuMillis);
  assert.equal(
    context.configuration.ephemeralStorageMb,
    lambdaContextDefaults.ephemeralStorageMb,
  );
  assert.equal(context.configuration.architecture, 'x86_64');
  assert.equal(context.configuration.maxConcurrency, null);
  assert.equal(context.capabilities.reservedConcurrency, false);
});

test('provisioned concurrency is bounded by reserved concurrency at runtime', () => {
  const { context } = build({
    definition: {
      metaData: {
        runtimeConfig: {
          maxConcurrency: 2,
          provisionedConcurrency: 5,
        },
      },
    },
  });
  assert.equal(context.configuration.maxConcurrency, 2);
  assert.equal(context.configuration.provisionedConcurrency, 0);
});
