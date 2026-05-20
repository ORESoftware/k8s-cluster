'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const {
  DEFAULT_LOCK_KEYS,
  distributeRate,
  loadConfig,
  parseLockBackend,
  parseLockKeys,
} = require('../src/config');

test('default config targets 1k requests per second across at least three workers and five keys', () => {
  const config = loadConfig({});

  assert.equal(config.lockBackend, 'live-mutex');
  assert.equal(config.brokerHost, 'dd-live-mutex.default.svc.cluster.local');
  assert.equal(config.brokerPort, 6970);
  assert.equal(config.redisHost, 'dd-redis-cache.default.svc.cluster.local');
  assert.equal(config.redisPort, 6379);
  assert.equal(config.requestsPerSecond, 1000);
  assert.equal(config.workerProcesses, 3);
  assert.equal(config.clientsPerWorker, 12);
  assert.deepEqual(config.lockKeys, DEFAULT_LOCK_KEYS);
  assert.equal(config.lockKeys.length, 5);
});

test('redis backend config targets the redis cache service', () => {
  const config = loadConfig({ LOCK_BACKEND: 'redis' });

  assert.equal(config.lockBackend, 'redis');
  assert.equal(config.redisHost, 'dd-redis-cache.default.svc.cluster.local');
  assert.equal(config.redisPort, 6379);
  assert.equal(config.redisDatabase, 0);
  assert.equal(config.redisLockPrefix, 'dd-locktest');
  assert.equal(config.redisRetryDelayMs, 1);
});

test('lock backend parsing only enables supported backends', () => {
  assert.equal(parseLockBackend('redis'), 'redis');
  assert.equal(parseLockBackend('live-mutex'), 'live-mutex');
  assert.equal(parseLockBackend('anything-else'), 'live-mutex');
});

test('worker process count is never configured below three', () => {
  assert.equal(loadConfig({ WORKER_PROCESSES: '1' }).workerProcesses, 3);
  assert.equal(loadConfig({ WORKER_PROCESSES: '2' }).workerProcesses, 3);
  assert.equal(loadConfig({ WORKER_PROCESSES: '4' }).workerProcesses, 4);
});

test('request rate is distributed across workers without losing requests', () => {
  assert.deepEqual(distributeRate(1000, 3), [334, 333, 333]);
  assert.equal(distributeRate(1000, 3).reduce((sum, rate) => sum + rate, 0), 1000);
});

test('lock key parsing trims blanks and removes duplicates', () => {
  assert.deepEqual(parseLockKeys('a, b, a,,c'), ['a', 'b', 'c']);
  assert.deepEqual(parseLockKeys(''), [...DEFAULT_LOCK_KEYS]);
});
