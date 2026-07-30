import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const APP_SOURCE_URL = new URL('../App.gs', import.meta.url);
const EXPECTED_SPACE = 'spaces/AAQAoHKdzvI';
const EXPECTED_FILTER = 'createTime > "2026-05-10T03:59:59.999Z"';

function signedBytes(buffer) {
  return [...buffer].map((byte) => (byte > 127 ? byte - 256 : byte));
}

function unsignedBytes(bytes) {
  return Buffer.from(bytes.map((byte) => (byte < 0 ? byte + 256 : byte)));
}

function createHarness({ messages = [], nextPageToken = null } = {}) {
  const properties = new Map();
  const cache = new Map();
  const chatCalls = [];
  let uuidCounter = 0;

  const scriptProperties = {
    getProperty(key) {
      return properties.has(key) ? properties.get(key) : null;
    },
    setProperty(key, value) {
      properties.set(key, String(value));
      return this;
    },
    setProperties(values) {
      for (const [key, value] of Object.entries(values)) properties.set(key, String(value));
      return this;
    },
    deleteProperty(key) {
      properties.delete(key);
      return this;
    },
  };

  const context = vm.createContext({
    console: {
      log() {},
      error() {},
      warn() {},
    },
    Utilities: {
      DigestAlgorithm: { SHA_256: 'SHA_256' },
      Charset: { UTF_8: 'UTF_8' },
      getUuid() {
        uuidCounter += 1;
        return `uuid-${uuidCounter}`;
      },
      computeDigest(algorithm, value) {
        assert.equal(algorithm, 'SHA_256');
        const digest = createHash('sha256').update(String(value), 'utf8').digest();
        return signedBytes(digest);
      },
      base64EncodeWebSafe(bytes) {
        return unsignedBytes(bytes).toString('base64url');
      },
      formatDate() {
        return '20260728-120000';
      },
    },
    PropertiesService: {
      getScriptProperties() {
        return scriptProperties;
      },
    },
    CacheService: {
      getScriptCache() {
        return {
          get(key) {
            return cache.get(key) ?? null;
          },
          put(key, value) {
            cache.set(key, String(value));
          },
        };
      },
    },
    LockService: {
      getScriptLock() {
        return {
          tryLock() {
            return true;
          },
          waitLock() {},
          releaseLock() {},
        };
      },
    },
    ScriptApp: {
      getService() {
        return {
          getUrl() {
            return 'https://script.google.com/macros/s/test/exec';
          },
        };
      },
    },
    Session: {
      getTemporaryActiveUserKey() {
        return 'temporary-user-key';
      },
    },
    ContentService: {
      MimeType: { JSON: 'application/json' },
      createTextOutput(text) {
        return {
          text,
          mimeType: null,
          setMimeType(mimeType) {
            this.mimeType = mimeType;
            return this;
          },
        };
      },
    },
    Chat: {
      Spaces: {
        get(name) {
          assert.equal(name, EXPECTED_SPACE);
          return {
            name,
            displayName: 'alex-alex-me',
            spaceType: 'SPACE',
            spaceHistoryState: 'HISTORY_ON',
            threaded: true,
          };
        },
        Messages: {
          list(parent, options) {
            chatCalls.push({ parent, options: structuredClone(options) });
            return {
              messages: structuredClone(messages),
              nextPageToken,
            };
          },
        },
      },
    },
    structuredClone,
  });

  return {
    context,
    properties,
    cache,
    chatCalls,
    async load() {
      const source = await readFile(APP_SOURCE_URL, 'utf8');
      vm.runInContext(source, context, { filename: 'App.gs' });
      return this;
    },
    json(output) {
      assert.equal(output.mimeType, 'application/json');
      return JSON.parse(output.text);
    },
  };
}

function fixtureMessage(overrides = {}) {
  return {
    name: 'spaces/AAQAoHKdzvI/messages/m1',
    space: { name: EXPECTED_SPACE },
    thread: { name: 'spaces/AAQAoHKdzvI/threads/t1' },
    sender: { name: 'users/1', displayName: 'Alex', type: 'HUMAN' },
    createTime: '2026-05-11T12:00:00.000Z',
    text: 'Create a tested Google Chat import.',
    formattedText: 'Create a tested Google Chat import.',
    ...overrides,
  };
}

test('health is public, versioned, and reports configuration state', async () => {
  const harness = await createHarness().load();
  const response = harness.json(
    harness.context.doGet({ parameter: { action: 'health' } }),
  );

  assert.equal(response.ok, true);
  assert.equal(response.service, 'google-chat-linear-bridge');
  assert.equal(response.version, '1.0.1');
  assert.equal(response.configured, false);
});

