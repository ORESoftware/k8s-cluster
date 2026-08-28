#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

const MAX_WORKFLOW_BYTES = 512 * 1024;
const MAX_MANIFEST_ENTRIES = 64;
const SHA_PIN = /^[0-9a-f]{40}$/i;
const SHA256_PIN = /^sha256:[0-9a-f]{64}$/i;

function leadingSpaces(line) {
  return line.length - line.trimStart().length;
}

function lineNumberAt(source, index) {
  return source.slice(0, Math.max(0, index)).split('\n').length;
}

function stripCommentOnlyLines(source) {
  return source
    .split('\n')
    .map((line) => (line.trimStart().startsWith('#') ? '' : line))
    .join('\n');
}

function eventNames(source) {
  const names = new Set();
  const lines = source.split('\n');

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const onMatch = line.match(/^(\s*)(?:on|["']on["'])\s*:\s*(.*)$/);
    if (!onMatch) continue;

    const baseIndent = onMatch[1].length;
    const inline = onMatch[2].trim();
    for (const event of ['pull_request', 'pull_request_target']) {
      const eventPattern = new RegExp(`(?:^|[\\s,\\[])${event}(?:$|[\\s,\\]])`);
      if (eventPattern.test(inline)) names.add(event);
    }

    if (inline !== '') continue;

    for (let child = index + 1; child < lines.length; child += 1) {
      const candidate = lines[child];
      if (candidate.trim() === '' || candidate.trimStart().startsWith('#')) continue;
      const indent = leadingSpaces(candidate);
      if (indent <= baseIndent) break;
      const eventMatch = candidate.match(/^\s*(pull_request|pull_request_target)\s*:/);
      if (eventMatch) names.add(eventMatch[1]);
    }
  }

  return names;
}

function stepBlock(lines, lineIndex) {
  let start = lineIndex;
  let stepIndent = leadingSpaces(lines[lineIndex]);

  for (let index = lineIndex; index >= 0; index -= 1) {
    const match = lines[index].match(/^(\s*)-\s+/);
    if (!match) continue;
    if (match[1].length <= stepIndent) {
      start = index;
      stepIndent = match[1].length;
      break;
    }
  }

  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    const match = lines[index].match(/^(\s*)-\s+/);
    if (match && match[1].length === stepIndent) {
      end = index;
      break;
    }
  }

  return {
    start,
    end,
    text: lines.slice(start, end).join('\n'),
  };
}

function annotationEscape(value) {
  return String(value)
    .replaceAll('%', '%25')
    .replaceAll('\r', '%0D')
    .replaceAll('\n', '%0A');
}

export function scanWorkflowText(source, displayPath = '<workflow>') {
  if (typeof source !== 'string') {
    throw new TypeError('workflow source must be a string');
  }

  const byteLength = Buffer.byteLength(source, 'utf8');
  const issues = [];
  const add = (code, message, index = 0) => {
    issues.push({
      code,
      message,
      path: displayPath,
      line: lineNumberAt(source, index),
    });
  };

  if (byteLength > MAX_WORKFLOW_BYTES) {
    add('workflow-too-large', `workflow is ${byteLength} bytes; limit is ${MAX_WORKFLOW_BYTES}`);
    return issues;
  }
  if (source.includes('\0')) {
    add('nul-byte', 'workflow contains a NUL byte');
    return issues;
  }

  const active = stripCommentOnlyLines(source);
  const events = eventNames(active);
  const isPullRequestWorkflow = events.has('pull_request') || events.has('pull_request_target');
  if (!isPullRequestWorkflow) return issues;

  const permissionPatterns = [
    {
      code: 'write-all-permissions',
      regex: /^\s*permissions\s*:\s*write-all\s*$/gim,
      message: 'PR-triggered workflows must not request write-all permissions',
    },
    {
      code: 'write-permission',
      regex: /^\s*(?:actions|checks|contents|deployments|id-token|packages|pull-requests|security-events|statuses|workflows)\s*:\s*write\s*$/gim,
      message: 'PR-triggered workflows must not request write-capable token scopes',
    },
  ];
  for (const pattern of permissionPatterns) {
    for (const match of active.matchAll(pattern.regex)) {
      add(pattern.code, pattern.message, match.index);
    }
  }

  const lines = active.split('\n');
  for (let index = 0; index < lines.length; index += 1) {
    const usesMatch = lines[index].match(/\buses\s*:\s*["']?([^\s"'#]+)["']?/i);
    if (!usesMatch) continue;

    const target = usesMatch[1];
    if (target.startsWith('./')) continue;

    if (target.startsWith('docker://')) {
      const digest = target.includes('@') ? target.slice(target.lastIndexOf('@') + 1) : '';
      if (!SHA256_PIN.test(digest)) {
        add('mutable-docker-action', `Docker action must be pinned by sha256 digest: ${target}`, active.indexOf(lines[index]));
      }
      continue;
    }

    const separator = target.lastIndexOf('@');
    const ref = separator >= 0 ? target.slice(separator + 1) : '';
    if (!SHA_PIN.test(ref)) {
      add('mutable-action-ref', `action or reusable workflow must use a full 40-character commit SHA: ${target}`, active.indexOf(lines[index]));
    }

    if (!/^actions\/checkout@/i.test(target)) continue;
    const block = stepBlock(lines, index);
    if (!/^\s*persist-credentials\s*:\s*false\s*$/im.test(block.text)) {
      add('checkout-persists-credentials', 'actions/checkout must set persist-credentials: false in PR workflows', active.indexOf(lines[index]));
    }

    if (events.has('pull_request_target')) {
      const checksOutUntrustedHead = /github\.(?:head_ref|event\.pull_request\.head\.(?:sha|ref|label|repo\.full_name))|refs\/pull\//i.test(block.text);
      if (checksOutUntrustedHead) {
        add('target-checkout-head', 'pull_request_target must never checkout the pull request head or head repository', active.indexOf(lines[index]));
      }
    }
  }

  const forbiddenCommands = [
    ['git-push', /\bgit\s+(?:-[^\s]+\s+)*push\b/gi, 'PR workflows must not push refs'],
    ['git-commit', /\bgit\s+(?:-[^\s]+\s+)*commit\b/gi, 'PR workflows must not create commits'],
    ['git-update-ref', /\bgit\s+(?:-[^\s]+\s+)*update-ref\b/gi, 'PR workflows must not update refs'],
    ['gh-pr-mutation', /\bgh\s+pr\s+(?:close|edit|merge|ready|reopen)\b/gi, 'PR workflows must not mutate pull requests'],
    ['workflow-self-delete', /(?:\brm\b|\bunlink\b|\bgit\s+rm\b)[^\n]*\.github\/workflows\//gi, 'PR workflows must not delete workflow definitions'],
    ['decoded-shell', /\bbase64\b[^\n]*(?:--decode|-d)\b[^\n]*\|\s*(?:ba)?sh\b/gi, 'PR workflows must not decode data directly into a shell'],
    ['github-ref-api-mutation', /\bcurl\b[^\n]*api\.github\.com[^\n]*(?:\/git\/refs|\/pulls|\/actions\/workflows)/gi, 'PR workflows must not mutate GitHub refs, pull requests, or workflows via curl'],
  ];
  for (const [code, regex, message] of forbiddenCommands) {
    for (const match of active.matchAll(regex)) add(code, message, match.index);
  }

  if (events.has('pull_request_target')) {
    const untrustedShellPatterns = [
      /\$\{\{\s*github\.event\.pull_request\.(?:title|body|head\.ref|head\.label)\s*\}\}/gi,
      /\$\{\{\s*github\.head_ref\s*\}\}/gi,
    ];
    for (const regex of untrustedShellPatterns) {
      for (const match of active.matchAll(regex)) {
        const lineStart = active.lastIndexOf('\n', match.index) + 1;
        const lineEnd = active.indexOf('\n', match.index);
        const line = active.slice(lineStart, lineEnd === -1 ? undefined : lineEnd);
        if (/\brun\s*:|\bshell\s*:/i.test(line)) {
          add('target-untrusted-shell-input', 'pull_request_target must not interpolate pull request-controlled text into shell commands', match.index);
        }
      }
    }

    const targetGitCheckout = /\bgit\s+(?:checkout|switch)\b[^\n]*(?:github\.head_ref|pull_request\.head|refs\/pull\/)/gi;
    for (const match of active.matchAll(targetGitCheckout)) {
      add('target-git-checkout-head', 'pull_request_target must not git-checkout pull request head content', match.index);
    }
  }

  return issues;
}

async function loadManifest(manifestPath) {
  const raw = await readFile(manifestPath, 'utf8');
  const parsed = JSON.parse(raw);
  if (!Array.isArray(parsed) || parsed.length > MAX_MANIFEST_ENTRIES) {
    throw new Error(`manifest must be an array with at most ${MAX_MANIFEST_ENTRIES} entries`);
  }

  return parsed.map((entry, index) => {
    if (
      entry === null ||
      typeof entry !== 'object' ||
      typeof entry.path !== 'string' ||
      typeof entry.displayPath !== 'string' ||
      entry.path.length === 0 ||
      entry.displayPath.length === 0
    ) {
      throw new Error(`invalid manifest entry at index ${index}`);
    }
    return entry;
  });
}

async function cli(argv) {
  let entries;
  if (argv[0] === '--manifest') {
    if (argv.length !== 2) throw new Error('usage: check-pr-workflow-safety.mjs --manifest <manifest.json>');
    entries = await loadManifest(argv[1]);
  } else {
    if (argv.length === 0) throw new Error('provide one or more workflow files or --manifest <manifest.json>');
    entries = argv.map((file) => ({ path: file, displayPath: file }));
  }

  const allIssues = [];
  for (const entry of entries) {
    const absolute = path.resolve(entry.path);
    const bytes = await readFile(absolute);
    if (bytes.length > MAX_WORKFLOW_BYTES) {
      allIssues.push({
        code: 'workflow-too-large',
        message: `workflow is ${bytes.length} bytes; limit is ${MAX_WORKFLOW_BYTES}`,
        path: entry.displayPath,
        line: 1,
      });
      continue;
    }
    if (bytes.includes(0)) {
      allIssues.push({ code: 'nul-byte', message: 'workflow contains a NUL byte', path: entry.displayPath, line: 1 });
      continue;
    }
    allIssues.push(...scanWorkflowText(bytes.toString('utf8'), entry.displayPath));
  }

  if (allIssues.length === 0) {
    console.log(`workflow safety: ${entries.length} file(s) passed`);
    return;
  }

  for (const issue of allIssues) {
    console.error(
      `::error file=${annotationEscape(issue.path)},line=${issue.line},title=${annotationEscape(issue.code)}::${annotationEscape(issue.message)}`,
    );
  }
  throw new Error(`workflow safety rejected ${allIssues.length} issue(s)`);
}

const invokedDirectly = process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url;
if (invokedDirectly) {
  cli(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
