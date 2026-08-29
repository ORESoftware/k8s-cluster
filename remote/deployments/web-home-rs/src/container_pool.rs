pub(crate) const CONTAINER_POOL_CONFIG_CSS: &str = r###":root {
  color-scheme: dark;
  --bg: #0c1116;
  --panel: #141a21;
  --panel-2: #1a222b;
  --line: #1f2a36;
  --line-2: #2a3848;
  --text: #e6edf3;
  --muted: #8a9aae;
  --accent: #4ea1ff;
  --accent-2: #67d391;
  --warn: #ffb454;
  --bad: #ff7a7a;
  --good: #67d391;
  --code: #e6edf3;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: 'Inter', system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
  background: var(--bg);
  color: var(--text);
}
.cpool-shell {
  display: grid;
  grid-template-columns: minmax(280px, 320px) 1fr;
  gap: 0;
  min-height: calc(100vh - 60px);
}
@media (max-width: 900px) {
  .cpool-shell { grid-template-columns: 1fr; }
}
.cpool-sidebar {
  background: var(--panel);
  border-right: 1px solid var(--line);
  padding: 18px 14px;
  overflow-y: auto;
  max-height: calc(100vh - 60px);
}
.cpool-sidebar h2 {
  font-size: 14px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--muted);
  margin: 0 0 12px;
}
.cpool-img-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 6px; }
.cpool-img {
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 10px 12px;
  cursor: pointer;
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 6px 12px;
  align-items: center;
  transition: border-color 0.12s, background 0.12s;
}
.cpool-img:hover { border-color: var(--line-2); }
.cpool-img.active { border-color: var(--accent); background: #16202c; }
.cpool-img .title { font-weight: 600; color: var(--text); font-size: 13px; }
.cpool-img .slug { color: var(--muted); font-size: 12px; font-family: 'JetBrains Mono', ui-monospace, SFMono-Regular, monospace; }
.cpool-img .badge {
  font-size: 11px;
  border-radius: 999px;
  padding: 2px 8px;
  border: 1px solid var(--line-2);
  color: var(--muted);
  white-space: nowrap;
}
.cpool-img .badge.ok { color: var(--good); border-color: rgba(103,211,145,0.4); background: rgba(103,211,145,0.07); }
.cpool-img .badge.fail { color: var(--bad); border-color: rgba(255,122,122,0.4); background: rgba(255,122,122,0.07); }
.cpool-img .badge.run { color: var(--accent); border-color: rgba(78,161,255,0.4); background: rgba(78,161,255,0.07); }
.cpool-img .meta { grid-column: 1 / -1; color: var(--muted); font-size: 11px; font-family: ui-monospace, SFMono-Regular, monospace; }

.cpool-main { padding: 22px 28px; max-width: 1100px; }
.cpool-empty { color: var(--muted); padding: 60px 0; text-align: center; }
.cpool-head { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin-bottom: 6px; }
.cpool-head h1 { font-size: 22px; margin: 0; }
.cpool-head .image-ref { font-family: ui-monospace, monospace; color: var(--accent); font-size: 13px; }
.cpool-sub { color: var(--muted); font-size: 13px; margin-bottom: 18px; }
.cpool-sub code { background: var(--panel-2); padding: 1px 6px; border-radius: 4px; color: var(--code); }

.cpool-actions { display: flex; gap: 10px; flex-wrap: wrap; margin: 16px 0 12px; }
.cpool-actions button, .cpool-actions select, .cpool-actions input {
  background: var(--panel-2);
  border: 1px solid var(--line);
  color: var(--text);
  padding: 8px 14px;
  border-radius: 8px;
  font-size: 13px;
  cursor: pointer;
  font-family: inherit;
}
.cpool-actions button:hover { border-color: var(--line-2); }
.cpool-actions button.primary { background: var(--accent); color: #0c1116; border-color: var(--accent); font-weight: 600; }
.cpool-actions button.primary:hover { background: #6ab1ff; border-color: #6ab1ff; }
.cpool-actions button.warn { color: var(--warn); border-color: rgba(255,180,84,0.4); }
.cpool-actions button:disabled { opacity: 0.5; cursor: not-allowed; }
.cpool-status { font-size: 12px; color: var(--muted); margin-left: auto; }

.cpool-editor textarea {
  width: 100%;
  min-height: 320px;
  background: #0a0f15;
  color: var(--code);
  font-family: 'JetBrains Mono', ui-monospace, SFMono-Regular, monospace;
  font-size: 13px;
  line-height: 1.45;
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 12px 14px;
  resize: vertical;
}
.cpool-editor .editor-meta { display: flex; gap: 14px; font-size: 12px; color: var(--muted); margin: 4px 0 10px; flex-wrap: wrap; }
.cpool-editor .editor-meta code { font-family: ui-monospace, monospace; color: var(--code); }

.cpool-section { margin-top: 26px; }
.cpool-section h3 { font-size: 14px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); margin: 0 0 10px; }
.cpool-table { width: 100%; border-collapse: collapse; font-size: 13px; }
.cpool-table th, .cpool-table td { padding: 8px 10px; text-align: left; border-bottom: 1px solid var(--line); }
.cpool-table th { color: var(--muted); font-weight: 500; font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; }
.cpool-table td.mono { font-family: ui-monospace, monospace; color: var(--muted); font-size: 12px; }
.cpool-table tr:hover { background: var(--panel-2); }
.cpool-table .status-pill { display: inline-block; padding: 2px 8px; border-radius: 999px; border: 1px solid var(--line-2); font-size: 11px; color: var(--muted); }
.cpool-table .status-pill.passed, .cpool-table .status-pill.built { color: var(--good); border-color: rgba(103,211,145,0.4); }
.cpool-table .status-pill.failed, .cpool-table .status-pill.errored { color: var(--bad); border-color: rgba(255,122,122,0.4); }
.cpool-table .status-pill.running, .cpool-table .status-pill.building, .cpool-table .status-pill.testing { color: var(--accent); border-color: rgba(78,161,255,0.4); }
.cpool-row-action { background: none; border: none; color: var(--accent); cursor: pointer; font-size: 12px; padding: 0; }

.cpool-modal { position: fixed; inset: 0; background: rgba(6, 10, 14, 0.7); z-index: 90; display: none; align-items: center; justify-content: center; padding: 24px; }
.cpool-modal.open { display: flex; }
.cpool-modal-card { background: var(--panel); border: 1px solid var(--line); border-radius: 10px; max-width: 900px; width: 100%; max-height: 80vh; display: flex; flex-direction: column; }
.cpool-modal-head { padding: 14px 18px; border-bottom: 1px solid var(--line); display: flex; align-items: center; gap: 10px; }
.cpool-modal-head h3 { margin: 0; font-size: 15px; }
.cpool-modal-body { padding: 14px 18px; overflow-y: auto; font-family: ui-monospace, monospace; font-size: 12px; white-space: pre-wrap; color: var(--code); }
.cpool-modal-close { margin-left: auto; background: transparent; border: 1px solid var(--line-2); color: var(--text); padding: 4px 10px; border-radius: 6px; cursor: pointer; }
.cpool-toast { position: fixed; bottom: 24px; right: 24px; background: var(--panel); border: 1px solid var(--line-2); padding: 10px 14px; border-radius: 8px; font-size: 13px; color: var(--text); box-shadow: 0 12px 32px rgba(0,0,0,0.4); display: none; z-index: 100; }
.cpool-toast.show { display: block; }
.cpool-toast.bad { border-color: rgba(255,122,122,0.6); color: var(--bad); }
.cpool-toast.good { border-color: rgba(103,211,145,0.6); color: var(--good); }
"###;

pub(crate) const CONTAINER_POOL_CONFIG_BODY: &str = r###"<div class="cpool-shell">
  <aside class="cpool-sidebar">
    <h2>Pool images</h2>
    <ul id="cpool-image-list" class="cpool-img-list" aria-label="Container pool images"></ul>
  </aside>
  <main class="cpool-main">
    <div id="cpool-empty" class="cpool-empty">Select a pool image on the left to view and edit its Dockerfile.</div>
    <div id="cpool-detail" hidden>
      <div class="cpool-head">
        <h1 id="cpool-title">…</h1>
        <span id="cpool-image-ref" class="image-ref"></span>
      </div>
      <div class="cpool-sub">
        Dockerfile <code id="cpool-dockerfile-path">…</code> &middot; build context <code id="cpool-build-context">…</code> &middot; namespace <code id="cpool-namespace">dd-pool</code>
      </div>
      <p id="cpool-notes" class="cpool-sub"></p>
      <div class="cpool-actions">
        <button id="cpool-load-disk" type="button" title="Replace the editor contents with the on-disk Dockerfile from git">Load disk default</button>
        <button id="cpool-save" class="primary" type="button">Save as new revision</button>
        <button id="cpool-build-test" class="primary" type="button">Build &amp; test</button>
        <span id="cpool-status" class="cpool-status">idle</span>
      </div>
      <div class="cpool-editor">
        <div class="editor-meta">
          <span>SHA-256 <code id="cpool-sha">—</code></span>
          <span>Source <code id="cpool-source">—</code></span>
          <span>Bytes <code id="cpool-bytes">0</code></span>
        </div>
        <textarea id="cpool-textarea" spellcheck="false" placeholder="# Dockerfile contents will appear here"></textarea>
      </div>
      <section class="cpool-section">
        <h3>Build &amp; test history</h3>
        <table class="cpool-table" id="cpool-builds-table">
          <thead>
            <tr><th>When</th><th>Overall</th><th>Build</th><th>Test</th><th>Revision</th><th>Tag</th><th></th></tr>
          </thead>
          <tbody><tr><td colspan="7" style="color:var(--muted)">No build runs yet.</td></tr></tbody>
        </table>
      </section>
      <section class="cpool-section">
        <h3>Dockerfile revisions</h3>
        <table class="cpool-table" id="cpool-revisions-table">
          <thead>
            <tr><th>When</th><th>SHA-256</th><th>Source</th><th>Notes</th><th></th></tr>
          </thead>
          <tbody><tr><td colspan="5" style="color:var(--muted)">No saved revisions yet.</td></tr></tbody>
        </table>
      </section>
    </div>
  </main>
</div>

<div id="cpool-modal" class="cpool-modal" role="dialog" aria-modal="true">
  <div class="cpool-modal-card">
    <div class="cpool-modal-head">
      <h3 id="cpool-modal-title">Build logs</h3>
      <button id="cpool-modal-close" class="cpool-modal-close" type="button">Close</button>
    </div>
    <div id="cpool-modal-body" class="cpool-modal-body"></div>
  </div>
</div>

<div id="cpool-toast" class="cpool-toast" role="status" aria-live="polite"></div>
"###;

pub(crate) const CONTAINER_POOL_CONFIG_JS: &str = r###"const $ = (id) => document.getElementById(id);
const state = {
  images: [],
  currentSlug: null,
  current: null,
  pollHandle: null,
  buildsEnabled: false,
};

function showToast(message, level = 'info') {
  const el = $('cpool-toast');
  el.textContent = message;
  el.classList.remove('good', 'bad');
  if (level === 'good') el.classList.add('good');
  if (level === 'bad') el.classList.add('bad');
  el.classList.add('show');
  clearTimeout(showToast._t);
  showToast._t = setTimeout(() => el.classList.remove('show'), 4500);
}

function statusBadge(overall) {
  if (!overall) return '';
  const c = String(overall).toLowerCase();
  let cls = '';
  if (c === 'passed') cls = 'ok';
  else if (c === 'failed' || c === 'errored') cls = 'fail';
  else if (c === 'running' || c === 'building' || c === 'testing' || c === 'queued') cls = 'run';
  return `<span class="badge ${cls}">${c}</span>`;
}

function fmtDate(iso) {
  if (!iso) return '—';
  try {
    const d = new Date(iso);
    return d.toLocaleString();
  } catch (_) { return iso; }
}

async function loadImages() {
  const r = await fetch('/api/container-pool/images', { cache: 'no-store' });
  if (!r.ok) { showToast('Failed to list images', 'bad'); return; }
  const body = await r.json();
  state.images = body.images || [];
  state.buildsEnabled = !!body.buildsEnabled;
  renderImageList();
}

function renderImageList() {
  const list = $('cpool-image-list');
  list.innerHTML = '';
  for (const img of state.images) {
    const li = document.createElement('li');
    li.className = 'cpool-img' + (img.slug === state.currentSlug ? ' active' : '');
    li.dataset.slug = img.slug;
    const lastOverall = img.latest_build && img.latest_build.overall_status;
    const badge = lastOverall ? statusBadge(lastOverall) : '';
    li.innerHTML = `
      <div>
        <div class="title">${img.display_name}</div>
        <div class="slug">${img.slug}</div>
      </div>
      <div>${badge}</div>
      <div class="meta">${img.image_ref}</div>
    `;
    li.addEventListener('click', () => selectImage(img.slug));
    list.appendChild(li);
  }
}

async function selectImage(slug) {
  state.currentSlug = slug;
  renderImageList();
  $('cpool-empty').hidden = true;
  $('cpool-detail').hidden = false;
  $('cpool-status').textContent = 'loading';
  await Promise.all([loadDetail(slug), loadRevisions(slug), loadBuilds(slug)]);
  $('cpool-status').textContent = 'idle';
}

async function loadDetail(slug) {
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(slug)}`);
  if (!r.ok) { showToast('Failed to load image detail', 'bad'); return; }
  const body = await r.json();
  state.current = body;
  $('cpool-title').textContent = body.image.displayName;
  $('cpool-image-ref').textContent = body.image.imageRef;
  $('cpool-dockerfile-path').textContent = body.image.dockerfilePath;
  $('cpool-build-context').textContent = body.image.buildContext;
  $('cpool-namespace').textContent = body.namespace || 'dd-pool';
  $('cpool-notes').textContent = body.image.notes || '';
  const rev = body.currentRevision || {};
  const text = rev.dockerfile_text || '';
  $('cpool-textarea').value = text;
  $('cpool-sha').textContent = rev.dockerfile_sha256 ? rev.dockerfile_sha256.slice(0, 12) : '—';
  $('cpool-source').textContent = rev.source || '—';
  $('cpool-bytes').textContent = text.length;
  $('cpool-build-test').disabled = !state.buildsEnabled;
  if (!state.buildsEnabled) {
    $('cpool-build-test').title = 'Builds disabled — set CONTAINER_POOL_IMAGE_BUILDS_ENABLED=true on dd-remote-rest-api';
  } else {
    $('cpool-build-test').title = 'Build the candidate image and run a smoke test';
  }
}

async function loadRevisions(slug) {
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(slug)}/revisions`);
  if (!r.ok) return;
  const body = await r.json();
  const rows = body.revisions || [];
  const tbody = $('cpool-revisions-table').querySelector('tbody');
  tbody.innerHTML = '';
  if (!rows.length) {
    tbody.innerHTML = '<tr><td colspan="5" style="color:var(--muted)">No saved revisions yet.</td></tr>';
    return;
  }
  for (const rev of rows) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td class="mono">${fmtDate(rev.created_at)}</td>
      <td class="mono">${(rev.dockerfile_sha256 || '').slice(0, 12)}</td>
      <td>${rev.source}</td>
      <td>${escapeHtml(rev.notes || '')}</td>
      <td><button class="cpool-row-action" data-rev="${rev.id}">Load</button></td>
    `;
    tbody.appendChild(tr);
  }
  tbody.querySelectorAll('.cpool-row-action').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const id = btn.dataset.rev;
      const r2 = await fetch(`/api/container-pool/images/${encodeURIComponent(slug)}/dockerfile?revisionId=${encodeURIComponent(id)}`);
      if (!r2.ok) { showToast('Failed to load revision', 'bad'); return; }
      const body2 = await r2.json();
      const rev2 = body2.revision || {};
      $('cpool-textarea').value = rev2.dockerfile_text || '';
      $('cpool-sha').textContent = (rev2.dockerfile_sha256 || '').slice(0, 12);
      $('cpool-source').textContent = rev2.source || '—';
      $('cpool-bytes').textContent = ($('cpool-textarea').value || '').length;
      showToast('Loaded revision into editor', 'good');
    });
  });
}

async function loadBuilds(slug) {
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(slug)}/builds`);
  if (!r.ok) return;
  const body = await r.json();
  const rows = body.builds || [];
  const tbody = $('cpool-builds-table').querySelector('tbody');
  tbody.innerHTML = '';
  if (!rows.length) {
    tbody.innerHTML = '<tr><td colspan="7" style="color:var(--muted)">No build runs yet.</td></tr>';
    return;
  }
  for (const b of rows) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td class="mono">${fmtDate(b.created_at)}</td>
      <td><span class="status-pill ${b.overall_status}">${b.overall_status}</span></td>
      <td><span class="status-pill ${b.build_status}">${b.build_status}</span></td>
      <td><span class="status-pill ${b.test_status}">${b.test_status}</span></td>
      <td class="mono">${(b.revision_id || '').slice(0, 8)}</td>
      <td class="mono">${b.candidate_tag}</td>
      <td><button class="cpool-row-action" data-build="${b.id}">Logs</button></td>
    `;
    tbody.appendChild(tr);
  }
  tbody.querySelectorAll('.cpool-row-action').forEach((btn) => {
    btn.addEventListener('click', () => openBuildLogs(btn.dataset.build));
  });
}

async function openBuildLogs(buildId) {
  const r = await fetch(`/api/container-pool/builds/${encodeURIComponent(buildId)}`);
  if (!r.ok) { showToast('Failed to load build', 'bad'); return; }
  const body = await r.json();
  const b = body.build || {};
  $('cpool-modal-title').textContent = `${b.image_slug} → ${b.candidate_tag}`;
  const parts = [];
  parts.push(`overall:  ${b.overall_status}`);
  parts.push(`build:    ${b.build_status}    started ${fmtDate(b.build_started_at)} → ${fmtDate(b.build_finished_at)}`);
  parts.push(`test:     ${b.test_status}    started ${fmtDate(b.test_started_at)} → ${fmtDate(b.test_finished_at)}`);
  if (b.error_message) parts.push(`error:    ${b.error_message}`);
  parts.push('');
  parts.push('========= BUILD LOG =========');
  parts.push(b.build_log_excerpt || '(no build log)');
  parts.push('');
  parts.push('========= TEST LOG ==========');
  parts.push(b.test_log_excerpt || '(no test log)');
  $('cpool-modal-body').textContent = parts.join('\n');
  $('cpool-modal').classList.add('open');
}

function escapeHtml(value) {
  return String(value || '').replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  }[c]));
}

async function saveRevision() {
  if (!state.currentSlug) return;
  const text = $('cpool-textarea').value;
  const notes = window.prompt('Optional notes for this revision:', '') || '';
  $('cpool-status').textContent = 'saving';
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(state.currentSlug)}/dockerfile`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ dockerfileText: text, notes }),
  });
  if (!r.ok) {
    const body = await r.json().catch(() => ({}));
    showToast(`Save failed: ${body.error || r.status}`, 'bad');
    $('cpool-status').textContent = 'idle';
    return;
  }
  showToast('Saved revision', 'good');
  await loadRevisions(state.currentSlug);
  await loadDetail(state.currentSlug);
  $('cpool-status').textContent = 'idle';
}

