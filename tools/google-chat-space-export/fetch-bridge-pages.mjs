#!/usr/bin/env node

/**
 * Pulls every page from the Google Chat HTTP bridge into a directory of raw
 * page files that import-plan.mjs can read directly.
 *
 *   CHAT_BRIDGE_TOKEN=... node tools/google-chat-space-export/fetch-bridge-pages.mjs \
 *     --out ./private/google-chat-export
 *
 * This script deliberately does not filter by date. The bridge's own floor is
 * fixed and callers cannot narrow it, so windowing belongs to the planner:
 *
 *   node tools/google-chat-space-export/import-plan.mjs \
 *     --input ./private/google-chat-export --since 2026-06-05T04:00:00.000Z
 *
 * Rotate the bridge token after an import with rotateBridgeToken().
 */

import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';

const EXEC_URL =
  'https://script.google.com/macros/s/AKfycbzIMOO0eQ12WjgRvLmYAdn3zryB57Ush6uWfQWc-iHNvVu6X0ULbPfPv7WMaYdMp2Tq/exec';
const EXPECTED_SPACE = 'spaces/AAQAoHKdzvI';
const DEFAULT_PAGE_SIZE = 250;
const EMPTY_PAGE_RETRIES = 4;
const EMPTY_PAGE_RETRY_DELAY_MS = 2_000;
const BRIDGE_REQUEST_RETRIES = 5;
const BRIDGE_REQUEST_RETRY_DELAY_MS = 2_000;

function usage() {
  return `Usage:
  CHAT_BRIDGE_TOKEN=<token> node tools/google-chat-space-export/fetch-bridge-pages.mjs \\
    --out <directory> [--page-size <1-250>]

Writes page-00001.json, page-00002.json, … plus fetch-summary.json.
The token is read from CHAT_BRIDGE_TOKEN so it never appears in argv or a URL.
`;
}

function parseArgs(argv) {
  const options = { out: null, pageSize: DEFAULT_PAGE_SIZE, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith('--')) {
        throw new Error(`Missing value for ${arg}`);
      }
      return argv[index];
    };
    switch (arg) {
      case '--out':
        options.out = next();
        break;
      case '--page-size':
        options.pageSize = Number(next());
        break;
      case '--help':
      case '-h':
        options.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

/**
 * Apps Script runs the POST and then 302s to a googleusercontent.com echo URL
 * that only serves GET; re-POSTing there answers 405 with an HTML body. Follow
 * the Location explicitly as GET so retry behavior remains deterministic while
 * the token stays in the original POST body.
 */
async function postJsonOnce(body) {
  let response = await fetch(EXEC_URL, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
    redirect: 'manual',
  });
  if ([301, 302, 303, 307, 308].includes(response.status)) {
    const location = response.headers.get('location');
    if (!location) throw new Error(`Bridge redirect ${response.status} omitted Location.`);
    response = await fetch(location, { redirect: 'follow' });
  }
  const text = await response.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error(`Non-JSON bridge response (HTTP ${response.status}): ${text.slice(0, 200)}`);
  }
  // ContentService always answers 200, so the envelope carries the real status.
  if (!parsed.ok) throw new Error(`Bridge error: ${JSON.stringify(parsed.error ?? parsed)}`);
  return parsed;
}

async function postJson(body) {
  let lastError;
  for (let attempt = 1; attempt <= BRIDGE_REQUEST_RETRIES; attempt += 1) {
    try {
      return await postJsonOnce(body);
    } catch (error) {
      lastError = error;
      if (attempt < BRIDGE_REQUEST_RETRIES) {
        process.stderr.write(
          `Bridge ${body.action || 'request'} failed; retrying (${attempt}/${BRIDGE_REQUEST_RETRIES}).\n`,
        );
        await new Promise((resolve) =>
          setTimeout(resolve, attempt * BRIDGE_REQUEST_RETRY_DELAY_MS),
        );
      }
    }
  }
  throw lastError;
}

async function fetchMessagePage(body) {
  let page;
  for (let attempt = 1; attempt <= EMPTY_PAGE_RETRIES; attempt += 1) {
    page = await postJson(body);
    const messages = page.data?.messages ?? [];
    if (messages.length > 0 || page.data?.nextPageToken) return page;
    if (attempt < EMPTY_PAGE_RETRIES) {
      const position = body.pageToken ? 'continuation' : 'first';
      process.stderr.write(
        `Bridge returned an empty ${position} page; retrying (${attempt}/${EMPTY_PAGE_RETRIES}).\n`,
      );
      await new Promise((resolve) => setTimeout(resolve, EMPTY_PAGE_RETRY_DELAY_MS));
    }
  }
  if (body.pageToken) {
    throw new Error('Bridge returned an empty continuation page after retries.');
  }
  return page;
}

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    process.stdout.write(usage());
    return;
  }
  if (!options.out) throw new Error(`--out is required.\n\n${usage()}`);

  const token = process.env.CHAT_BRIDGE_TOKEN;
  if (!token) throw new Error('CHAT_BRIDGE_TOKEN is not set.');
  if (!Number.isInteger(options.pageSize) || options.pageSize < 1 || options.pageSize > 250) {
    throw new Error(`--page-size must be an integer between 1 and 250: ${options.pageSize}`);
  }

  await mkdir(options.out, { recursive: true });

  const space = await postJson({ action: 'space', token });
  const spaceName = space.data?.name;
  if (spaceName && spaceName !== EXPECTED_SPACE) {
    throw new Error(`Unexpected space ${spaceName}; refusing to write an off-target export.`);
  }

  let pageToken = '';
  let part = 0;
  let messageCount = 0;

  do {
    part += 1;
    const request = {
      action: 'messages',
      token,
      pageSize: options.pageSize,
      ...(pageToken ? { pageToken } : {}),
    };
    const page = await fetchMessagePage(request);

    const messages = page.data?.messages ?? [];
    messageCount += messages.length;
    const name = `page-${String(part).padStart(5, '0')}.json`;
    await writeFile(path.join(options.out, name), `${JSON.stringify(page, null, 2)}\n`);
    process.stderr.write(`${name}: ${messages.length} messages\n`);

    pageToken = page.data?.nextPageToken ?? '';
  } while (pageToken);

  if (messageCount === 0) {
    throw new Error('Bridge returned zero messages after retries; refusing a vacuous audit receipt.');
  }

  const summary = { space: EXPECTED_SPACE, pages: part, messages: messageCount };
  await writeFile(
    path.join(options.out, 'fetch-summary.json'),
    `${JSON.stringify(summary, null, 2)}\n`,
  );
  process.stdout.write(`${JSON.stringify(summary)}\n`);
}

if (import.meta.url === (await import('node:url')).pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exitCode = 1;
  });
}
