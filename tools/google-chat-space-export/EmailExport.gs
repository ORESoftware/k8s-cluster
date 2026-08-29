/*
 * Resumable Google Chat export delivered to the connected Gmail inbox.
 *
 * This file intentionally has no HTTP entry point. Run
 * `startEmailGoogleChatExport()` manually in the Apps Script editor. Each API
 * page is sent as one JSON attachment to alexander.d.mills@gmail.com. The
 * downstream importer can read those attachments through the connected Gmail
 * app without exposing the bridge token in a URL, source file, or CI log.
 */

'use strict';

const CHAT_EMAIL_EXPORT = Object.freeze({
  version: 1,
  recipient: 'alexander.d.mills@gmail.com',
  stateProperty: 'CHAT_EMAIL_EXPORT_STATE_V1',
  continuationFunction: 'continueEmailGoogleChatExport',
  pageSize: 100,
  maxParts: 200,
  continuationDelayMs: 60 * 1000,
  subjectPrefix: '[Google Chat export AAQAoHKdzvI]',
});

/**
 * Starts a new export and immediately sends the first page.
 *
 * The state contains pagination metadata only; message bodies live solely in
 * the JSON email attachments. Running this again starts a new run ID and makes
 * prior emails easy to distinguish and deduplicate.
 */
function startEmailGoogleChatExport() {
  const lock = LockService.getScriptLock();
  lock.waitLock(30000);

  let state;
  try {
    removeEmailExportTriggers_();
    const probe = probeChatAccess_();
    const now = new Date();
    const runId = Utilities.formatDate(now, 'UTC', 'yyyyMMdd-HHmmss');

    state = {
      version: CHAT_EMAIL_EXPORT.version,
      runId,
      recipient: CHAT_EMAIL_EXPORT.recipient,
      pageToken: null,
      partNumber: 0,
      totalMessages: 0,
      startedAt: now.toISOString(),
      completedAt: null,
      cancelledAt: null,
      lastSentAt: null,
      pendingPart: null,
      probe: {
        checkedAt: probe.checkedAt,
        spaceName: probe.space && probe.space.name,
        displayName: probe.space && probe.space.displayName,
      },
    };

    saveEmailExportState_(state);
    console.log(
      'Google Chat email export started. runId=' +
        runId +
        ' recipient=' +
        CHAT_EMAIL_EXPORT.recipient,
    );
  } finally {
    lock.releaseLock();
  }

  return continueEmailGoogleChatExport();
}

/**
 * Sends one page and schedules the next page when needed.
 *
 * A crash after MailApp succeeds but before state persistence can resend the
 * same runId/part number. That is safer than silently skipping data; consumers
 * must deduplicate by runId + partNumber + filename.
 */
function continueEmailGoogleChatExport() {
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(30000)) {
    throw new Error('Another Google Chat email-export invocation is running.');
  }

  try {
    removeEmailExportTriggers_();
    const state = loadEmailExportState_();
    if (!state) {
      throw new Error('No email export exists. Run startEmailGoogleChatExport() first.');
    }
    if (state.cancelledAt) {
      console.log('Email export is cancelled: ' + state.cancelledAt);
      return emailExportStatusFromState_(state);
    }
    if (state.completedAt) {
      console.log('Email export is already complete: ' + state.completedAt);
      return emailExportStatusFromState_(state);
    }
    if (Number(state.partNumber) >= CHAT_EMAIL_EXPORT.maxParts) {
      throw new Error(
        'Email export reached the safety limit of ' + CHAT_EMAIL_EXPORT.maxParts + ' parts.',
      );
    }

    const nextPartNumber = Number(state.partNumber) + 1;
    const pageTokenBefore = state.pageToken || '';
    state.pendingPart = {
      partNumber: nextPartNumber,
      pageTokenBefore,
      preparedAt: new Date().toISOString(),
    };
    saveEmailExportState_(state);

    const page = listMessagePage_({
      pageToken: pageTokenBefore,
      pageSize: CHAT_EMAIL_EXPORT.pageSize,
      metadataOnly: false,
      showDeleted: true,
      threadName: '',
    });

    const filename =
      'google-chat-' +
      CHAT_BRIDGE.spaceId +
      '-' +
      state.runId +
      '-part-' +
      padEmailExportPart_(nextPartNumber) +
      '.json';
    const exportedAt = new Date().toISOString();
    const payload = {
      exportVersion: CHAT_EMAIL_EXPORT.version,
      runId: state.runId,
      partNumber: nextPartNumber,
      exportedAt,
      recipient: CHAT_EMAIL_EXPORT.recipient,
      source: publicConfig_(),
      filter: page.filter,
      pageSize: page.pageSize,
      messageCount: page.messageCount,
      messages: page.messages,
      nextPageToken: page.nextPageToken,
      dedupeKey:
        'google-chat-email-export:' +
        CHAT_BRIDGE.spaceId +
        ':' +
        state.runId +
        ':' +
        padEmailExportPart_(nextPartNumber),
    };

    sendEmailExportPart_(state, filename, payload);

    state.partNumber = nextPartNumber;
    state.totalMessages = Number(state.totalMessages) + Number(page.messageCount || 0);
    state.pageToken = page.nextPageToken || null;
    state.lastSentAt = exportedAt;
    state.pendingPart = null;
    saveEmailExportState_(state);

    if (state.pageToken) {
      ScriptApp.newTrigger(CHAT_EMAIL_EXPORT.continuationFunction)
        .timeBased()
        .after(CHAT_EMAIL_EXPORT.continuationDelayMs)
        .create();
      console.log(
        'Sent part ' +
          nextPartNumber +
          ' with ' +
          page.messageCount +
          ' messages; continuation scheduled.',
      );
      return emailExportStatusFromState_(state);
    }

    state.completedAt = new Date().toISOString();
    saveEmailExportState_(state);
    removeEmailExportTriggers_();
    sendEmailExportSummary_(state);
    console.log(
      'Google Chat email export complete. runId=' +
        state.runId +
        ' parts=' +
        state.partNumber +
        ' messages=' +
        state.totalMessages,
    );
    return emailExportStatusFromState_(state);
  } finally {
    lock.releaseLock();
  }
}

