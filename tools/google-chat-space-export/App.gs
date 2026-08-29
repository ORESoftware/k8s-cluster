/**
 * Google Chat -> Linear bridge for one fixed Chat space.
 *
 * Target space: alex-alex-me (spaces/AAQAoHKdzvI)
 * Start date:   May 10, 2026 00:00 America/New_York
 *
 * Security model:
 * - Deploy the web app as yourself so Chat API calls use your Google identity.
 * - Sensitive endpoints require a high-entropy bridge token.
 * - Only the SHA-256 hash of that token is stored in Script Properties.
 * - The target space and earliest timestamp are hard-coded; callers cannot widen
 *   access to other spaces or earlier history.
 * - Prefer POST requests. GET is supported for clients that cannot issue POST,
 *   but query-string tokens can appear in access logs; rotate the token after use.
 *
 * Required Advanced Service: Google Chat API, service identifier "Chat", v1.
 */

'use strict';

const CHAT_BRIDGE = Object.freeze({
  version: '1.0.1',
  spaceName: 'spaces/AAQAoHKdzvI',
  spaceId: 'AAQAoHKdzvI',
  expectedDisplayName: 'alex-alex-me',
  sourceUrl: 'https://chat.google.com/room/AAQAoHKdzvI?cls=5',

  // 2026-05-10 00:00:00 America/New_York == 2026-05-10T04:00:00.000Z.
  // spaces.messages.list supports `>` but not `>=`, so subtract one millisecond.
  startTimeInclusive: '2026-05-10T04:00:00.000Z',
  baseFilter: 'createTime > "2026-05-10T03:59:59.999Z"',

  defaultPageSize: 100,
  maxPageSize: 250,
  maxTextCharsPerField: 100000,
  maxRequestsPerMinute: 30,

  tokenHashProperty: 'CHAT_BRIDGE_TOKEN_SHA256',
  tokenCreatedAtProperty: 'CHAT_BRIDGE_TOKEN_CREATED_AT',
  lastRequestProperty: 'CHAT_BRIDGE_LAST_REQUEST_AT',
});

/**
 * Run once from the Apps Script editor after enabling the Google Chat advanced
 * service. It verifies access and creates a temporary bridge token.
 *
 * Copy the token from the execution log. Only its SHA-256 hash is retained.
 */
function setupBridge() {
  const probe = probeChatAccess_();
  const bridgeToken = generateBridgeToken_();
  storeBridgeToken_(bridgeToken);

  console.log('CHAT_BRIDGE_TOKEN=' + bridgeToken);
  console.log('Keep this token private and rotate it after the import.');

  return {
    ok: true,
    bridgeToken,
    webAppUrl: safeWebAppUrl_(),
    target: publicConfig_(),
    probe,
  };
}

/** Rotates the bridge token and invalidates the previous token immediately. */
function rotateBridgeToken() {
  const bridgeToken = generateBridgeToken_();
  storeBridgeToken_(bridgeToken);
  console.log('CHAT_BRIDGE_TOKEN=' + bridgeToken);
  console.log('The previous bridge token is now invalid.');
  return { ok: true, bridgeToken, createdAt: new Date().toISOString() };
}

/** Disables all authenticated HTTP access until setupBridge() is run again. */
function disableBridge() {
  const props = PropertiesService.getScriptProperties();
  props.deleteProperty(CHAT_BRIDGE.tokenHashProperty);
  props.deleteProperty(CHAT_BRIDGE.tokenCreatedAtProperty);
  return { ok: true, disabledAt: new Date().toISOString() };
}

/** Manual editor test that returns the first page without exposing the token. */
function testReadFirstPage() {
  return listMessagePage_({ pageSize: 10, metadataOnly: false });
}

/** Public web-app GET handler. */
function doGet(e) {
  return handleHttpRequest_('GET', e || {});
}

/** Public web-app POST handler. */
function doPost(e) {
  return handleHttpRequest_('POST', e || {});
}

