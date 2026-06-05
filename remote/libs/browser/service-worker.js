const CACHE_NAME = "dd-browser-drafts-v1";
const DRAFT_PREFIX = "/__dd_browser_drafts__/";

self.addEventListener("install", (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

function draftRequest(key) {
  return new Request(`${DRAFT_PREFIX}${encodeURIComponent(String(key || ""))}.json`, {
    method: "GET",
    credentials: "same-origin",
  });
}

async function saveDraft(key, record) {
  if (!key) return { ok: false, error: "draft key is required" };
  const cache = await caches.open(CACHE_NAME);
  await cache.put(
    draftRequest(key),
    new Response(JSON.stringify(record || null), {
      headers: {
        "content-type": "application/json; charset=utf-8",
        "cache-control": "no-store",
      },
    }),
  );
  return { ok: true };
}

async function loadDraft(key) {
  if (!key) return { ok: false, error: "draft key is required" };
  const cache = await caches.open(CACHE_NAME);
  const response = await cache.match(draftRequest(key));
  if (!response) return { ok: true, record: null };
  return { ok: true, record: await response.json() };
}

async function deleteDraft(key) {
  if (!key) return { ok: false, error: "draft key is required" };
  const cache = await caches.open(CACHE_NAME);
  await cache.delete(draftRequest(key));
  return { ok: true };
}

self.addEventListener("message", (event) => {
  const port = event.ports && event.ports[0];
  if (!port) return;
  const message = event.data || {};
  const reply = (payload) => port.postMessage(payload);
  const fail = (error) => reply({ ok: false, error: error instanceof Error ? error.message : String(error) });

  if (message.type === "dd-lambda-draft-save") {
    saveDraft(message.key, message.record).then(reply, fail);
    return;
  }
  if (message.type === "dd-lambda-draft-load") {
    loadDraft(message.key).then(reply, fail);
    return;
  }
  if (message.type === "dd-lambda-draft-delete") {
    deleteDraft(message.key).then(reply, fail);
    return;
  }
  reply({ ok: false, error: "unsupported service worker message type" });
});
