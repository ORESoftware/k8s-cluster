const GOOGLE_CHAT_EXPORT = Object.freeze({
  spaceName: 'spaces/AAQAoHKdzvI',
  spaceId: 'AAQAoHKdzvI',
  // May 10, 2026 00:00:00 America/New_York is 2026-05-10T04:00:00Z.
  // The Chat API supports `>` rather than `>=`, so use one millisecond earlier.
  filter: 'createTime > "2026-05-10T03:59:59.999Z"',
  pageSize: 1000,
  stateKey: 'google_chat_export_state_v1',
  continuationFunction: 'continueGoogleChatExport',
});

/**
 * Starts a new resumable export of the alex-alex-me Google Chat space.
 *
 * Run this function manually once. It creates a private Drive folder and exports
 * one API page per JSON file. If more pages remain, a time-based trigger resumes
 * the export without retaining message text in logs or Script Properties.
 */
function startGoogleChatExport() {
  const lock = LockService.getScriptLock();
  lock.waitLock(30000);
  try {
    removeContinuationTriggers_();

    const startedAt = new Date();
    const runId = Utilities.formatDate(startedAt, 'UTC', 'yyyyMMdd-HHmmss');
    const folderName = `google-chat-${GOOGLE_CHAT_EXPORT.spaceId}-from-2026-05-10-${runId}`;
    const folder = DriveApp.createFolder(folderName);

    const state = {
      version: 1,
      runId,
      folderId: folder.getId(),
      folderUrl: folder.getUrl(),
      pageToken: null,
      partNumber: 0,
      totalMessages: 0,
      startedAt: startedAt.toISOString(),
      completedAt: null,
      cancelledAt: null,
    };

    saveState_(state);
    replaceJsonFile_(folder, 'export-config.json', {
      version: state.version,
      runId: state.runId,
      spaceName: GOOGLE_CHAT_EXPORT.spaceName,
      spaceId: GOOGLE_CHAT_EXPORT.spaceId,
      startTimeInclusive: '2026-05-10T04:00:00.000Z',
      startDateInterpretation: 'May 10, 2026 at midnight America/New_York',
      filter: GOOGLE_CHAT_EXPORT.filter,
      startedAt: state.startedAt,
      sourceUrl: 'https://chat.google.com/room/AAQAoHKdzvI?cls=5',
    });

    console.log(`Google Chat export started. Drive folder: ${folder.getUrl()}`);
  } finally {
    lock.releaseLock();
  }

  continueGoogleChatExport();
}

/**
 * Exports one page and schedules the next page when needed.
 * Safe to rerun: part files are deterministically replaced before state advances,
 * and concurrent invocations are serialized with a script lock.
 */
function continueGoogleChatExport() {
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(30000)) {
    throw new Error('Another export invocation is already running.');
  }

  try {
    removeContinuationTriggers_();
    const state = loadState_();
    if (!state) {
      throw new Error('No export is active. Run startGoogleChatExport() first.');
    }
    if (state.cancelledAt) {
      console.log(`Export was cancelled at ${state.cancelledAt}. Drive folder: ${state.folderUrl}`);
      return;
    }
    if (state.completedAt) {
      console.log(`Export already completed. Drive folder: ${state.folderUrl}`);
      return;
    }

    const folder = DriveApp.getFolderById(state.folderId);
    let response;
    try {
      response = Chat.Spaces.Messages.list(GOOGLE_CHAT_EXPORT.spaceName, {
        pageSize: GOOGLE_CHAT_EXPORT.pageSize,
        pageToken: state.pageToken || undefined,
        filter: GOOGLE_CHAT_EXPORT.filter,
        orderBy: 'createTime ASC',
        showDeleted: true,
      });
    } catch (error) {
      replaceJsonFile_(folder, 'export-error.json', {
        failedAt: new Date().toISOString(),
        message: String(error && error.message ? error.message : error),
        guidance:
          'Confirm the Advanced Google Chat service is enabled, the Chat API is enabled in the linked Cloud project, and the authenticated account can access the space.',
      });
      throw error;
    }

    const messages = (response.messages || []).map(normalizeMessage_);
    const nextPartNumber = Number(state.partNumber) + 1;
    const partName = `messages-part-${String(nextPartNumber).padStart(5, '0')}.json`;

    replaceJsonFile_(folder, partName, {
      version: 1,
      runId: state.runId,
      spaceName: GOOGLE_CHAT_EXPORT.spaceName,
      filter: GOOGLE_CHAT_EXPORT.filter,
      partNumber: nextPartNumber,
      messageCount: messages.length,
      exportedAt: new Date().toISOString(),
      messages,
    });

    state.partNumber = nextPartNumber;
    state.totalMessages = Number(state.totalMessages) + messages.length;
    state.pageToken = response.nextPageToken || null;
    saveState_(state);

    if (state.pageToken) {
      ScriptApp.newTrigger(GOOGLE_CHAT_EXPORT.continuationFunction)
        .timeBased()
        .after(60 * 1000)
        .create();
      console.log(
        `Exported part ${nextPartNumber} (${messages.length} messages). Continuation scheduled.`,
      );
      return;
    }

    state.completedAt = new Date().toISOString();
    saveState_(state);
    replaceJsonFile_(folder, 'export-summary.json', {
      version: 1,
      runId: state.runId,
      spaceName: GOOGLE_CHAT_EXPORT.spaceName,
      spaceId: GOOGLE_CHAT_EXPORT.spaceId,
      filter: GOOGLE_CHAT_EXPORT.filter,
      startedAt: state.startedAt,
      completedAt: state.completedAt,
      parts: state.partNumber,
      totalMessages: state.totalMessages,
      folderUrl: state.folderUrl,
      importKeyPrefix: `google-chat:${GOOGLE_CHAT_EXPORT.spaceId}:`,
    });

    removeContinuationTriggers_();
    console.log(
      `Google Chat export complete: ${state.totalMessages} messages in ${state.partNumber} part(s). Drive folder: ${state.folderUrl}`,
    );
  } finally {
    lock.releaseLock();
  }
}

