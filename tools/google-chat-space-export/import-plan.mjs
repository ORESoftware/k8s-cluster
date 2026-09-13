#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

export const EXPECTED_SPACE_NAME = 'spaces/AAQAoHKdzvI';
export const EXPECTED_SPACE_ID = 'AAQAoHKdzvI';
export const START_TIME_INCLUSIVE = '2026-05-10T04:00:00.000Z';
export const PLAN_SCHEMA_VERSION = 1;

const ACKNOWLEDGEMENT_PATTERNS = [
  /^(?:ok|okay|k|kk|got it|sounds good|great|thanks|thank you|ty|done|perfect|cool|nice|agreed|yep|yes|nope|no|sure|works for me|will do|on it)$/i,
  /^(?:👍|👌|✅|🙏|🙌|🎉|❤️|❤|🙂|😊|💯)+$/u,
];
const GREETING_ONLY_PATTERN = /^(?:hi|hello|hey|hola|greetings|good (?:morning|afternoon|evening))(?:[ ,]+(?:all|everyone|team|folks|there|alex))?[!.]*$/i;
const STANDALONE_EXCLUSION_PATTERNS = [
  /^(?:(?:all\s+)?(?:ai\s+)?agents?\s+(?:should|must)\s+)?(?:please\s+)?(?:ignore|skip|exclude)(?:\s+(?:this|the))?(?:\s+(?:message|thread|prompt|request))?(?:\s+(?:for|from)\s+(?:the\s+)?(?:ai\s+)?agents?)?[.!]*$/i,
  /^(?:please\s+)?(?:do not|don't)\s+(?:process|action|implement|work on|create (?:an?\s+)?issue for)(?:\s+(?:this|the))?(?:\s+(?:message|thread|prompt|request))?[.!]*$/i,
  /^(?:nothing to do|no action required|not actionable)[.!]*$/i,
];
const ACTIONABLE_PREFIX_PATTERN = /^(?:add|adopt|audit|build|change|check|create|debug|deploy|document|fix|generate|harden|implement|investigate|make|merge|migrate|open|patch|publish|push|refactor|remove|repair|replace|review|run|set up|setup|ship|test|track|update|upgrade|validate|verify|write)\b/i;
const SENSITIVE_FRAGMENT_PATTERN = /^(?:health|medical|diagnosis|medication|pregnancy|menstrual(?:\s+cycle)?|phone|email|address|contact)(?:\s+(?:note|info|information|details))?$/i;

function usage() {
  return `Usage:
  node tools/google-chat-space-export/import-plan.mjs \\
    --input <export-file-or-directory> [--input <another-path>] \\
    [--since <ISO-8601 timestamp>] \\
    [--until <ISO-8601 timestamp>] \\
    [--existing-index <linear-issues.json>] \\
    [--project-map <project-map.json>] \\
    [--json <plan.json>] [--markdown <plan.md>]

The command is read-only. It validates, deduplicates, classifies, and groups
exported Google Chat messages, then emits a dry-run plan. It never calls Linear
or creates issues.

--since may only narrow the fixed boundary ${START_TIME_INCLUSIVE}.
--until is exclusive and makes a rolling window repeatable.
`;
}

function parseArgs(argv) {
  const options = {
    inputs: [],
    since: null,
    until: null,
    existingIndex: null,
    projectMap: null,
    jsonOutput: null,
    markdownOutput: null,
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
      case '--input': options.inputs.push(next()); break;
      case '--since': options.since = next(); break;
      case '--until': options.until = next(); break;
      case '--existing-index': options.existingIndex = next(); break;
      case '--project-map': options.projectMap = next(); break;
      case '--json': options.jsonOutput = next(); break;
      case '--markdown': options.markdownOutput = next(); break;
      case '--help':
      case '-h': options.help = true; break;
      default: throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

function sha256(value) {
  return createHash('sha256').update(String(value)).digest('hex');
}

function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value).sort().map((key) => [key, stableValue(value[key])]),
    );
  }
  return value;
}

function stableStringify(value) {
  return JSON.stringify(stableValue(value));
}

export function computePlanId({
  spaceName,
  windowStartInclusive,
  windowEndExclusive = null,
  candidates,
}) {
  return `google-chat-import-plan:${sha256(stableStringify({
    spaceName,
    windowStartInclusive,
    windowEndExclusive,
    candidates: candidates.map((candidate) => ({
      key: candidate.candidateKey,
      action: candidate.action,
      messageCount: candidate.messageCount,
    })),
  })).slice(0, 24)}`;
}

