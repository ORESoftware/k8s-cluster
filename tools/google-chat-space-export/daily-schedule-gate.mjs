#!/usr/bin/env node

import { promises as fs } from 'node:fs';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

const SCHEDULE_BY_OFFSET = new Map([
  ['-0500', '0 1 * * *'],
  ['-0600', '0 2 * * *'],
]);

export function chicagoUtcOffset(now = new Date()) {
  const offsetName = new Intl.DateTimeFormat('en-US', {
    timeZone: 'America/Chicago',
    timeZoneName: 'longOffset',
  })
    .formatToParts(now)
    .find((part) => part.type === 'timeZoneName')?.value;
  const match = /^GMT([+-])(\d{2}):(\d{2})$/.exec(offsetName || '');
  if (!match) throw new Error(`Could not resolve America/Chicago UTC offset: ${offsetName}`);
  return `${match[1]}${match[2]}${match[3]}`;
}

export function shouldRunDailyReconciliation({ eventName, eventSchedule, utcOffset }) {
  if (eventName !== 'schedule') return true;
  if (![...SCHEDULE_BY_OFFSET.values()].includes(eventSchedule)) {
    throw new Error(`Unrecognized scheduled reconciliation expression: ${eventSchedule}`);
  }
  const expectedSchedule = SCHEDULE_BY_OFFSET.get(utcOffset);
  if (!expectedSchedule) {
    throw new Error(`Unsupported America/Chicago UTC offset: ${utcOffset}`);
  }
  return eventSchedule === expectedSchedule;
}

export async function main() {
  const eventName = process.env.GITHUB_EVENT_NAME || '';
  const eventSchedule = process.env.GITHUB_EVENT_SCHEDULE || '';
  const utcOffset = chicagoUtcOffset();
  const shouldRun = shouldRunDailyReconciliation({ eventName, eventSchedule, utcOffset });
  const output = `should-run=${shouldRun}\n`;
  if (!process.env.GITHUB_OUTPUT) throw new Error('GITHUB_OUTPUT is required');
  await fs.appendFile(process.env.GITHUB_OUTPUT, output, 'utf8');
  if (process.env.GITHUB_STEP_SUMMARY) {
    await fs.appendFile(
      process.env.GITHUB_STEP_SUMMARY,
      `Daily reconciliation gate: event=${eventName} schedule=${eventSchedule || 'n/a'} offset=${utcOffset} run=${shouldRun}\n`,
      'utf8',
    );
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  main().catch((error) => {
    process.stderr.write(`daily reconciliation schedule gate failed: ${error.message}\n`);
    process.exitCode = 1;
  });
}
