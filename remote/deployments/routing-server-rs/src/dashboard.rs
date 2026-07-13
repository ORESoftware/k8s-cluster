//! The live canvas dashboard. Self-contained HTML/JS (no build step, no CDN): it
//! POSTs a generate+solve request, then polls `/api/solve/{id}` and redraws the
//! stops and current best routes on a <canvas> as the master's incumbent improves.

pub const DASHBOARD_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>dd-routing-server — live VRP/TSP</title>
<style>
  :root { color-scheme: light dark; }
  body { font-family: system-ui, sans-serif; margin: 1.25rem; max-width: 1100px; }
  h1 { font-size: 1.25rem; margin: 0 0 0.25rem; }
  p.sub { margin: 0 0 1rem; color: #666; }
  form { display: flex; flex-wrap: wrap; gap: 0.75rem; align-items: end; margin-bottom: 1rem; }
  label { display: flex; flex-direction: column; font-size: 0.75rem; color: #555; gap: 0.2rem; }
  input { width: 6rem; padding: 0.35rem; font-size: 0.9rem; }
  button { padding: 0.5rem 1rem; font-size: 0.9rem; cursor: pointer; }
  #stats { display: flex; gap: 1.5rem; margin: 0.5rem 0; font-variant-numeric: tabular-nums; }
  #stats b { font-size: 1.1rem; }
  canvas { border: 1px solid #8884; border-radius: 8px; width: 100%; height: auto; background: #fafafa; }
  @media (prefers-color-scheme: dark) { canvas { background: #1a1a1a; } }
</style>
</head>
<body>
  <h1>dd-routing-server</h1>
  <p class="sub">Distributed multi-start VRP/TSP. Workers race construction + 2-opt restarts over NATS JetStream; the incumbent tour streams here live.</p>
  <form id="form">
    <label>stops<input id="count" type="number" value="120" min="3" max="1000"></label>
    <label>vehicles<input id="vehicles" type="number" value="4" min="1" max="64"></label>
    <label>restarts<input id="restarts" type="number" value="24" min="1" max="512"></label>
    <label>seed<input id="seed" type="number" value="42" min="0"></label>
    <button type="submit">Generate &amp; solve</button>
  </form>
  <div id="stats">
    <span>best distance: <b id="distance">—</b></span>
    <span>restarts: <b id="progress">0/0</b></span>
    <span>improvements: <b id="improvements">0</b></span>
    <span>status: <b id="status">idle</b></span>
  </div>
  <canvas id="canvas" width="1000" height="600"></canvas>

<script>
const palette = ['#e6194b','#3cb44b','#4363d8','#f58231','#911eb4','#42d4f4','#f032e6','#bfef45','#fabed4','#469990','#dcbeff','#9A6324','#800000','#808000','#000075','#a9a9a9'];
let timer = null;

async function startSolve(ev) {
  ev.preventDefault();
  if (timer) { clearInterval(timer); timer = null; }
  const body = {
    generate: {
      count: +document.getElementById('count').value,
      vehicles: +document.getElementById('vehicles').value,
      seed: +document.getElementById('seed').value,
    },
    restarts: +document.getElementById('restarts').value,
  };
  const res = await fetch('/api/solve', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  if (!res.ok) { document.getElementById('status').textContent = 'error'; return; }
  const { solveId } = await res.json();
  document.getElementById('status').textContent = 'running';
  timer = setInterval(() => poll(solveId), 400);
  poll(solveId);
}

async function poll(solveId) {
  const res = await fetch('/api/solve/' + solveId);
  if (!res.ok) return;
  const state = await res.json();
  document.getElementById('distance').textContent = state.bestDistance > 0 ? state.bestDistance.toFixed(1) : '—';
  document.getElementById('progress').textContent = state.restartsDone + '/' + state.restartsTotal;
  document.getElementById('improvements').textContent = state.improvements;
  document.getElementById('status').textContent = state.status;
  draw(state);
  if (state.status !== 'running' && timer) { clearInterval(timer); timer = null; }
}

function draw(state) {
  const canvas = document.getElementById('canvas');
  const ctx = canvas.getContext('2d');
  const W = canvas.width, H = canvas.height, pad = 24;
  ctx.clearRect(0, 0, W, H);
  const stops = state.stops;
  if (!stops || !stops.length) return;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const s of stops) { minX = Math.min(minX, s.x); minY = Math.min(minY, s.y); maxX = Math.max(maxX, s.x); maxY = Math.max(maxY, s.y); }
  const sx = (maxX > minX) ? (W - 2 * pad) / (maxX - minX) : 1;
  const sy = (maxY > minY) ? (H - 2 * pad) / (maxY - minY) : 1;
  const px = s => pad + (s.x - minX) * sx;
  const py = s => pad + (s.y - minY) * sy;

  // Routes first (so stops draw on top).
  (state.routes || []).forEach((route, ri) => {
    if (route.length < 2) return;
    ctx.strokeStyle = palette[ri % palette.length];
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i <= route.length; i++) {
      const s = stops[route[i % route.length]];
      const X = px(s), Y = py(s);
      if (i === 0) ctx.moveTo(X, Y); else ctx.lineTo(X, Y);
    }
    ctx.stroke();
  });

  // Stops.
  for (let i = 0; i < stops.length; i++) {
    const s = stops[i];
    const X = px(s), Y = py(s);
    if (i === state.depotIndex) {
      ctx.fillStyle = '#111';
      ctx.fillRect(X - 4, Y - 4, 8, 8);
    } else {
      ctx.fillStyle = '#3478f6';
      ctx.beginPath();
      ctx.arc(X, Y, 2.5, 0, Math.PI * 2);
      ctx.fill();
    }
  }
}

document.getElementById('form').addEventListener('submit', startSolve);
</script>
</body>
</html>"#;
