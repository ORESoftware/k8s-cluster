import { execFile } from 'node:child_process';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { promisify } from 'node:util';

import { tool, type ToolSet } from 'ai';
import { z } from 'zod';

import type { AgentRunnerEvent } from './types.js';

const execFileAsync = promisify(execFile);

const PR_BODY_MAX_BYTES = 60_000;
const PR_COMMENT_MAX_BYTES = 60_000;
const GH_TIMEOUT_MS = 30_000;
// Subcommands the workspace-tools-driven agent is allowed to invoke through gh.
// Anything that closes, merges, or marks-ready a PR is intentionally absent.
const ALLOWED_GH_SUBCOMMANDS = new Set(['view', 'comment', 'edit']);

const BLOCKED_PATH_SEGMENTS = new Set(['.git', 'node_modules', '.pnpm-store', '.next', '.turbo']);
const DEFAULT_READ_MAX_BYTES = 48_000;
const MAX_READ_BYTES = 160_000;
const MAX_WRITE_BYTES = 500_000;
const DEFAULT_LIST_MAX_FILES = 250;
const MAX_LIST_FILES = 1000;
const DEFAULT_LIST_MAX_DEPTH = 6;
const MAX_LIST_DEPTH = 12;

type Emit = (event: AgentRunnerEvent) => void;

function toolLog(emit: Emit, text: string): void {
  emit({ kind: 'stderr', text });
}

function pathSegmentAllowed(segment: string): boolean {
  return segment !== '' && segment !== '.' && segment !== '..' && !BLOCKED_PATH_SEGMENTS.has(segment);
}

function safeWorkspacePath(cwd: string, rawPath: string): { absolutePath: string; relativePath: string } {
  const input = rawPath.trim();
  if (!input) {
    throw new Error('path is required');
  }
  if (input.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(input)) {
    throw new Error('absolute paths are not allowed');
  }

  const absoluteCwd = resolve(cwd);
  const absolutePath = resolve(absoluteCwd, input);
  const relativePath = relative(absoluteCwd, absolutePath);
  if (relativePath.startsWith('..') || relativePath.includes(`..${sep}`)) {
    throw new Error(`path escapes workspace: ${rawPath}`);
  }
  for (const segment of relativePath.split(/[\\/]+/)) {
    if (segment && !pathSegmentAllowed(segment)) {
      throw new Error(`path segment is not allowed: ${segment}`);
    }
  }
  return { absolutePath, relativePath: relativePath || '.' };
}

function clampPositiveInt(value: number | undefined, fallback: number, max: number): number {
  if (!Number.isFinite(value) || !value || value <= 0) {
    return fallback;
  }
  return Math.min(Math.trunc(value), max);
}

function decodeUtf8Prefix(buffer: Buffer, maxBytes: number): { text: string; truncated: boolean } {
  const truncated = buffer.byteLength > maxBytes;
  return {
    text: buffer.subarray(0, Math.min(buffer.byteLength, maxBytes)).toString('utf8'),
    truncated,
  };
}

async function walkFiles(input: {
  cwd: string;
  rootPath: string;
  query?: string;
  maxFiles: number;
  maxDepth: number;
}): Promise<string[]> {
  const out: string[] = [];
  const query = input.query?.trim().toLowerCase();

  async function visit(absoluteDir: string, depth: number): Promise<void> {
    if (out.length >= input.maxFiles || depth > input.maxDepth) {
      return;
    }
    const entries = await readdir(absoluteDir, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (out.length >= input.maxFiles) {
        return;
      }
      if (!pathSegmentAllowed(entry.name)) {
        continue;
      }
      const absolutePath = resolve(absoluteDir, entry.name);
      const relativePath = relative(input.cwd, absolutePath);
      if (entry.isDirectory()) {
        await visit(absolutePath, depth + 1);
        continue;
      }
      if (!entry.isFile()) {
        continue;
      }
      if (query && !relativePath.toLowerCase().includes(query)) {
        continue;
      }
      out.push(relativePath);
    }
  }

  await visit(input.rootPath, 0);
  return out;
}