function handleHttpRequest_(method, event) {
  const requestId = Utilities.getUuid();
  const startedAt = Date.now();

  try {
    const request = parseHttpRequest_(method, event);
    const action = String(request.action || 'health').toLowerCase();

    if (action === 'health') {
      return jsonOutput_({
        ok: true,
        requestId,
        service: 'google-chat-linear-bridge',
        version: CHAT_BRIDGE.version,
        now: new Date().toISOString(),
        configured: bridgeIsConfigured_(),
      });
    }

    requireBridgeToken_(request.token);
    enforceRateLimit_();

    let data;
    switch (action) {
      case 'status':
        data = bridgeStatus_();
        break;
      case 'probe':
        data = probeChatAccess_();
        break;
      case 'space':
        data = getTargetSpace_();
        break;
      case 'messages':
      case 'page':
        data = listMessagePage_(request);
        break;
      default:
        throw new BridgeError_(
          'unknown_action',
          'Supported actions: health, status, probe, space, messages.',
          400,
        );
    }

    PropertiesService.getScriptProperties().setProperty(
      CHAT_BRIDGE.lastRequestProperty,
      new Date().toISOString(),
    );

    return jsonOutput_({
      ok: true,
      requestId,
      action,
      elapsedMs: Date.now() - startedAt,
      data,
    });
  } catch (error) {
    const normalized = normalizeError_(error);
    console.error(
      JSON.stringify({
        requestId,
        code: normalized.code,
        status: normalized.status,
        message: normalized.message,
      }),
    );

    // Apps Script ContentService does not expose an HTTP status setter, so the
    // semantic status is returned in the JSON envelope.
    return jsonOutput_({
      ok: false,
      requestId,
      elapsedMs: Date.now() - startedAt,
      error: normalized,
    });
  }
}

function parseHttpRequest_(method, event) {
  const query = event.parameter || {};
  let body = {};

  if (method === 'POST' && event.postData && event.postData.contents) {
    const contentType = String(event.postData.type || '').toLowerCase();
    const raw = String(event.postData.contents || '').trim();
    if (raw) {
      if (contentType.indexOf('application/json') !== -1 || raw.charAt(0) === '{') {
        try {
          body = JSON.parse(raw);
        } catch (error) {
          throw new BridgeError_('invalid_json', 'The POST body is not valid JSON.', 400);
        }
      } else {
        body = query;
      }
    }
  }

  const merged = Object.assign({}, query, body);
  return {
    action: merged.action || merged.mode || 'health',
    token: merged.token || merged.key || '',
    pageToken: merged.pageToken || merged.cursor || '',
    pageSize: parseInteger_(merged.pageSize, CHAT_BRIDGE.defaultPageSize),
    metadataOnly: parseBoolean_(merged.metadataOnly, false),
    showDeleted: parseBoolean_(merged.showDeleted, true),
    threadName: merged.threadName || merged.thread || '',
  };
}

function bridgeStatus_() {
  const props = PropertiesService.getScriptProperties();
  return {
    configured: bridgeIsConfigured_(),
    tokenCreatedAt: props.getProperty(CHAT_BRIDGE.tokenCreatedAtProperty),
    lastRequestAt: props.getProperty(CHAT_BRIDGE.lastRequestProperty),
    webAppUrl: safeWebAppUrl_(),
    target: publicConfig_(),
    supportedActions: ['health', 'status', 'probe', 'space', 'messages'],
    preferredTransport: 'POST application/json',
    getWarning: 'GET tokens may appear in URL logs; rotate the token after use.',
  };
}

function probeChatAccess_() {
  const space = Chat.Spaces.get(CHAT_BRIDGE.spaceName);
  const page = Chat.Spaces.Messages.list(CHAT_BRIDGE.spaceName, {
    pageSize: 1,
    filter: CHAT_BRIDGE.baseFilter,
    showDeleted: true,
  });

  return {
    checkedAt: new Date().toISOString(),
    space: normalizeSpace_(space),
    firstPageMessageCount: (page.messages || []).length,
    hasMoreMessages: Boolean(page.nextPageToken),
  };
}

function getTargetSpace_() {
  return normalizeSpace_(Chat.Spaces.get(CHAT_BRIDGE.spaceName));
}

function listMessagePage_(request) {
  const pageSize = clamp_(
    parseInteger_(request.pageSize, CHAT_BRIDGE.defaultPageSize),
    1,
    CHAT_BRIDGE.maxPageSize,
  );
  const pageToken = String(request.pageToken || '').trim();
  const metadataOnly = parseBoolean_(request.metadataOnly, false);
  const showDeleted = parseBoolean_(request.showDeleted, true);
  const threadName = validateThreadName_(request.threadName);
  const filter = threadName
    ? CHAT_BRIDGE.baseFilter + ' AND thread.name = ' + threadName
    : CHAT_BRIDGE.baseFilter;

  const options = {
    pageSize,
    filter,
    showDeleted,
  };
  if (pageToken) options.pageToken = pageToken;

  const response = Chat.Spaces.Messages.list(CHAT_BRIDGE.spaceName, options);
  const messages = (response.messages || []).map(function (message) {
    return normalizeMessage_(message, metadataOnly);
  });

  return {
    target: publicConfig_(),
    filter,
    pageSize,
    messageCount: messages.length,
    metadataOnly,
    showDeleted,
    threadName: threadName || null,
    messages,
    nextPageToken: response.nextPageToken || null,
    nextRequest: response.nextPageToken
      ? {
          action: 'messages',
          pageToken: response.nextPageToken,
          pageSize,
          metadataOnly,
          showDeleted,
          threadName: threadName || undefined,
        }
      : null,
  };
}

