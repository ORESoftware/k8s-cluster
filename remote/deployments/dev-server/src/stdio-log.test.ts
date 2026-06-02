import { strict as assert } from 'node:assert';
import { EventEmitter } from 'node:events';
import { describe, it } from 'node:test';

import {
  installProcessLogBridge,
  structuredLogLine,
  type ProcessEventSource,
} from './stdio-log.js';

class MemorySink {
  chunks: string[] = [];

  write(chunk: string): boolean {
    this.chunks.push(chunk);
    return true;
  }
}

describe('stdio-log', () => {
  it('formats the shared dd.log.v1 envelope', () => {
    const line = structuredLogLine(
      {
        severityText: 'INFO',
        body: 'task queued',
        eventName: 'agent.task.queued',
        serviceName: 'dd-dev-server-api',
        serviceNamespace: 'remote-dev',
        scopeName: 'test-scope',
        attributes: {
          'dd.request.id': 'req-1',
          missing: undefined,
        },
      },
      () => 1234,
    );

    const parsed = JSON.parse(line) as Record<string, unknown>;
    assert.equal(parsed.schema, 'dd.log.v1');
    assert.equal(parsed.time_unix_nano, '1234000000');
    assert.equal(parsed.severity_text, 'INFO');
    assert.equal(parsed.severity_number, 9);
    assert.equal(parsed.body, 'task queued');
    assert.equal(parsed.resource_service_name, 'dd-dev-server-api');
    assert.equal(parsed.scope_name, 'test-scope');
    assert.deepEqual(parsed.attributes, { 'dd.request.id': 'req-1' });
  });

  it('bridges explicit process info and warning events without patching streams', () => {
    const processEvents = new EventEmitter();
    const stdout = new MemorySink();
    const stderr = new MemorySink();
    const uninstall = installProcessLogBridge({
      serviceName: 'dd-dev-server-api',
      processEvents: processEvents as unknown as ProcessEventSource,
      stdout,
      stderr,
      nowMs: () => 2000,
    });

    processEvents.emit('info', {
      message: 'worker ready',
      eventName: 'agent.worker.ready',
      attributes: { worker: 'one', nested: { ignored: true } },
    });
    processEvents.emit('warning', new Error('careful now'));

    assert.equal(stdout.chunks.length, 1);
    assert.equal(stderr.chunks.length, 1);

    const info = JSON.parse(stdout.chunks[0] ?? '{}') as Record<string, unknown>;
    assert.equal(info.schema, 'dd.log.v1');
    assert.equal(info.severity_text, 'INFO');
    assert.equal(info.event_name, 'agent.worker.ready');
    assert.equal(info.body, 'worker ready');
    assert.deepEqual(info.attributes, { worker: 'one' });

    const warning = JSON.parse(stderr.chunks[0] ?? '{}') as Record<string, unknown>;
    assert.equal(warning.schema, 'dd.log.v1');
    assert.equal(warning.severity_text, 'WARN');
    assert.equal(warning.event_name, 'node.process.warning');
    assert.equal(warning.body, 'careful now');

    uninstall();
    processEvents.emit('info', { message: 'after uninstall' });
    assert.equal(stdout.chunks.length, 1);
  });
});