export function createWorkspaceTools(cwd: string, emit: Emit): ToolSet {
  return {
    list_files: tool({
      description:
        'List workspace files. Use this before reading files. Never touches .git, node_modules, build caches, or paths outside the repo.',
      inputSchema: z.object({
        path: z.string().default('.').describe('Workspace-relative directory to list.'),
        query: z.string().optional().describe('Optional case-insensitive substring filter.'),
        maxFiles: z.number().int().positive().max(MAX_LIST_FILES).optional(),
        maxDepth: z.number().int().positive().max(MAX_LIST_DEPTH).optional(),
      }),
      execute: async ({ path, query, maxFiles, maxDepth }) => {
        const safe = safeWorkspacePath(cwd, path);
        const files = await walkFiles({
          cwd: resolve(cwd),
          rootPath: safe.absolutePath,
          query,
          maxFiles: clampPositiveInt(maxFiles, DEFAULT_LIST_MAX_FILES, MAX_LIST_FILES),
          maxDepth: clampPositiveInt(maxDepth, DEFAULT_LIST_MAX_DEPTH, MAX_LIST_DEPTH),
        });
        toolLog(emit, `workspace-tool:list_files ${safe.relativePath} -> ${files.length} file(s)`);
        return { ok: true, files };
      },
    }),

    read_file: tool({
      description:
        'Read a UTF-8 text file from the workspace. Use before editing so changes preserve existing content.',
      inputSchema: z.object({
        path: z.string().describe('Workspace-relative file path.'),
        maxBytes: z.number().int().positive().max(MAX_READ_BYTES).optional(),
      }),
      execute: async ({ path, maxBytes }) => {
        const safe = safeWorkspacePath(cwd, path);
        const buffer = await readFile(safe.absolutePath);
        const limit = clampPositiveInt(maxBytes, DEFAULT_READ_MAX_BYTES, MAX_READ_BYTES);
        const { text, truncated } = decodeUtf8Prefix(buffer, limit);
        toolLog(emit, `workspace-tool:read_file ${safe.relativePath} bytes=${buffer.byteLength}`);
        return { ok: true, path: safe.relativePath, text, truncated, bytes: buffer.byteLength };
      },
    }),

    write_file: tool({
      description:
        'Create or replace a UTF-8 text file inside the workspace. Prefer replace_in_file for small targeted edits.',
      inputSchema: z.object({
        path: z.string().describe('Workspace-relative file path.'),
        content: z.string().max(MAX_WRITE_BYTES).describe('Full UTF-8 file content to write.'),
      }),
      execute: async ({ path, content }) => {
        const safe = safeWorkspacePath(cwd, path);
        await mkdir(dirname(safe.absolutePath), { recursive: true });
        await writeFile(safe.absolutePath, content, 'utf8');
        toolLog(emit, `workspace-tool:write_file ${safe.relativePath} bytes=${Buffer.byteLength(content)}`);
        return { ok: true, path: safe.relativePath, bytes: Buffer.byteLength(content) };
      },
    }),

    replace_in_file: tool({
      description:
        'Replace exact text in a UTF-8 workspace file. Fails if the search text is absent. Use for focused edits.',
      inputSchema: z.object({
        path: z.string().describe('Workspace-relative file path.'),
        search: z.string().min(1).describe('Exact text to replace.'),
        replacement: z.string().describe('Replacement text.'),
        replaceAll: z.boolean().default(false).describe('Replace every occurrence instead of only the first.'),
      }),
      execute: async ({ path, search, replacement, replaceAll }) => {
        const safe = safeWorkspacePath(cwd, path);
        const before = await readFile(safe.absolutePath, 'utf8');
        if (!before.includes(search)) {
          throw new Error(`search text not found in ${safe.relativePath}`);
        }
        const after = replaceAll ? before.split(search).join(replacement) : before.replace(search, replacement);
        await writeFile(safe.absolutePath, after, 'utf8');
        const replacements = replaceAll ? before.split(search).length - 1 : 1;
        toolLog(emit, `workspace-tool:replace_in_file ${safe.relativePath} replacements=${replacements}`);
        return { ok: true, path: safe.relativePath, replacements };
      },
    }),

    append_file: tool({
      description: 'Append UTF-8 text to a workspace file, creating parent directories when needed.',
      inputSchema: z.object({
        path: z.string().describe('Workspace-relative file path.'),
        text: z.string().max(MAX_WRITE_BYTES).describe('Text to append.'),
      }),
      execute: async ({ path, text }) => {
        const safe = safeWorkspacePath(cwd, path);
        await mkdir(dirname(safe.absolutePath), { recursive: true });
        const current = await readFile(safe.absolutePath, 'utf8').catch(() => '');
        await writeFile(safe.absolutePath, `${current}${text}`, 'utf8');
        toolLog(emit, `workspace-tool:append_file ${safe.relativePath} bytes=${Buffer.byteLength(text)}`);
        return { ok: true, path: safe.relativePath, appendedBytes: Buffer.byteLength(text) };
      },
    }),

    workspace_status: tool({
      description: 'Show git status and diff summary for the current workspace. This is read-only.',
      inputSchema: z.object({}),
      execute: async () => {
        const [status, diffStat] = await Promise.all([
          execFileAsync('git', ['status', '--short'], { cwd }),
          execFileAsync('git', ['diff', '--stat'], { cwd }),
        ]);
        toolLog(emit, 'workspace-tool:workspace_status');
        return {
          ok: true,
          status: status.stdout.trim(),
          diffStat: diffStat.stdout.trim(),
        };
      },
    }),

    pr_view: tool({
      description:
        'Read metadata for the draft pull request opened from the current branch (number, title, body, url, state, isDraft). Use this when the user asks about an existing PR before commenting or editing it.',
      inputSchema: z.object({}),
      execute: async () => githubPrAction(cwd, emit, 'view', []),
    }),

    pr_comment: tool({
      description:
        'Post a real comment on the GitHub pull request that is open from this workspace branch. The agent does not have GH credentials directly; this tool runs server-side with the worker token. Use this — not append_file — when the user asks for a PR comment.',
      inputSchema: z.object({
        body: z
          .string()
          .min(1)
          .max(PR_COMMENT_MAX_BYTES)
          .describe('Markdown body of the PR comment.'),
      }),
      execute: async ({ body }) =>
        githubPrAction(cwd, emit, 'comment', ['--body', body]),
    }),

    pr_update_body: tool({
      description:
        'Replace the body (description) of the draft pull request open from this workspace branch. Title is left unchanged. Never marks the PR ready or merges; use the user-facing UI for that.',
      inputSchema: z.object({
        body: z
          .string()
          .min(1)
          .max(PR_BODY_MAX_BYTES)
          .describe('Replacement Markdown body.'),
      }),
      execute: async ({ body }) =>
        githubPrAction(cwd, emit, 'edit', ['--body', body]),
    }),
  };
}