function normalizeSpace_(space) {
  return {
    name: space && space.name ? space.name : CHAT_BRIDGE.spaceName,
    displayName: space && space.displayName ? space.displayName : null,
    spaceType: space && space.spaceType ? space.spaceType : null,
    spaceHistoryState: space && space.spaceHistoryState ? space.spaceHistoryState : null,
    threaded: space && typeof space.threaded === 'boolean' ? space.threaded : null,
    sourceUrl: CHAT_BRIDGE.sourceUrl,
    expectedDisplayName: CHAT_BRIDGE.expectedDisplayName,
  };
}

function normalizeMessage_(message, metadataOnly) {
  const name = message && message.name ? String(message.name) : '';
  const threadName =
    message && message.thread && message.thread.name ? String(message.thread.name) : null;

  const result = {
    sourceKey:
      'google-chat:' +
      CHAT_BRIDGE.spaceId +
      ':' +
      (name || (message && message.createTime) || 'unknown'),
    name: name || null,
    spaceName:
      message && message.space && message.space.name
        ? String(message.space.name)
        : CHAT_BRIDGE.spaceName,
    thread: threadName
      ? {
          name: threadName,
          sourceKey: 'google-chat:' + CHAT_BRIDGE.spaceId + ':' + threadName,
        }
      : null,
    threadReply: Boolean(message && message.threadReply),
    sender: message && message.sender
      ? {
          name: message.sender.name || null,
          type: message.sender.type || null,
          displayName: message.sender.displayName || null,
          domainId: message.sender.domainId || null,
        }
      : null,
    createTime: (message && message.createTime) || null,
    lastUpdateTime: (message && message.lastUpdateTime) || null,
    deleteTime: (message && message.deleteTime) || null,
    clientAssignedMessageId: (message && message.clientAssignedMessageId) || null,
    silent: Boolean(message && message.silent),
    matchedUrl: (message && message.matchedUrl) || null,
    deletionMetadata: (message && message.deletionMetadata) || null,
    quotedMessageMetadata: (message && message.quotedMessageMetadata) || null,
    emojiReactionSummaries: (message && message.emojiReactionSummaries) || [],
    annotations: (message && message.annotations) || [],
    attachments: normalizeAttachments_((message && message.attachment) || []),
    attachedGifs: (message && message.attachedGifs) || [],
  };

  if (!metadataOnly) {
    result.text = clipText_((message && message.text) || '');
    result.formattedText = clipText_((message && message.formattedText) || '');
    result.argumentText = clipText_((message && message.argumentText) || '');
    result.fallbackText = clipText_((message && message.fallbackText) || '');
  }

  return result;
}

function normalizeAttachments_(attachments) {
  return attachments.map(function (attachment) {
    return {
      name: attachment.name || null,
      contentName: attachment.contentName || null,
      contentType: attachment.contentType || null,
      attachmentDataRef: attachment.attachmentDataRef || null,
      source: attachment.source || null,
      downloadUri: attachment.downloadUri || null,
      thumbnailUri: attachment.thumbnailUri || null,
    };
  });
}

function publicConfig_() {
  return {
    version: CHAT_BRIDGE.version,
    spaceName: CHAT_BRIDGE.spaceName,
    spaceId: CHAT_BRIDGE.spaceId,
    expectedDisplayName: CHAT_BRIDGE.expectedDisplayName,
    sourceUrl: CHAT_BRIDGE.sourceUrl,
    startTimeInclusive: CHAT_BRIDGE.startTimeInclusive,
    filter: CHAT_BRIDGE.baseFilter,
  };
}

function requireBridgeToken_(providedToken) {
  const expectedHash = PropertiesService.getScriptProperties().getProperty(
    CHAT_BRIDGE.tokenHashProperty,
  );
  if (!expectedHash) {
    throw new BridgeError_(
      'bridge_not_configured',
      'Run setupBridge() from the Apps Script editor first.',
      503,
    );
  }

  const provided = String(providedToken || '');
  if (!provided || !constantTimeEqual_(sha256Hex_(provided), expectedHash)) {
    throw new BridgeError_('unauthorized', 'Invalid or missing bridge token.', 401);
  }
}

function enforceRateLimit_() {
  const minuteBucket = Utilities.formatDate(new Date(), 'UTC', 'yyyyMMddHHmm');
  const cacheKey = 'chat-bridge-rate-' + minuteBucket;
  const lock = LockService.getScriptLock();

  if (!lock.tryLock(5000)) {
    throw new BridgeError_('busy', 'The bridge is busy; retry shortly.', 503);
  }

  try {
    const cache = CacheService.getScriptCache();
    const count = Number(cache.get(cacheKey) || '0') + 1;
    if (count > CHAT_BRIDGE.maxRequestsPerMinute) {
      throw new BridgeError_('rate_limited', 'Too many requests; retry next minute.', 429);
    }
    cache.put(cacheKey, String(count), 120);
  } finally {
    lock.releaseLock();
  }
}