test('authenticated routes reject missing and invalid bridge tokens', async () => {
  const harness = await createHarness().load();
  const setup = harness.context.setupBridge();
  harness.chatCalls.length = 0;

  const missing = harness.json(
    harness.context.doGet({ parameter: { action: 'status' } }),
  );
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'unauthorized');

  const invalid = harness.json(
    harness.context.doGet({ parameter: { action: 'status', token: 'wrong-token' } }),
  );
  assert.equal(invalid.ok, false);
  assert.equal(invalid.error.code, 'unauthorized');

  const valid = harness.json(
    harness.context.doGet({ parameter: { action: 'status', token: setup.bridgeToken } }),
  );
  assert.equal(valid.ok, true);
  assert.equal(valid.data.configured, true);
  assert.equal(valid.data.target.spaceName, EXPECTED_SPACE);
  assert.equal(valid.data.target.startTimeInclusive, '2026-05-10T04:00:00.000Z');
});

test('message listing uses the fixed filter, pagination, and default API ordering', async () => {
  const harness = await createHarness({
    messages: [fixtureMessage()],
    nextPageToken: 'next-token',
  }).load();
  const setup = harness.context.setupBridge();
  harness.chatCalls.length = 0;

  const response = harness.json(
    harness.context.doGet({
      parameter: {
        action: 'messages',
        token: setup.bridgeToken,
        pageSize: '250',
        pageToken: 'incoming-token',
        showDeleted: 'true',
      },
    }),
  );

  assert.equal(response.ok, true);
  assert.equal(harness.chatCalls.length, 1);
  const call = harness.chatCalls[0];
  assert.equal(call.parent, EXPECTED_SPACE);
  assert.deepEqual(call.options, {
    pageSize: 250,
    filter: EXPECTED_FILTER,
    showDeleted: true,
    pageToken: 'incoming-token',
  });
  assert.equal(Object.hasOwn(call.options, 'orderBy'), false);
  assert.equal(response.data.nextPageToken, 'next-token');
  assert.equal(
    response.data.messages[0].sourceKey,
    'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/m1',
  );
  assert.equal(
    response.data.messages[0].thread.sourceKey,
    'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/threads/t1',
  );
});

test('probe also relies on the documented default createTime ASC ordering', async () => {
  const harness = await createHarness({ messages: [fixtureMessage()] }).load();
  const probe = harness.context.probeChatAccess_();

  assert.equal(probe.space.name, EXPECTED_SPACE);
  assert.equal(harness.chatCalls.length, 1);
  assert.deepEqual(harness.chatCalls[0].options, {
    pageSize: 1,
    filter: EXPECTED_FILTER,
    showDeleted: true,
  });
  assert.equal(Object.hasOwn(harness.chatCalls[0].options, 'orderBy'), false);
});

test('thread filtering cannot escape the configured Chat space', async () => {
  const harness = await createHarness().load();
  const setup = harness.context.setupBridge();
  harness.chatCalls.length = 0;

  const rejected = harness.json(
    harness.context.doGet({
      parameter: {
        action: 'messages',
        token: setup.bridgeToken,
        threadName: 'spaces/OTHER/threads/t1',
      },
    }),
  );
  assert.equal(rejected.ok, false);
  assert.equal(rejected.error.code, 'invalid_thread');
  assert.equal(harness.chatCalls.length, 0);

  const accepted = harness.json(
    harness.context.doGet({
      parameter: {
        action: 'messages',
        token: setup.bridgeToken,
        threadName: 'spaces/AAQAoHKdzvI/threads/t1',
      },
    }),
  );
  assert.equal(accepted.ok, true);
  assert.equal(
    harness.chatCalls[0].options.filter,
    `${EXPECTED_FILTER} AND thread.name = spaces/AAQAoHKdzvI/threads/t1`,
  );
});

test('metadata-only responses omit message bodies but preserve provenance', async () => {
  const harness = await createHarness({ messages: [fixtureMessage()] }).load();
  const setup = harness.context.setupBridge();
  harness.chatCalls.length = 0;

  const response = harness.json(
    harness.context.doPost({
      parameter: {},
      postData: {
        type: 'application/json',
        contents: JSON.stringify({
          action: 'messages',
          token: setup.bridgeToken,
          metadataOnly: true,
        }),
      },
    }),
  );

  assert.equal(response.ok, true);
  assert.equal(response.data.metadataOnly, true);
  assert.equal(Object.hasOwn(response.data.messages[0], 'text'), false);
  assert.equal(response.data.messages[0].name, 'spaces/AAQAoHKdzvI/messages/m1');
  assert.equal(response.data.messages[0].createTime, '2026-05-11T12:00:00.000Z');
});