/** Returns non-sensitive progress metadata. */
function getEmailGoogleChatExportStatus() {
  const state = loadEmailExportState_();
  return state ? emailExportStatusFromState_(state) : { active: false, configured: false };
}

/** Stops future continuation triggers while preserving emails already sent. */
function cancelEmailGoogleChatExport() {
  const lock = LockService.getScriptLock();
  lock.waitLock(30000);
  try {
    removeEmailExportTriggers_();
    const state = loadEmailExportState_();
    if (!state) return { active: false, cancelled: false };
    state.cancelledAt = new Date().toISOString();
    saveEmailExportState_(state);
    return emailExportStatusFromState_(state);
  } finally {
    lock.releaseLock();
  }
}

/** Clears local pagination state after the import is reconciled. */
function clearEmailGoogleChatExportState() {
  removeEmailExportTriggers_();
  PropertiesService.getScriptProperties().deleteProperty(CHAT_EMAIL_EXPORT.stateProperty);
  return { ok: true, clearedAt: new Date().toISOString() };
}

function sendEmailExportPart_(state, filename, payload) {
  const json = JSON.stringify(payload, null, 2);
  const blob = Utilities.newBlob(json, 'application/json', filename);
  const subject =
    CHAT_EMAIL_EXPORT.subjectPrefix +
    ' run ' +
    state.runId +
    ' part ' +
    padEmailExportPart_(payload.partNumber);
  const body = [
    'Google Chat export page for duplicate-safe Linear ingestion.',
    '',
    'Space: ' + CHAT_BRIDGE.spaceName + ' (' + CHAT_BRIDGE.expectedDisplayName + ')',
    'Start boundary: ' + CHAT_BRIDGE.startTimeInclusive,
    'Run ID: ' + state.runId,
    'Part: ' + payload.partNumber,
    'Messages: ' + payload.messageCount,
    'Attachment: ' + filename,
    '',
    'The email body intentionally excludes Chat message content.',
  ].join('\n');

  MailApp.sendEmail({
    to: CHAT_EMAIL_EXPORT.recipient,
    subject,
    body,
    name: 'Google Chat Linear Bridge',
    attachments: [blob],
  });
}

function sendEmailExportSummary_(state) {
  const subject = CHAT_EMAIL_EXPORT.subjectPrefix + ' run ' + state.runId + ' COMPLETE';
  const body = [
    'Google Chat export completed.',
    '',
    'Space: ' + CHAT_BRIDGE.spaceName + ' (' + CHAT_BRIDGE.expectedDisplayName + ')',
    'Start boundary: ' + CHAT_BRIDGE.startTimeInclusive,
    'Run ID: ' + state.runId,
    'Parts: ' + state.partNumber,
    'Messages: ' + state.totalMessages,
    'Started: ' + state.startedAt,
    'Completed: ' + state.completedAt,
    '',
    'After the Linear import succeeds, rotateBridgeToken() or disableBridge().',
  ].join('\n');

  MailApp.sendEmail({
    to: CHAT_EMAIL_EXPORT.recipient,
    subject,
    body,
    name: 'Google Chat Linear Bridge',
  });
}

function emailExportStatusFromState_(state) {
  return {
    active: !state.completedAt && !state.cancelledAt,
    configured: true,
    runId: state.runId,
    recipient: state.recipient,
    parts: Number(state.partNumber) || 0,
    totalMessages: Number(state.totalMessages) || 0,
    startedAt: state.startedAt,
    lastSentAt: state.lastSentAt,
    completedAt: state.completedAt,
    cancelledAt: state.cancelledAt,
    hasMorePages: Boolean(state.pageToken),
    pendingPart: state.pendingPart
      ? {
          partNumber: state.pendingPart.partNumber,
          preparedAt: state.pendingPart.preparedAt,
        }
      : null,
  };
}

function loadEmailExportState_() {
  const raw = PropertiesService.getScriptProperties().getProperty(
    CHAT_EMAIL_EXPORT.stateProperty,
  );
  return raw ? JSON.parse(raw) : null;
}

function saveEmailExportState_(state) {
  PropertiesService.getScriptProperties().setProperty(
    CHAT_EMAIL_EXPORT.stateProperty,
    JSON.stringify(state),
  );
}

function removeEmailExportTriggers_() {
  const triggers = ScriptApp.getProjectTriggers();
  for (let index = 0; index < triggers.length; index += 1) {
    if (triggers[index].getHandlerFunction() === CHAT_EMAIL_EXPORT.continuationFunction) {
      ScriptApp.deleteTrigger(triggers[index]);
    }
  }
}

function padEmailExportPart_(partNumber) {
  return ('00000' + String(partNumber)).slice(-5);
}