function storeBridgeToken_(token) {
  PropertiesService.getScriptProperties().setProperties({
    [CHAT_BRIDGE.tokenHashProperty]: sha256Hex_(token),
    [CHAT_BRIDGE.tokenCreatedAtProperty]: new Date().toISOString(),
  });
}

function generateBridgeToken_() {
  const entropy = [
    Utilities.getUuid(),
    Utilities.getUuid(),
    Utilities.getUuid(),
    String(Date.now()),
    Session.getTemporaryActiveUserKey(),
  ].join(':');
  return Utilities.base64EncodeWebSafe(
    Utilities.computeDigest(
      Utilities.DigestAlgorithm.SHA_256,
      entropy,
      Utilities.Charset.UTF_8,
    ),
  ).replace(/=+$/g, '');
}

function sha256Hex_(value) {
  const bytes = Utilities.computeDigest(
    Utilities.DigestAlgorithm.SHA_256,
    String(value),
    Utilities.Charset.UTF_8,
  );
  return bytes
    .map(function (byte) {
      const normalized = byte < 0 ? byte + 256 : byte;
      return ('0' + normalized.toString(16)).slice(-2);
    })
    .join('');
}

function constantTimeEqual_(a, b) {
  const left = String(a || '');
  const right = String(b || '');
  let mismatch = left.length ^ right.length;
  const length = Math.max(left.length, right.length);
  for (let i = 0; i < length; i += 1) {
    mismatch |= (left.charCodeAt(i % Math.max(left.length, 1)) || 0) ^
      (right.charCodeAt(i % Math.max(right.length, 1)) || 0);
  }
  return mismatch === 0;
}

function validateThreadName_(value) {
  const threadName = String(value || '').trim();
  if (!threadName) return '';

  const prefix = CHAT_BRIDGE.spaceName + '/threads/';
  if (
    threadName.indexOf(prefix) !== 0 ||
    !/^spaces\/[A-Za-z0-9_-]+\/threads\/[A-Za-z0-9._~-]+$/.test(threadName)
  ) {
    throw new BridgeError_(
      'invalid_thread',
      'threadName must belong to the configured Chat space.',
      400,
    );
  }
  return threadName;
}

function clipText_(value) {
  const text = String(value || '');
  if (text.length <= CHAT_BRIDGE.maxTextCharsPerField) return text;
  return text.slice(0, CHAT_BRIDGE.maxTextCharsPerField) + '\n[TRUNCATED_BY_BRIDGE]';
}

function parseInteger_(value, fallback) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.trunc(parsed) : fallback;
}

function parseBoolean_(value, fallback) {
  if (typeof value === 'boolean') return value;
  if (value === undefined || value === null || value === '') return fallback;
  const normalized = String(value).trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].indexOf(normalized) !== -1) return true;
  if (['0', 'false', 'no', 'off'].indexOf(normalized) !== -1) return false;
  return fallback;
}

function clamp_(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function bridgeIsConfigured_() {
  return Boolean(
    PropertiesService.getScriptProperties().getProperty(CHAT_BRIDGE.tokenHashProperty),
  );
}

function safeWebAppUrl_() {
  try {
    return ScriptApp.getService().getUrl() || null;
  } catch (error) {
    return null;
  }
}

function jsonOutput_(value) {
  return ContentService.createTextOutput(JSON.stringify(value, null, 2)).setMimeType(
    ContentService.MimeType.JSON,
  );
}

function normalizeError_(error) {
  if (error instanceof BridgeError_) {
    return { code: error.code, message: error.message, status: error.status };
  }

  const message = error && error.message ? String(error.message) : String(error);
  let code = 'internal_error';
  let status = 500;

  if (/permission|forbidden|not authorized|insufficient/i.test(message)) {
    code = 'google_permission_denied';
    status = 403;
  } else if (/not found/i.test(message)) {
    code = 'google_resource_not_found';
    status = 404;
  } else if (/invalid.*scope|scope/i.test(message)) {
    code = 'google_scope_error';
    status = 403;
  } else if (/quota|rate/i.test(message)) {
    code = 'google_quota_error';
    status = 429;
  }

  return { code, message, status };
}

function BridgeError_(code, message, status) {
  this.name = 'BridgeError';
  this.code = code;
  this.message = message;
  this.status = status;
  this.stack = new Error(message).stack;
}
BridgeError_.prototype = Object.create(Error.prototype);
BridgeError_.prototype.constructor = BridgeError_;