function normalizeWhitespace(value) {
  return String(value ?? '')
    .replace(/\r\n?/g, '\n')
    .replace(/[\t ]+/g, ' ')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

function normalizeTitle(value) {
  return normalizeWhitespace(value)
    .toLocaleLowerCase('en-US')
    .replace(/https?:\/\/\S+/g, ' ')
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function messageText(message) {
  return normalizeWhitespace(
    message.text || message.formattedText || message.argumentText || message.fallbackText || '',
  );
}

function stripTerminalPunctuation(text) {
  return normalizeWhitespace(text).replace(/[.!?,;:]+$/g, '').trim();
}

function isAcknowledgementOnly(text) {
  const normalized = stripTerminalPunctuation(text);
  if (!normalized) return true;
  if (normalized.length > 80 || normalized.includes('\n')) return false;
  return ACKNOWLEDGEMENT_PATTERNS.some((pattern) => pattern.test(normalized));
}

function isStandaloneExclusionDirective(text) {
  const normalized = normalizeWhitespace(text);
  if (!normalized || normalized.length > 160 || normalized.includes('\n')) return false;
  return STANDALONE_EXCLUSION_PATTERNS.some((pattern) => pattern.test(normalized));
}

function isGreetingOnly(text) {
  const normalized = normalizeWhitespace(text);
  return normalized.length <= 80 && !normalized.includes('\n') && GREETING_ONLY_PATTERN.test(normalized);
}

function isShellPromptOnly(text) {
  const normalized = normalizeWhitespace(text);
  if (!normalized || normalized.length > 200 || normalized.includes('\n')) return false;
  return /^(?:[$#>%]|[A-Za-z0-9._-]+@[A-Za-z0-9._-]+(?::[^\n$#]{0,160})?[$#]|(?:PS\s+)?[A-Za-z]:\\[^>\n]{0,160}>)\s*$/.test(normalized);
}

function isContactOnly(text) {
  const normalized = stripTerminalPunctuation(text);
  if (!normalized || normalized.length > 200 || normalized.includes('\n')) return false;
  if (/^[^\s@<>]+@[^\s@<>]+\.[A-Za-z]{2,}$/.test(normalized)) return true;
  if (/^@[A-Za-z0-9_.-]{2,64}$/.test(normalized)) return true;
  if (/^\+?[\d()\s.-]{7,}$/.test(normalized)) {
    return (normalized.match(/\d/g) || []).length >= 7;
  }
  return /^(?:email|phone|contact|address)\s*:\s*\S.{0,160}$/i.test(normalized);
}

function isLowInformationFragment(text) {
  const normalized = normalizeWhitespace(text);
  if (!normalized || normalized.length > 120 || normalized.includes('\n')) return false;
  if (/[?]/.test(normalized)) return false;
  if (/https?:\/\//i.test(normalized)) return false;
  if (/\b[A-Z][A-Z0-9]+-[1-9][0-9]*\b/.test(normalized)) return false;
  const unwrapped = normalized
    .replace(/^[-*+>]\s+/, '')
    .replace(/^#{1,6}\s+/, '')
    .replace(/^\[[ xX]\]\s+/, '')
    .trim();
  if (ACTIONABLE_PREFIX_PATTERN.test(unwrapped)) return false;
  const tokens = unwrapped.split(/\s+/).filter(Boolean);
  return tokens.length <= 3 || (tokens.length <= 8 && /:$/.test(unwrapped));
}

export function classifyMessageText(text, options = {}) {
  const normalized = normalizeWhitespace(text);
  if (options.deleted) return { disposition: 'excluded', reasonCode: 'deleted' };
  if (!normalized) return { disposition: 'excluded', reasonCode: 'empty' };
  if (isStandaloneExclusionDirective(normalized)) {
    return { disposition: 'excluded', reasonCode: 'explicit_exclusion' };
  }
  if (isAcknowledgementOnly(normalized)) {
    return { disposition: 'excluded', reasonCode: 'acknowledgement' };
  }
  if (isGreetingOnly(normalized)) return { disposition: 'excluded', reasonCode: 'greeting' };
  if (isShellPromptOnly(normalized)) {
    return { disposition: 'excluded', reasonCode: 'shell_prompt' };
  }
  if (isContactOnly(normalized)) {
    return { disposition: 'quarantined', reasonCode: 'private_or_personal' };
  }
  if (SENSITIVE_FRAGMENT_PATTERN.test(stripTerminalPunctuation(normalized))) {
    return { disposition: 'quarantined', reasonCode: 'sensitive_fragment' };
  }
  if (isLowInformationFragment(normalized)) {
    return { disposition: 'quarantined', reasonCode: 'ambiguous_fragment' };
  }
  return { disposition: 'actionable', reasonCode: 'actionable' };
}

function validateTimestamp(value, label) {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) {
    throw new Error(`${label} is missing or is not an RFC-3339 timestamp: ${value}`);
  }
  return timestamp;
}

export function resolveWindowStart(since) {
  const boundary = Date.parse(START_TIME_INCLUSIVE);
  if (since === null || since === undefined || since === '') {
    return { iso: START_TIME_INCLUSIVE, timestamp: boundary, narrowed: false };
  }
  const timestamp = validateTimestamp(since, '--since');
  if (timestamp < boundary) {
    throw new Error(
      `--since ${since} predates the fixed boundary ${START_TIME_INCLUSIVE}; it can only narrow the window.`,
    );
  }
  return { iso: new Date(timestamp).toISOString(), timestamp, narrowed: timestamp > boundary };
}

export function resolveWindowEnd(until, startTimestamp) {
  if (until === null || until === undefined || until === '') {
    return { iso: null, timestamp: Number.POSITIVE_INFINITY, bounded: false };
  }
  const timestamp = validateTimestamp(until, '--until');
  if (timestamp <= startTimestamp) throw new Error('--until must be later than the effective --since boundary');
  return { iso: new Date(timestamp).toISOString(), timestamp, bounded: true };
}

function canonicalSourceKey(message) {
  if (message.sourceKey) return String(message.sourceKey);
  if (message.name) return `google-chat:${EXPECTED_SPACE_ID}:${message.name}`;
  return `google-chat:${EXPECTED_SPACE_ID}:derived:${sha256(stableStringify({
    createTime: message.createTime,
    sender: message.sender?.name || message.sender?.displayName || null,
    thread: message.thread?.name || null,
    text: messageText(message),
  }))}`;
}

function canonicalizeMessage(message, source, filePath) {
  if (!message || typeof message !== 'object') throw new Error(`Invalid message object in ${filePath}`);
  const spaceName = message.spaceName || message.space?.name || source?.spaceName || source?.name || null;
  if (spaceName !== EXPECTED_SPACE_NAME) {
    throw new Error(`Wrong Google Chat space in ${filePath}: expected ${EXPECTED_SPACE_NAME}, got ${spaceName}`);
  }
  const createTime = String(message.createTime || '');
  const createTimestamp = validateTimestamp(createTime, `Message createTime in ${filePath}`);
  if (createTimestamp < Date.parse(START_TIME_INCLUSIVE)) {
    throw new Error(`Message ${message.name || '<unnamed>'} predates ${START_TIME_INCLUSIVE}: ${createTime}`);
  }
  const sourceKey = canonicalSourceKey(message);
  const threadName = message.thread?.name || null;
  const threadSourceKey = message.thread?.sourceKey ||
    (threadName ? `google-chat:${EXPECTED_SPACE_ID}:${threadName}` : null);
  return {
    sourceKey,
    name: message.name || null,
    spaceName,
    threadName,
    threadSourceKey,
    threadReply: Boolean(message.threadReply),
    sender: {
      name: message.sender?.name || null,
      displayName: message.sender?.displayName || null,
      type: message.sender?.type || null,
    },
    createTime,
    lastUpdateTime: message.lastUpdateTime || null,
    deleteTime: message.deleteTime || null,
    deleted: Boolean(message.deleteTime || message.deletionMetadata),
    deletionMetadata: message.deletionMetadata || null,
    text: messageText(message),
    attachments: Array.isArray(message.attachments)
      ? message.attachments
      : Array.isArray(message.attachment) ? message.attachment : [],
    originFile: filePath,
  };
}

function extractPage(document, filePath) {
  const messages = Array.isArray(document?.messages)
    ? document.messages
    : Array.isArray(document?.data?.messages) ? document.data.messages : null;
  if (!messages) return null;
  const source = document.source || document.data?.target || document.data?.source || null;
  const declaredSpace = source?.spaceName || source?.name || null;
  if (declaredSpace && declaredSpace !== EXPECTED_SPACE_NAME) {
    throw new Error(`Wrong export source in ${filePath}: expected ${EXPECTED_SPACE_NAME}, got ${declaredSpace}`);
  }
  return {
    filePath,
    source,
    messages,
    runId: document.runId || document.data?.runId || null,
    partNumber: document.partNumber || document.data?.partNumber || null,
    nextPageToken: document.nextPageToken || document.data?.nextPageToken || null,
  };
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
    if (stat.isFile() && candidate.toLocaleLowerCase('en-US').endsWith('.json')) {
      files.push(path.resolve(candidate));
    }
  }
  for (const inputPath of inputPaths) await visit(path.resolve(inputPath));
  return [...new Set(files)].sort();
}

async function readJson(pathname) {
  try {
    return JSON.parse(await fs.readFile(pathname, 'utf8'));
  } catch (error) {
    throw new Error(`Could not parse JSON file ${pathname}: ${error.message}`);
  }
}

function extractSourceKeysFromIssue(issue) {
  const explicit = Array.isArray(issue.sourceKeys) ? issue.sourceKeys.map(String) : [];
  const searchable = [issue.description, issue.title, ...(issue.comments || [])].filter(Boolean).join('\n');
  const discovered = searchable.match(/google-chat:[A-Za-z0-9_./:~-]+/g) || [];
  return [...new Set([...explicit, ...discovered])].sort();
}

function buildExistingIndex(document) {
  const issues = Array.isArray(document) ? document : document?.issues;
  if (!issues) return { issues: [], bySourceKey: new Map(), byTitle: new Map() };
  if (!Array.isArray(issues)) throw new Error('Existing issue index must be an array or {issues: []}.');
  const normalizedIssues = issues.map((issue) => ({
    id: String(issue.id || issue.identifier || issue.url || ''),
    identifier: issue.identifier || issue.id || null,
    title: String(issue.title || ''),
    project: issue.project || null,
    state: issue.state || issue.status || null,
    url: issue.url || null,
    sourceKeys: extractSourceKeysFromIssue(issue),
  }));
  const bySourceKey = new Map();
  const byTitle = new Map();
  for (const issue of normalizedIssues) {
    for (const sourceKey of issue.sourceKeys) {
      const matches = bySourceKey.get(sourceKey) || [];
      matches.push(issue);
      bySourceKey.set(sourceKey, matches);
    }
    const titleKey = normalizeTitle(issue.title);
    if (titleKey) {
      const matches = byTitle.get(titleKey) || [];
      matches.push(issue);
      byTitle.set(titleKey, matches);
    }
  }
  return { issues: normalizedIssues, bySourceKey, byTitle };
}

function normalizeProjectMap(document) {
  if (!document) return { repositories: {}, organizations: {} };
  return {
    repositories: document.repositories || document.repos || {},
    organizations: document.organizations || document.orgs || {},
  };
}

function stripRepositorySuffix(value) {
  return String(value).replace(/\.git$/i, '').replace(/[),.;:'"\]}>]+$/g, '');
}

function extractGitHubReferences(texts) {
  const repositories = new Set();
  const organizations = new Set();
  const combined = texts.join('\n');
  const repositoryPattern = /(?:https?:\/\/)?(?:www\.)?github\.com\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)/gi;
  let match;
  while ((match = repositoryPattern.exec(combined)) !== null) {
    const owner = match[1];
    const repository = stripRepositorySuffix(match[2]);
    if (!owner || !repository) continue;
    repositories.add(`${owner}/${repository}`);
    organizations.add(owner);
  }
  const organizationPattern = /(?:https?:\/\/)?(?:www\.)?github\.com\/([A-Za-z0-9_.-]+)(?:\b|\/)/gi;
  while ((match = organizationPattern.exec(combined)) !== null) organizations.add(match[1]);
  return {
    repositories: [...repositories].sort((a, b) => a.localeCompare(b)),
    organizations: [...organizations].sort((a, b) => a.localeCompare(b)),
  };
}

function chooseProject(references, projectMap) {
  const mappedRepositories = references.repositories
    .map((repository) => ({ repository, project: projectMap.repositories[repository] }))
    .filter((entry) => entry.project);
  const mappedOrganizations = references.organizations
    .map((organization) => ({ organization, project: projectMap.organizations[organization] }))
    .filter((entry) => entry.project);
  const mappedProjects = new Set([
    ...mappedRepositories.map((entry) => entry.project),
    ...mappedOrganizations.map((entry) => entry.project),
  ]);
  if (mappedProjects.size === 1) {
    const project = [...mappedProjects][0];
    return {
      project,
      confidence: mappedRepositories.length > 0 ? 1 : 0.95,
      rationale: mappedRepositories.length > 0
        ? 'Explicit GitHub repository matched the configured project map.'
        : 'Explicit GitHub organization matched the configured project map.',
      ambiguous: false,
    };
  }
  if (mappedProjects.size > 1) {
    return { project: null, confidence: 0, rationale: 'Explicit GitHub references map to multiple Linear projects.', ambiguous: true };
  }
  if (references.repositories.length === 1) {
    return {
      project: `github.com/${references.repositories[0]}`,
      confidence: 0.72,
      rationale: 'Explicit repository reference found; verify that the matching Linear project exists.',
      ambiguous: false,
    };
  }
  if (references.repositories.length > 1) {
    return { project: null, confidence: 0, rationale: 'Multiple repository references require manual project selection.', ambiguous: true };
  }
  if (references.organizations.length === 1) {
    return {
      project: `github.com/${references.organizations[0]}`,
      confidence: 0.65,
      rationale: 'Explicit organization reference found; verify the matching Linear project.',
      ambiguous: false,
    };
  }
  if (references.organizations.length > 1) {
    return { project: null, confidence: 0, rationale: 'Multiple organization references require manual project selection.', ambiguous: true };
  }
  return { project: null, confidence: 0, rationale: 'No explicit GitHub organization or repository reference was found.', ambiguous: true };
}

function truncateAtWord(value, maxLength) {
  const text = normalizeWhitespace(value);
  if (text.length <= maxLength) return text;
  const truncated = text.slice(0, maxLength + 1);
  const boundary = truncated.lastIndexOf(' ');
  return `${truncated.slice(0, boundary > maxLength * 0.6 ? boundary : maxLength).trim()}…`;
}

function deriveActionableTitle(messages) {
  const first = messages[0];
  const line = normalizeWhitespace(first?.text || '')
    .split('\n').map((part) => part.trim()).find(Boolean);
  let title = String(line || '')
    .replace(/^[-*+>]\s+/, '')
    .replace(/^\[[ xX]\]\s+/, '')
    .replace(/^#{1,6}\s+/, '')
    .replace(/https?:\/\/\S+/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  if (title.length < 5) title = `Review Google Chat thread from ${first?.createTime?.slice(0, 10) || 'unknown date'}`;
  return truncateAtWord(title, 120);
}

function genericTitle(disposition, messages) {
  const date = messages[0]?.createTime?.slice(0, 10) || 'unknown date';
  if (disposition === 'quarantined') return `Review quarantined Google Chat thread from ${date}`;
  return `Excluded non-actionable Google Chat thread from ${date}`;
}

function classifyThread(messages) {
  const entries = messages.map((message) => ({
    message,
    classification: classifyMessageText(message.text, { deleted: message.deleted }),
  }));
  if (entries.some((entry) => entry.classification.reasonCode === 'explicit_exclusion')) {
    return {
      disposition: 'excluded',
      entries: entries.map((entry) => ({
        ...entry,
        classification: { disposition: 'excluded', reasonCode: 'thread_explicit_exclusion' },
      })),
      reasonCodes: ['thread_explicit_exclusion'],
      counts: { actionable: 0, quarantined: 0, excluded: messages.length },
    };
  }
  const counts = { actionable: 0, quarantined: 0, excluded: 0 };
  for (const entry of entries) counts[entry.classification.disposition] += 1;
  const disposition = counts.actionable > 0
    ? 'actionable'
    : counts.quarantined > 0 ? 'quarantined' : 'excluded';
  return {
    disposition,
    entries,
    reasonCodes: [...new Set(entries.map((entry) => entry.classification.reasonCode))].sort(),
    counts,
  };
}

function renderCandidateDescription(threadClassification) {
  const { entries, disposition } = threadClassification;
  const messages = entries.map((entry) => entry.message);
  const lines = [
    disposition === 'actionable'
      ? 'Imported from Google Chat after dry-run review.'
      : 'Google Chat content withheld from generated issue descriptions.',
    '',
    `Space: ${EXPECTED_SPACE_NAME}`,
    `Thread: ${messages[0]?.threadName || 'unthreaded'}`,
    '',
    'Source messages:',
  ];
  let remaining = 12000;
  for (const entry of entries) {
    const { message, classification } = entry;
    const author = message.sender.displayName || message.sender.name || 'unknown author';
    const excerpt = classification.disposition === 'actionable'
      ? truncateAtWord(message.text || '[empty message]', 800)
      : `[content withheld: ${classification.reasonCode}]`;
    const line = `- ${message.createTime} — ${author}: ${excerpt} (${message.sourceKey})`;
    if (line.length > remaining) {
      lines.push('- [Additional source-message metadata omitted.]');
      break;
    }
    lines.push(line);
    remaining -= line.length;
  }
  return lines.join('\n');
}

function issueSummary(issue) {
  return {
    id: issue.id,
    identifier: issue.identifier,
    title: issue.title,
    project: issue.project,
    state: issue.state,
    url: issue.url,
  };
}

export function buildImportPlan(pageDocuments, options = {}) {
  const projectMap = normalizeProjectMap(options.projectMap);
  const existing = buildExistingIndex(options.existingIndex || []);
  const window = resolveWindowStart(options.since);
  const windowEnd = resolveWindowEnd(options.until, window.timestamp);
  const uniqueMessages = new Map();
  const pageRuns = new Set();
  let rawMessageCount = 0;
  let ignoredJsonFiles = 0;

  for (const item of pageDocuments) {
    const page = extractPage(item.document, item.path);
    if (!page) { ignoredJsonFiles += 1; continue; }
    if (page.runId) pageRuns.add(String(page.runId));
    for (const rawMessage of page.messages) {
      rawMessageCount += 1;
      const message = canonicalizeMessage(rawMessage, page.source, item.path);
      const comparable = stableStringify({ ...message, originFile: undefined });
      const previous = uniqueMessages.get(message.sourceKey);
      if (previous && previous.comparable !== comparable) {
        throw new Error(`Conflicting duplicate message for source key ${message.sourceKey} in ${item.path}`);
      }
      if (!previous) uniqueMessages.set(message.sourceKey, { message, comparable });
    }
  }

  const deduped = [...uniqueMessages.values()].map((entry) => entry.message).sort((left, right) => {
    const time = left.createTime.localeCompare(right.createTime);
    return time || left.sourceKey.localeCompare(right.sourceKey);
  });
  const messages = window.narrowed || windowEnd.bounded
    ? deduped.filter((message) => {
        const timestamp = Date.parse(message.createTime);
        return timestamp >= window.timestamp && timestamp < windowEnd.timestamp;
      })
    : deduped;
  const windowedOutMessages = deduped.length - messages.length;
  const groups = new Map();
  for (const message of messages) {
    const threadKey = message.threadSourceKey || message.threadName || message.name || message.sourceKey;
    const group = groups.get(threadKey) || [];
    group.push(message);
    groups.set(threadKey, group);
  }

  const candidates = [];
  let skippedNonActionable = 0;
  for (const [threadKey, threadMessages] of [...groups.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    const classification = classifyThread(threadMessages);
    const actionableMessages = classification.entries
      .filter((entry) => entry.classification.disposition === 'actionable')
      .map((entry) => entry.message);
    const title = classification.disposition === 'actionable'
      ? deriveActionableTitle(actionableMessages)
      : genericTitle(classification.disposition, threadMessages);
    const normalizedCandidateTitle = normalizeTitle(title);
    const sourceKeys = threadMessages.map((message) => message.sourceKey).sort();
    const references = extractGitHubReferences(actionableMessages.map((message) => message.text));
    const project = chooseProject(references, projectMap);
    const exactIssues = new Map();
    for (const sourceKey of sourceKeys) {
      for (const issue of existing.bySourceKey.get(sourceKey) || []) exactIssues.set(issue.id, issue);
    }
    const titleIssues = existing.byTitle.get(normalizedCandidateTitle) || [];
    const manualReviewReasons = [];

    let action = 'create';
    if (classification.disposition === 'excluded') {
      action = 'skip-non-actionable';
      skippedNonActionable += 1;
    } else if (classification.disposition === 'quarantined') {
      action = 'manual-review';
      manualReviewReasons.push('Thread contains only ambiguous or sensitive short records; content is withheld.');
    } else if (exactIssues.size > 0) {
      action = 'comment-existing';
    } else {
      if (titleIssues.length > 0) manualReviewReasons.push('Normalized title matches one or more existing Linear issues.');
      if (project.ambiguous || project.confidence < 0.7) manualReviewReasons.push(project.rationale);
      if (actionableMessages.length > 20) {
        manualReviewReasons.push('Thread contains more than 20 actionable messages and may contain multiple work items.');
      }
      if (manualReviewReasons.length > 0) action = 'manual-review';
    }

    const candidateKey = `google-chat:${EXPECTED_SPACE_ID}:${sha256(stableStringify({
      threadKey,
      sourceKeys,
      title: normalizedCandidateTitle,
    })).slice(0, 24)}`;

    candidates.push({
      candidateKey,
      action,
      title,
      normalizedTitle: normalizedCandidateTitle,
      description: renderCandidateDescription(classification),
      threadKey,
      threadName: threadMessages[0]?.threadName || null,
      sourceKeys,
      messageNames: threadMessages.map((message) => message.name).filter(Boolean),
      firstCreateTime: threadMessages[0]?.createTime || null,
      lastCreateTime: threadMessages.at(-1)?.createTime || null,
      authors: [...new Set(threadMessages
        .map((message) => message.sender.displayName || message.sender.name)
        .filter(Boolean))].sort(),
      messageCount: threadMessages.length,
      substantiveMessageCount: classification.counts.actionable,
      classification: {
        disposition: classification.disposition,
        reasonCodes: classification.reasonCodes,
        actionableMessageCount: classification.counts.actionable,
        quarantinedMessageCount: classification.counts.quarantined,
        excludedMessageCount: classification.counts.excluded,
      },
      githubReferences: references,
      proposedProject: project.project,
      projectConfidence: project.confidence,
      projectRationale: project.rationale,
      exactExistingIssues: [...exactIssues.values()].map(issueSummary),
      titleExistingIssues: titleIssues.map(issueSummary),
      manualReviewReasons,
    });
  }

  const stats = {
    jsonFiles: pageDocuments.length,
    ignoredJsonFiles,
    exportRuns: pageRuns.size,
    rawMessages: rawMessageCount,
    uniqueMessages: deduped.length,
    duplicateMessages: rawMessageCount - deduped.length,
    windowedOutMessages,
    plannedMessages: messages.length,
    threads: groups.size,
    candidates: candidates.length,
    create: candidates.filter((candidate) => candidate.action === 'create').length,
    commentExisting: candidates.filter((candidate) => candidate.action === 'comment-existing').length,
    manualReview: candidates.filter((candidate) => candidate.action === 'manual-review').length,
    skippedNonActionable,
    actionableMessages: candidates.reduce((sum, candidate) => sum + candidate.classification.actionableMessageCount, 0),
    quarantinedMessages: candidates.reduce((sum, candidate) => sum + candidate.classification.quarantinedMessageCount, 0),
    excludedMessages: candidates.reduce((sum, candidate) => sum + candidate.classification.excludedMessageCount, 0),
    actionableThreads: candidates.filter((candidate) => candidate.classification.disposition === 'actionable').length,
    quarantinedThreads: candidates.filter((candidate) => candidate.classification.disposition === 'quarantined').length,
    excludedThreads: candidates.filter((candidate) => candidate.classification.disposition === 'excluded').length,
    duplicateOwned: candidates.filter((candidate) => candidate.action === 'comment-existing').length,
  };

  const planId = computePlanId({
    spaceName: EXPECTED_SPACE_NAME,
    windowStartInclusive: window.iso,
    windowEndExclusive: windowEnd.iso,
    candidates,
  });
  return {
    schemaVersion: PLAN_SCHEMA_VERSION,
    planId,
    generatedAt: new Date().toISOString(),
    source: {
      spaceName: EXPECTED_SPACE_NAME,
      spaceId: EXPECTED_SPACE_ID,
      startTimeInclusive: START_TIME_INCLUSIVE,
      windowStartInclusive: window.iso,
      ...(windowEnd.iso ? { windowEndExclusive: windowEnd.iso } : {}),
      runIds: [...pageRuns].sort(),
    },
    stats,
    candidates,
  };
}

function escapeMarkdownCell(value) {
  return String(value ?? '').replace(/\|/g, '\\|').replace(/\r?\n/g, '<br>');
}

export function renderMarkdown(plan) {
  const lines = [
    '# Google Chat → Linear dry-run plan',
    '',
    `Plan ID: \`${plan.planId}\``,
    '',
    `Source: \`${plan.source.spaceName}\` from \`${plan.source.startTimeInclusive}\``,
    ...(plan.source.windowStartInclusive && plan.source.windowStartInclusive !== plan.source.startTimeInclusive
      ? [`Window narrowed to messages at or after \`${plan.source.windowStartInclusive}\`.`] : []),
    ...(plan.source.windowEndExclusive
      ? [`Window excludes messages at or after \`${plan.source.windowEndExclusive}\`.`] : []),
    '',
    '## Reconciliation summary',
    '',
    '| Metric | Count |',
    '|---|---:|',
    ...Object.entries(plan.stats).map(([key, value]) => `| ${escapeMarkdownCell(key)} | ${value} |`),
    '',
    '## Candidates',
    '',
    '| Action | Disposition | Title | Proposed project | Confidence | Messages | Existing match |',
    '|---|---|---|---|---:|---:|---|',
  ];
  for (const candidate of plan.candidates) {
    const existing = candidate.exactExistingIssues.map((issue) => issue.identifier || issue.id).join(', ') ||
      candidate.titleExistingIssues.map((issue) => issue.identifier || issue.id).join(', ') || '';
    lines.push(`| ${escapeMarkdownCell(candidate.action)} | ${escapeMarkdownCell(candidate.classification?.disposition || 'unknown')} | ${escapeMarkdownCell(candidate.title)} | ${escapeMarkdownCell(candidate.proposedProject || 'manual review')} | ${candidate.projectConfidence.toFixed(2)} | ${candidate.messageCount} | ${escapeMarkdownCell(existing)} |`);
  }
  for (const candidate of plan.candidates) {
    lines.push('', `### ${candidate.title}`, '');
    lines.push(`- Candidate key: \`${candidate.candidateKey}\``);
    lines.push(`- Action: \`${candidate.action}\``);
    lines.push(`- Disposition: \`${candidate.classification?.disposition || 'unknown'}\``);
    lines.push(`- Thread: \`${candidate.threadName || candidate.threadKey}\``);
    lines.push(`- Proposed project: \`${candidate.proposedProject || 'unresolved'}\``);
    lines.push(`- Project rationale: ${candidate.projectRationale}`);
    if (candidate.manualReviewReasons.length > 0) {
      lines.push('- Manual-review reasons:');
      for (const reason of candidate.manualReviewReasons) lines.push(`  - ${reason}`);
    }
    lines.push('- Source keys:');
    for (const sourceKey of candidate.sourceKeys) lines.push(`  - \`${sourceKey}\``);
  }
  return `${lines.join('\n')}\n`;
}

async function writeOutput(destination, content) {
  await fs.mkdir(path.dirname(path.resolve(destination)), { recursive: true });
  await fs.writeFile(destination, content, 'utf8');
}

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) { process.stdout.write(usage()); return; }
  if (options.inputs.length === 0) throw new Error(`At least one --input path is required.\n\n${usage()}`);
  const jsonFiles = await collectJsonFiles(options.inputs);
  if (jsonFiles.length === 0) throw new Error('No JSON files were found in the supplied inputs.');
  const pageDocuments = [];
  for (const filePath of jsonFiles) pageDocuments.push({ path: filePath, document: await readJson(filePath) });
  const existingIndex = options.existingIndex ? await readJson(options.existingIndex) : [];
  const projectMap = options.projectMap ? await readJson(options.projectMap) : null;
  const plan = buildImportPlan(pageDocuments, {
    existingIndex,
    projectMap,
    since: options.since,
    until: options.until,
  });
  const json = `${JSON.stringify(plan, null, 2)}\n`;
  const markdown = renderMarkdown(plan);
  if (options.jsonOutput) await writeOutput(options.jsonOutput, json);
  if (options.markdownOutput) await writeOutput(options.markdownOutput, markdown);
  if (!options.jsonOutput && !options.markdownOutput) process.stdout.write(json);
  else process.stderr.write(`${JSON.stringify(plan.stats)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exitCode = 1;
  });
}
