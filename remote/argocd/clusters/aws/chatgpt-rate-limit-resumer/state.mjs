import { createHash } from 'node:crypto';
import {
  chmod,
  mkdir,
  open,
  readFile,
  rename,
  stat,
  unlink,
  writeFile,
} from 'node:fs/promises';
import process from 'node:process';
import {
  RUN_ID,
  RUN_LOCK_PATH,
  RUN_STATE_PATH,
  SEED_HASH_PATH,
  SEED_PATH,
  STATE_DIR,
  STORAGE_STATE_PATH,
  config,
  errorCode,
  log,
} from './runtime.mjs';

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if (error?.code === 'ENOENT') return false;
    throw error;
  }
}

function parseStorageState(raw, source) {
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(`${source} is not valid JSON: ${error.message}`);
  }

  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`${source} must be a Playwright storage-state object`);
  }
  if (!Array.isArray(parsed.cookies) || !Array.isArray(parsed.origins)) {
    throw new Error(`${source} must contain cookies[] and origins[]`);
  }
  return parsed;
}

async function writeTextAtomic(path, value, mode = 0o600) {
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  await writeFile(temporary, value, { encoding: 'utf8', mode });
  await rename(temporary, path);
  await chmod(path, mode);
}

export async function writeJsonAtomic(path, value) {
  await writeTextAtomic(path, `${JSON.stringify(value, null, 2)}\n`);
}

export async function acquireRunLock() {
  await mkdir(STATE_DIR, { recursive: true, mode: 0o700 });

  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const handle = await open(RUN_LOCK_PATH, 'wx', 0o600);
      await handle.writeFile(
        `${JSON.stringify({ runId: RUN_ID, startedAt: new Date().toISOString() })}\n`,
        'utf8',
      );
      await handle.sync();

      let released = false;
      return async () => {
        if (released) return;
        released = true;
        await handle.close().catch(() => {});

        try {
          const lock = JSON.parse(await readFile(RUN_LOCK_PATH, 'utf8'));
          if (lock?.runId === RUN_ID) await unlink(RUN_LOCK_PATH);
        } catch (error) {
          if (error?.code !== 'ENOENT') {
            log('run_lock_release_failed', { error_code: errorCode(error) });
          }
        }
      };
    } catch (error) {
      if (error?.code !== 'EEXIST') throw error;

      let startedAtMs = Number.NaN;
      try {
        const lock = JSON.parse(await readFile(RUN_LOCK_PATH, 'utf8'));
        startedAtMs = Date.parse(lock?.startedAt ?? '');
      } catch {
        try {
          startedAtMs = (await stat(RUN_LOCK_PATH)).mtimeMs;
        } catch (statError) {
          if (statError?.code === 'ENOENT') continue;
          throw statError;
        }
      }

      if (Number.isFinite(startedAtMs) && Date.now() - startedAtMs >= config.lockStaleMs) {
        const stalePath = `${RUN_LOCK_PATH}.stale.${Date.now()}.${process.pid}`;
        try {
          await rename(RUN_LOCK_PATH, stalePath);
          await unlink(stalePath).catch(() => {});
          log('stale_run_lock_reaped');
          continue;
        } catch (renameError) {
          if (renameError?.code === 'ENOENT') continue;
          throw renameError;
        }
      }

      const concurrent = new Error('Another ChatGPT resumer run owns the state lock.');
      concurrent.exitCode = 75;
      throw concurrent;
    }
  }

  const unavailable = new Error('Unable to acquire the ChatGPT resumer state lock.');
  unavailable.exitCode = 75;
  throw unavailable;
}

export async function prepareStorageState() {
  await mkdir(STATE_DIR, { recursive: true, mode: 0o700 });

  const hasSeed = await exists(SEED_PATH);
  const hasCurrent = await exists(STORAGE_STATE_PATH);

  let seedHash = null;
  let seedRaw = null;
  if (hasSeed) {
    seedRaw = await readFile(SEED_PATH, 'utf8');
    parseStorageState(seedRaw, SEED_PATH);
    seedHash = createHash('sha256').update(seedRaw).digest('hex');
  }

  let appliedSeedHash = null;
  if (await exists(SEED_HASH_PATH)) {
    appliedSeedHash = (await readFile(SEED_HASH_PATH, 'utf8')).trim();
  }

  if (seedRaw && (!hasCurrent || seedHash !== appliedSeedHash)) {
    await writeTextAtomic(STORAGE_STATE_PATH, `${JSON.stringify(parseStorageState(seedRaw, SEED_PATH))}\n`);
    await writeTextAtomic(SEED_HASH_PATH, `${seedHash}\n`);
    log('auth_state_seeded', { reason: hasCurrent ? 'seed_rotated' : 'initial_seed' });
  }

  if (!(await exists(STORAGE_STATE_PATH))) {
    const error = new Error(
      'No ChatGPT storage state is available. Add CHATGPT_STORAGE_STATE_JSON to dd/remote-dev/agent-secrets.',
    );
    error.exitCode = 78;
    throw error;
  }

  parseStorageState(await readFile(STORAGE_STATE_PATH, 'utf8'), STORAGE_STATE_PATH);
  return STORAGE_STATE_PATH;
}

function emptyRunState() {
  return { version: 1, conversations: {}, last_run: null };
}

export async function loadRunState() {
  if (!(await exists(RUN_STATE_PATH))) return emptyRunState();
  try {
    const value = JSON.parse(await readFile(RUN_STATE_PATH, 'utf8'));
    if (!value || value.version !== 1 || typeof value.conversations !== 'object') {
      throw new Error('unexpected state shape');
    }
    return value;
  } catch (error) {
    log('run_state_rejected', { error_code: errorCode(error) });
    return emptyRunState();
  }
}

export function pruneRunState(state, nowMs) {
  for (const [key, record] of Object.entries(state.conversations)) {
    const lastSeenMs = Date.parse(record.lastSeenAt ?? record.lastAttemptAt ?? '');
    if (!Number.isFinite(lastSeenMs) || nowMs - lastSeenMs > config.retentionMs) {
      delete state.conversations[key];
    }
  }
}


export async function saveBrowserState(context) {
  const temporary = `${STORAGE_STATE_PATH}.${process.pid}.tmp`;
  await context.storageState({ path: temporary });
  await rename(temporary, STORAGE_STATE_PATH);
  await chmod(STORAGE_STATE_PATH, 0o600);
}