/** Returns non-sensitive progress metadata; message text is never returned. */
function getGoogleChatExportStatus() {
  const state = loadState_();
  if (!state) return { active: false };
  return {
    active: !state.completedAt && !state.cancelledAt,
    runId: state.runId,
    folderUrl: state.folderUrl,
    parts: state.partNumber,
    totalMessages: state.totalMessages,
    startedAt: state.startedAt,
    completedAt: state.completedAt,
    cancelledAt: state.cancelledAt,
  };
}

/** Cancels future continuation triggers but leaves already exported files intact. */
function cancelGoogleChatExport() {
  removeContinuationTriggers_();
  const state = loadState_();
  if (state) {
    state.cancelledAt = new Date().toISOString();
    saveState_(state);
  }
  console.log('Google Chat export continuation cancelled. Existing Drive files were retained.');
}

function normalizeMessage_(message) {
  const name = message.name || '';
  const threadName = message.thread && message.thread.name ? message.thread.name : null;
  return {
    sourceKey: `google-chat:${GOOGLE_CHAT_EXPORT.spaceId}:${name || message.createTime || 'unknown'}`,
    name,
    sender: message.sender
      ? {
          name: message.sender.name || null,
          type: message.sender.type || null,
          displayName: message.sender.displayName || null,
        }
      : null,
    createTime: message.createTime || null,
    lastUpdateTime: message.lastUpdateTime || null,
    deleteTime: message.deleteTime || null,
    text: message.text || '',
    formattedText: message.formattedText || '',
    argumentText: message.argumentText || '',
    fallbackText: message.fallbackText || '',
    thread: threadName
      ? {
          name: threadName,
          sourceKey: `google-chat:${GOOGLE_CHAT_EXPORT.spaceId}:${threadName}`,
        }
      : null,
    threadReply: Boolean(message.threadReply),
    silent: Boolean(message.silent),
    annotations: message.annotations || [],
    attachments: message.attachment || [],
    matchedUrl: message.matchedUrl || null,
    emojiReactionSummaries: message.emojiReactionSummaries || [],
    deletionMetadata: message.deletionMetadata || null,
    quotedMessageMetadata: message.quotedMessageMetadata || null,
    attachedGifs: message.attachedGifs || [],
  };
}

function replaceJsonFile_(folder, filename, value) {
  const existing = folder.getFilesByName(filename);
  while (existing.hasNext()) {
    existing.next().setTrashed(true);
  }
  const json = JSON.stringify(value, null, 2);
  const blob = Utilities.newBlob(json, 'application/json', filename);
  folder.createFile(blob);
}

function loadState_() {
  const raw = PropertiesService.getScriptProperties().getProperty(
    GOOGLE_CHAT_EXPORT.stateKey,
  );
  return raw ? JSON.parse(raw) : null;
}

function saveState_(state) {
  PropertiesService.getScriptProperties().setProperty(
    GOOGLE_CHAT_EXPORT.stateKey,
    JSON.stringify(state),
  );
}

function removeContinuationTriggers_() {
  for (const trigger of ScriptApp.getProjectTriggers()) {
    if (trigger.getHandlerFunction() === GOOGLE_CHAT_EXPORT.continuationFunction) {
      ScriptApp.deleteTrigger(trigger);
    }
  }
}
