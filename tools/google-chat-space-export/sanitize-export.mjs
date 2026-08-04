#!/usr/bin/env node

import { promises as fs } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import { classifySensitiveMessage, quarantineMessage } from './sensitive-content.mjs';

function usage() {
  return `Usage:
  node tools/google-chat-space-export/sanitize-export.mjs \\
    --input <raw-export-file-or-directory> \\
    --out <sanitized-directory> \\
    [--since <RFC-3339>] [--report <report.json>] [--fail-on-sensitive]

Copies planner-readable Google Chat export pages into a sanitized directory.
Secret-bearing and high-confidence contact-only messages retain provenance and
routing metadata, but all content and attachments are quarantined. Reports never
contain message bodies or secret values.
`;
}

function parseArgs(argv) {
  const options = {
    inputs: [],
    output: null,
    report: null,
    since: null,
    failOnSensitive: false,
    help: false,
  };
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
      case '--input':
        options.inputs.push(next());
        break;
      case '--out':
        options.output = next();
        break;
      case '--report':
        options.report = next();
        break;
      case '--since':
        options.since = next();
        break;
      case '--fail-on-sensitive':
        options.failOnSensitive = true;
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

async function collectJsonFiles(inputPaths) {
  const files = [];
  async function visit(candidate) {
    const stat = await fs.stat(candidate);
    if (stat.isDirectory()) {
      const entries = await fs.readdir(candidate, { withFileTypes: true });
      entries.sort((left, right) => left.name.localeCompare(right.name));
      for (const entry of entries) await visit(path.join(candidate, entry.name));
      return;
    }
    if (stat.isFile() && candidate.toLowerCase().endsWith('.json')) files.push(path.resolve(candidate));
  }
  for (const input of inputPaths) await visit(path.resolve(input));
  return [...new Set(files)].sort();
}

function extractMessages(document) {
  if (Array.isArray(document?.messages)) return document.messages;
  if (Array.isArray(document?.data?.messages)) return document.data.messages;
  return null;
}

function replaceMessages(document, messages) {
  const output = structuredClone(document);
  if (Array.isArray(output?.messages)) output.messages = messages;
  else if (Array.isArray(output?.data?.messages)) output.data.messages = messages;
  return output;
}

function sourceKey(message) {
  return String(message?.sourceKey || message?.name || '<unknown>');
}

function timestamp(value, label) {
  if (!value) return null;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error(`${label} is not RFC-3339: ${value}`);
  return parsed;
}

export async function sanitizeDocuments(documents, options = {}) {
  const sinceTimestamp = timestamp(options.since, '--since');
  const report = {
    schemaVersion: 1,
    generatedAt: new Date().toISOString(),
    sinceInclusive: options.since || null,
    documents: documents.length,
    messagesSeen: 0,
    messagesInWindow: 0,
    safeMessages: 0,
    sensitiveMessages: 0,
    privateContactMessages: 0,
    quarantined: [],
  };

  const sanitized = documents.map(({ document, filePath }) => {
    const messages = extractMessages(document);
    if (!messages) return { document: structuredClone(document), filePath, messagePage: false };

    const outputMessages = messages.map((message) => {
      report.messagesSeen += 1;
      const createTimestamp = timestamp(message?.createTime, `createTime for ${sourceKey(message)}`);
      const inWindow = sinceTimestamp === null || (createTimestamp !== null && createTimestamp >= sinceTimestamp);
      if (inWindow) report.messagesInWindow += 1;

      const classification = classifySensitiveMessage(message);
      if (classification.classification === 'safe') {
        if (inWindow) report.safeMessages += 1;
        return structuredClone(message);
      }

      if (inWindow) {
        if (classification.classification === 'sensitive-secret') report.sensitiveMessages += 1;
        if (classification.classification === 'private-contact') report.privateContactMessages += 1;
        report.quarantined.push({
          sourceKey: sourceKey(message),
          createTime: message?.createTime || null,
          classification: classification.classification,
          kinds: classification.kinds,
        });
      }
      return quarantineMessage(message, classification);
    });

    return { document: replaceMessages(document, outputMessages), filePath, messagePage: true };
  });

  report.quarantined.sort((left, right) =>
    String(left.createTime).localeCompare(String(right.createTime)) ||
    left.sourceKey.localeCompare(right.sourceKey),
  );
  return { sanitized, report };
}

async function main(argv) {
  const options = parseArgs(argv);
  if (options.help) {
    process.stdout.write(usage());
    return;
  }
  if (options.inputs.length === 0 || !options.output) {
    throw new Error(`--input and --out are required.\n\n${usage()}`);
  }

  const files = await collectJsonFiles(options.inputs);
  const documents = [];
  for (const filePath of files) {
    documents.push({ filePath, document: JSON.parse(await fs.readFile(filePath, 'utf8')) });
  }

  const { sanitized, report } = await sanitizeDocuments(documents, options);
  const outputRoot = path.resolve(options.output);
  await fs.mkdir(outputRoot, { recursive: true });
  for (const entry of sanitized) {
    const filename = path.basename(entry.filePath);
    await fs.writeFile(path.join(outputRoot, filename), `${JSON.stringify(entry.document, null, 2)}\n`, {
      mode: 0o600,
    });
  }

  const reportPath = path.resolve(options.report || path.join(outputRoot, 'safety-report.json'));
  await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`${JSON.stringify(report)}\n`);

  if (options.failOnSensitive && report.sensitiveMessages > 0) process.exitCode = 2;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
