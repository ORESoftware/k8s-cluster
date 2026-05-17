'use strict';

const { spawn } = require('node:child_process');
const path = require('node:path');

const { parseNonNegativeInteger } = require('./config');

const mainPath = path.join(__dirname, 'main.js');
const durationSeconds = Math.max(
  1,
  parseNonNegativeInteger('COMPARE_DURATION_SECONDS', 60, process.env),
);
const pauseSeconds = parseNonNegativeInteger('COMPARE_PAUSE_SECONDS', 5, process.env);

function parseAggregate(line) {
  if (!line.includes('lock-loadtest-node aggregate')) {
    return null;
  }

  const values = {};
  for (const part of line.trim().split(/\s+/)) {
    const [key, value] = part.split('=');
    if (key && value !== undefined) {
      values[key] = value;
    }
  }

  if (!values.backend || !values.released) {
    return null;
  }

  return {
    backend: values.backend,
    released: Number.parseInt(values.released, 10) || 0,
    failed: Number.parseInt(values.failed, 10) || 0,
    avgLatencyMs: Number.parseFloat(values.avg_latency_ms) || 0,
    maxLatencyMs: Number.parseFloat(values.max_latency_ms) || 0,
  };
}

function runBackend(backend) {
  return new Promise((resolve, reject) => {
    let lastAggregate = null;
    let stdoutBuffer = '';
    const child = spawn(process.execPath, [mainPath], {
      env: {
        ...process.env,
        LOCK_BACKEND: backend,
        TEST_DURATION_SECONDS: String(durationSeconds),
      },
      stdio: ['ignore', 'pipe', 'inherit'],
    });

    child.stdout.on('data', (chunk) => {
      const text = chunk.toString('utf8');
      process.stdout.write(text);
      stdoutBuffer += text;
      const lines = stdoutBuffer.split(/\r?\n/);
      stdoutBuffer = lines.pop() || '';
      for (const line of lines) {
        const aggregate = parseAggregate(line);
        if (aggregate) {
          lastAggregate = aggregate;
        }
      }
    });

    child.on('error', reject);
    child.on('exit', (code, signal) => {
      const aggregate = parseAggregate(stdoutBuffer);
      if (aggregate) {
        lastAggregate = aggregate;
      }
      if (code !== 0) {
        reject(new Error(`${backend} run exited with code=${code} signal=${signal}`));
        return;
      }
      if (!lastAggregate) {
        reject(new Error(`${backend} run did not emit aggregate stats`));
        return;
      }
      resolve({
        ...lastAggregate,
        throughputRps: lastAggregate.released / durationSeconds,
      });
    });
  });
}

function chooseWinner(results) {
  const [first, second] = results;
  if (Math.abs(first.throughputRps - second.throughputRps) >= 1) {
    return first.throughputRps > second.throughputRps ? first.backend : second.backend;
  }
  return first.avgLatencyMs <= second.avgLatencyMs ? first.backend : second.backend;
}

async function main() {
  const results = [];

  for (const backend of ['live-mutex', 'redis']) {
    console.log(`lock-loadtest-compare starting backend=${backend} duration_seconds=${durationSeconds}`);
    results.push(await runBackend(backend));
    if (pauseSeconds > 0 && backend !== 'redis') {
      await new Promise((resolve) => setTimeout(resolve, pauseSeconds * 1000));
    }
  }

  for (const result of results) {
    console.log(
      [
        'lock-loadtest-compare result',
        `backend=${result.backend}`,
        `throughput_rps=${result.throughputRps.toFixed(2)}`,
        `released=${result.released}`,
        `failed=${result.failed}`,
        `avg_latency_ms=${result.avgLatencyMs.toFixed(2)}`,
        `max_latency_ms=${result.maxLatencyMs.toFixed(2)}`,
      ].join(' '),
    );
  }

  console.log(`lock-loadtest-compare winner backend=${chooseWinner(results)}`);
}

main().catch((error) => {
  console.error(`lock-loadtest-compare fatal error=${error.stack || error.message || error}`);
  process.exit(1);
});
