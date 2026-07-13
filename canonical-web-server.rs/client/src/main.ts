import htmx from "htmx.org";
import "htmx-ext-ws";
import { CanonicalSyncClient, type CanonicalSyncClientOptions } from "./sync";

declare global {
  interface Window {
    htmx: typeof htmx;
    canonicalSync?: CanonicalSyncClient;
    bootstrapCanonicalSync: (options: CanonicalSyncClientOptions) => Promise<CanonicalSyncClient>;
  }
}

window.htmx = htmx;
htmx.config.allowEval = false;

interface HtmxWebSocketMessageDetail {
  message: string;
}

interface HtmxWebSocketCloseDetail {
  event: CloseEvent;
}

let handlingSocketAuthLoss = false;

function handleSocketAuthLoss(): void {
  if (handlingSocketAuthLoss) {
    return;
  }
  handlingSocketAuthLoss = true;
  const clear = window.canonicalSync?.clearLocalData() ?? Promise.resolve();
  void clear.finally(() => window.location.assign("/login"));
}

htmx.on("htmx:wsBeforeMessage", (event) => {
  const message = (event as CustomEvent<HtmxWebSocketMessageDetail>).detail.message;
  try {
    const payload: unknown = JSON.parse(message);
    if (typeof payload !== "object" || payload === null || !("type" in payload)) {
      return;
    }
    const type = (payload as { type?: unknown }).type;
    if (typeof type !== "string") {
      return;
    }
    if (type === "sync.invalidated" || type === "resync_required") {
      event.preventDefault();
      void window.canonicalSync?.syncNow();
    } else if (type === "access_revoked") {
      event.preventDefault();
      handleSocketAuthLoss();
    } else if (type === "hello" || type === "pong") {
      event.preventDefault();
    }
  } catch {
    // Non-protocol messages remain available for normal HTMX OOB swaps.
  }
});

htmx.on("htmx:wsClose", (event) => {
  const close = (event as CustomEvent<HtmxWebSocketCloseDetail>).detail.event;
  if (close.code === 1008) {
    handleSocketAuthLoss();
  }
});

const csrfToken = document.querySelector<HTMLMetaElement>('meta[name="csrf-token"]')?.content;
if (csrfToken !== undefined && csrfToken.length > 0) {
  htmx.on("htmx:configRequest", (event) => {
    const detail = (event as CustomEvent<{ headers: Record<string, string> }>).detail;
    detail.headers["x-csrf-token"] = csrfToken;
  });
}

export async function bootstrapCanonicalSync(
  options: CanonicalSyncClientOptions,
): Promise<CanonicalSyncClient> {
  await window.canonicalSync?.close();
  const client = await CanonicalSyncClient.bootstrap(options);
  window.canonicalSync = client;
  client.start();
  return client;
}

window.bootstrapCanonicalSync = bootstrapCanonicalSync;

const accountKey = document.querySelector<HTMLMetaElement>('meta[name="canonical-account-key"]')?.content;
if (accountKey !== undefined && accountKey.length > 0) {
  void bootstrapCanonicalSync({ accountKey }).then(wireDraftNoteUi);
}

const logoutForm = document.querySelector<HTMLFormElement>('form[action="/auth/logout"]');
logoutForm?.addEventListener("submit", (event) => {
  event.preventDefault();
  void (async () => {
    try {
      await window.canonicalSync?.clearLocalData();
    } finally {
      logoutForm.submit();
    }
  })();
});

function wireDraftNoteUi(client: CanonicalSyncClient): void {
  const form = document.querySelector<HTMLFormElement>('form[data-sync-form="draft_note"]');
  const list = document.querySelector<HTMLElement>('[data-sync-list="draft_note"]');
  const conflictList = document.querySelector<HTMLElement>('[data-sync-conflicts="draft_note"]');
  const status = document.querySelector<HTMLElement>("#sync-status");
  if (form === null || list === null) {
    return;
  }

  const render = async (): Promise<void> => {
    const notes = await client.store.listEffectiveDraftNotes();
    const fragment = document.createDocumentFragment();
    for (const note of notes) {
      const article = document.createElement("article");
      article.className = "card";
      const heading = document.createElement("h3");
      heading.textContent = note.value.title;
      const body = document.createElement("p");
      body.textContent = note.value.body;
      const metadata = document.createElement("small");
      metadata.className = "muted";
      metadata.textContent = note.failed
        ? "Sync failed; discard this local change or try again with a new edit"
        : note.conflicted
        ? "Conflict needs review"
        : note.pending
          ? "Saved locally; sync pending"
          : `Synced at version ${note.version ?? "new"}`;
      const action = document.createElement("button");
      action.type = "button";
      if (note.failed) {
        action.textContent = "Discard local change";
        action.addEventListener("click", () => {
          void client.discardLocalVersion(note.id).catch(showError);
        });
      } else if (note.conflicted) {
        action.textContent = "Use server version";
        action.addEventListener("click", () => {
          void client.acceptServerVersion(note.id).catch(showError);
        });
      } else {
        action.textContent = "Delete";
        action.addEventListener("click", () => {
          void client.deleteDraftNote(note.id).catch(showError);
        });
      }
      article.append(heading, body, metadata, document.createElement("br"), action);
      fragment.append(article);
    }
    list.replaceChildren(fragment);
    if (conflictList !== null) {
      const conflicts = await client.store.listConflicts();
      const conflictFragment = document.createDocumentFragment();
      for (const conflict of conflicts) {
        const article = document.createElement("article");
        article.className = "card";
        const heading = document.createElement("h3");
        heading.textContent =
          conflict.local.action === "delete"
            ? "Delete conflicted"
            : conflict.local.value?.title ?? "Edit conflicted";
        const explanation = document.createElement("p");
        explanation.className = "error";
        explanation.textContent =
          conflict.reason === "gone"
            ? "The server record was deleted."
            : "A newer server version exists.";
        const serverValue = document.createElement("p");
        serverValue.className = "muted";
        serverValue.textContent =
          conflict.server === null || conflict.server.deleted
            ? "Server version: deleted"
            : `Server version: ${conflict.server.value?.title ?? "untitled"}`;
        const resolution = document.createElement("button");
        resolution.type = "button";
        resolution.textContent = "Use server version";
        resolution.addEventListener("click", () => {
          void client.acceptServerVersion(conflict.id).catch(showError);
        });
        article.append(heading, explanation, serverValue, resolution);
        conflictFragment.append(article);
      }
      conflictList.replaceChildren(conflictFragment);
    }
    if (status !== null) {
      status.dataset.state = navigator.onLine ? "synced" : "offline";
      status.textContent = navigator.onLine ? "Local cache and server sync active" : "Offline; edits stay queued locally";
    }
  };

  const showError = (error: unknown): void => {
    if (status !== null) {
      status.dataset.state = "offline";
      status.textContent = error instanceof Error ? error.message : "The local change could not be saved";
    }
  };

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const data = new FormData(form);
    const existingId = String(data.get("id") ?? "").trim();
    const id = existingId.length > 0 ? existingId : crypto.randomUUID();
    void client
      .putDraftNote(id, {
        title: String(data.get("title") ?? ""),
        body: String(data.get("body") ?? ""),
      })
      .then(() => {
        form.reset();
        return render();
      })
      .catch(showError);
  });
  client.addEventListener("change", () => void render());
  void render();
}

export { htmx };
export * from "./sync";