async function githubPrAction(
  cwd: string,
  emit: Emit,
  subcommand: 'view' | 'comment' | 'edit',
  extraArgs: string[],
): Promise<{
  ok: true;
  subcommand: string;
  url?: string;
  number?: number;
  title?: string;
  state?: string;
  isDraft?: boolean;
  body?: string;
  message: string;
}> {
  if (!ALLOWED_GH_SUBCOMMANDS.has(subcommand)) {
    throw new Error(`gh pr ${subcommand} is not allowed by the workspace-tools allowlist`);
  }
  const ghToken = process.env.GH_PAT;
  if (!ghToken) {
    throw new Error(
      'pr_comment / pr_update_body / pr_view require GH_PAT on the worker — surface this in the final summary instead of editing files',
    );
  }
  if (extraArgs.some((arg) => /^--(merge|squash|rebase|delete-branch|ready|reopen|close)$/.test(arg))) {
    throw new Error('refused: PR merge / close / ready flags are not exposed to agent tools');
  }

  let prMeta: { url?: string; number?: number; title?: string; state?: string; isDraft?: boolean; body?: string } = {};
  const ghEnv = { ...process.env, GH_TOKEN: ghToken };
  try {
    const viewArgs = ['pr', 'view', '--json', 'url,number,title,state,isDraft,body'];
    const { stdout } = await execFileAsync('gh', viewArgs, {
      cwd,
      env: ghEnv,
      timeout: GH_TIMEOUT_MS,
    });
    prMeta = JSON.parse(stdout) as typeof prMeta;
  } catch (err) {
    throw new Error(
      `gh pr view failed (is there an open PR for this branch?): ${err instanceof Error ? err.message : String(err)}`,
    );
  }

  if (subcommand === 'view') {
    toolLog(emit, `workspace-tool:pr_view #${prMeta.number ?? '?'} state=${prMeta.state ?? 'unknown'}`);
    return {
      ok: true,
      subcommand,
      ...prMeta,
      message: `pr_view ok for #${prMeta.number ?? 'unknown'}`,
    };
  }

  if (prMeta.state && prMeta.state !== 'OPEN') {
    throw new Error(
      `refused: PR is in state ${prMeta.state}; agent tools only modify open draft PRs`,
    );
  }
  if (subcommand === 'edit' && prMeta.isDraft === false) {
    throw new Error(
      'refused: PR has been marked ready by a human reviewer; agent tools do not edit non-draft PRs',
    );
  }

  try {
    const ghArgs = ['pr', subcommand, ...extraArgs];
    await execFileAsync('gh', ghArgs, {
      cwd,
      env: ghEnv,
      timeout: GH_TIMEOUT_MS,
      maxBuffer: 1_000_000,
    });
  } catch (err) {
    throw new Error(
      `gh pr ${subcommand} failed: ${err instanceof Error ? err.message : String(err)}`,
    );
  }

  toolLog(
    emit,
    `workspace-tool:pr_${subcommand} #${prMeta.number ?? '?'} bytes=${extraArgs.find((_, i) => extraArgs[i - 1] === '--body')?.length ?? 0}`,
  );
  return {
    ok: true,
    subcommand,
    ...prMeta,
    message: `pr_${subcommand} ok for #${prMeta.number ?? 'unknown'}`,
  };
}