async function loadDiskDefault() {
  if (!state.currentSlug) return;
  $('cpool-status').textContent = 'loading disk default';
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(state.currentSlug)}/dockerfile?source=disk-default`, { cache: 'no-store' });
  if (!r.ok) { showToast('Failed to load disk default', 'bad'); $('cpool-status').textContent = 'idle'; return; }
  const body = await r.json();
  $('cpool-textarea').value = body.dockerfileText || '';
  $('cpool-sha').textContent = (body.dockerfileSha256 || '').slice(0, 12);
  $('cpool-source').textContent = 'disk-default';
  $('cpool-bytes').textContent = ($('cpool-textarea').value || '').length;
  showToast('Loaded on-disk Dockerfile', 'good');
  $('cpool-status').textContent = 'idle';
}

async function triggerBuildTest() {
  if (!state.currentSlug) return;
  if (!state.buildsEnabled) { showToast('Builds disabled', 'bad'); return; }
  const useCurrent = window.confirm('Save the current editor contents as a new revision and build+test it?\n\nCancel to use the latest saved revision instead.');
  $('cpool-status').textContent = 'kicking off build';
  $('cpool-build-test').disabled = true;
  let bodyJson = {};
  if (useCurrent) {
    bodyJson = { dockerfileText: $('cpool-textarea').value, notes: 'Submitted from /container-pool/config build+test action' };
  }
  const r = await fetch(`/api/container-pool/images/${encodeURIComponent(state.currentSlug)}/build-test`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(bodyJson),
  });
  $('cpool-build-test').disabled = false;
  if (!r.ok) {
    const body = await r.json().catch(() => ({}));
    showToast(`Build-test failed: ${body.error || r.status}`, 'bad');
    $('cpool-status').textContent = 'idle';
    return;
  }
  const body = await r.json();
  showToast(`Build queued: ${body.build.id.slice(0, 8)}`, 'good');
  $('cpool-status').textContent = 'building (polling…)';
  startPolling(body.build.id);
  await loadBuilds(state.currentSlug);
}

function startPolling(buildId) {
  if (state.pollHandle) clearInterval(state.pollHandle);
  state.pollHandle = setInterval(async () => {
    try {
      const r = await fetch(`/api/container-pool/builds/${encodeURIComponent(buildId)}`);
      if (!r.ok) return;
      const body = await r.json();
      const overall = body.build && body.build.overall_status;
      $('cpool-status').textContent = `build ${buildId.slice(0,8)}: ${overall}`;
      await loadBuilds(state.currentSlug);
      if (overall === 'passed' || overall === 'failed' || overall === 'errored' || overall === 'cancelled') {
        clearInterval(state.pollHandle); state.pollHandle = null;
        const lvl = overall === 'passed' ? 'good' : 'bad';
        showToast(`Build ${buildId.slice(0,8)}: ${overall}`, lvl);
        $('cpool-status').textContent = 'idle';
        await loadImages();
      }
    } catch (_) { /* ignore transient errors */ }
  }, 4000);
}

document.addEventListener('DOMContentLoaded', () => {
  $('cpool-save').addEventListener('click', saveRevision);
  $('cpool-load-disk').addEventListener('click', loadDiskDefault);
  $('cpool-build-test').addEventListener('click', triggerBuildTest);
  $('cpool-modal-close').addEventListener('click', () => $('cpool-modal').classList.remove('open'));
  $('cpool-modal').addEventListener('click', (e) => { if (e.target === $('cpool-modal')) $('cpool-modal').classList.remove('open'); });
  loadImages();
});
"###;

