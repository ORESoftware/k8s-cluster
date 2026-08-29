/// Interactive landing page. All `fetch`/link URLs are RELATIVE so the page
/// works both at `/` (local `cargo run`) and behind the gateway at `/des-rs/`
/// (which strips the prefix). "Run" buttons hit `simulations/<name>/run?exact=1`
/// so a click runs exactly one catalogue entry.
pub(crate) const LANDING_HTML: &str = r####"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>discrete-event-system.rs — DES engine</title>
<style>
:root{color-scheme:dark;--bg:#0b1021;--panel:#0f1422;--line:#21262d;--ink:#e6edf3;--dim:#9aa4b2;--accent:#1f6feb;--accent2:#388bfd;--ok:#3fb950;--err:#f85149}
*{box-sizing:border-box}
body{font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;margin:0;color:var(--ink);
  background:
    radial-gradient(1100px 520px at 12% -8%,rgba(31,111,235,.20),transparent 60%),
    radial-gradient(900px 480px at 96% 0%,rgba(126,231,135,.10),transparent 55%),
    var(--bg)}
main{max-width:1060px;margin:0 auto;padding:30px 22px 80px}
.hero{border:1px solid var(--line);border-radius:16px;padding:24px 24px 22px;margin:6px 0 28px;
  background:linear-gradient(180deg,rgba(31,111,235,.10),rgba(15,20,34,.6));backdrop-filter:blur(2px)}
.hero .top{display:flex;align-items:center;gap:12px;flex-wrap:wrap}
h1{font-size:1.8rem;margin:0;letter-spacing:-.01em;background:linear-gradient(90deg,#e6edf3,#9ecbff);-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent}
.pill-health{display:inline-flex;align-items:center;gap:7px;font-size:.78rem;color:var(--dim);border:1px solid var(--line);border-radius:999px;padding:4px 11px;background:#0b1021}
.pill-health .dot{width:8px;height:8px;border-radius:50%;background:#6b7689;box-shadow:0 0 0 0 rgba(63,185,80,.5)}
.pill-health.up .dot{background:var(--ok);animation:pulse 2.2s infinite}
.pill-health.down .dot{background:var(--err)}
.pill-health.up{color:#b7f0c2;border-color:rgba(63,185,80,.35)}
@keyframes pulse{0%{box-shadow:0 0 0 0 rgba(63,185,80,.45)}70%{box-shadow:0 0 0 7px rgba(63,185,80,0)}100%{box-shadow:0 0 0 0 rgba(63,185,80,0)}}
.sub{color:var(--dim);margin:12px 0 16px;font-size:.95rem;line-height:1.55;max-width:74ch}
.actions{display:flex;gap:10px;flex-wrap:wrap;margin:0}
a.btn,button.btn{font:inherit;font-size:.9rem;cursor:pointer;border-radius:8px;padding:9px 14px;text-decoration:none;border:1px solid #2b3344;background:#161b22;color:#e6edf3}
a.btn.primary{background:#1f6feb;border-color:#1f6feb;color:#fff}
a.btn:hover,button.btn:hover{border-color:#3b82f6}
h2{font-size:1.06rem;margin:34px 0 12px;color:#c9d4e3;display:flex;align-items:center;gap:9px}
h2::before{content:"";width:4px;height:16px;border-radius:3px;background:linear-gradient(180deg,var(--accent),var(--accent2))}
.muted{color:#6b7689;font-weight:400;font-size:.85rem}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(244px,1fr));gap:12px}
.sim{display:flex;flex-direction:column;gap:8px;border:1px solid var(--line);border-radius:12px;padding:14px;background:var(--panel);transition:transform .14s ease,border-color .14s ease,box-shadow .14s ease}
.sim:hover{transform:translateY(-2px);border-color:#30496f;box-shadow:0 10px 26px rgba(0,0,0,.35)}
.sim.feat{background:linear-gradient(180deg,rgba(31,111,235,.07),var(--panel))}
.sim .label{font-size:.92rem;text-transform:capitalize}
.sim .name{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.78rem;color:#9ecbff;word-break:break-all}
.sim .desc{font-size:.8rem;color:#8b949e;line-height:1.45;flex:1}
.sim .row{display:flex;align-items:center;gap:8px;justify-content:flex-end;flex-wrap:wrap;margin-top:2px}
.sim .links{display:flex;align-items:center;gap:6px;flex-wrap:wrap}
.sim .links:empty{display:none}
.sim .open{font:inherit;font-size:.82rem;cursor:pointer;border-radius:7px;padding:6px 14px;border:1px solid #1f6feb;background:#1f6feb;color:#fff;text-decoration:none}
.sim .open:hover{background:#388bfd;border-color:#388bfd}
.sim .json{font:inherit;font-size:.8rem;border-radius:7px;padding:6px 10px;border:1px solid #2b3344;background:#161b22;color:#9aa4b2;text-decoration:none}
.sim .json:hover{border-color:#3b82f6;color:#e6edf3}
.list{display:flex;flex-direction:column;gap:8px}
.strow{border:1px solid #21262d;border-radius:9px;padding:10px 14px;background:#0f1422;font-size:.86rem;line-height:1.45;color:#c9d4e3}
.strow code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;color:#9ecbff;background:#161b22;padding:1px 7px;border-radius:5px}
.strow .ops{color:#6b7689;font-size:.78rem;margin-left:6px}
.run{font:inherit;font-size:.82rem;cursor:pointer;border-radius:7px;padding:6px 14px;border:1px solid #238636;background:#238636;color:#fff}
.run:hover{background:#2ea043}
.run[disabled]{opacity:.55;cursor:default}
.st{font-size:.8rem;color:#9aa4b2;min-height:1.1em;flex:1;min-width:86px}
.st.ok{color:#3fb950}.st.err{color:#f85149}
.filterbar{position:sticky;top:0;z-index:5;display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin:0 0 12px;padding:10px 0;background:linear-gradient(180deg,var(--bg) 70%,rgba(11,16,33,0))}
#filter{font:inherit;font-size:.9rem;background:#0f1422;border:1px solid var(--line);border-radius:8px;color:var(--ink);padding:9px 12px;width:280px;max-width:60vw}
#filter:focus{outline:none;border-color:var(--accent);box-shadow:0 0 0 3px rgba(31,111,235,.25)}
.filterbar .kbd{font-family:ui-monospace,Menlo,monospace;font-size:.72rem;color:#8b949e;border:1px solid var(--line);border-bottom-width:2px;border-radius:6px;padding:1px 6px;background:#0b1021}
.filterbar .shown{color:#8b949e;font-size:.82rem}
.toast{position:fixed;left:50%;bottom:26px;transform:translateX(-50%) translateY(160%);transition:transform .25s;background:#161b22;border:1px solid #2b3344;border-radius:10px;padding:12px 18px;box-shadow:0 8px 30px rgba(0,0,0,.5);font-size:.9rem}
.toast.show{transform:translateX(-50%) translateY(0)}
.toast a{color:#58a6ff}
</style>
</head>
<body>
<main>
<div class="hero">
  <div class="top">
    <h1>discrete-event-system.rs</h1>
    <span id="health" class="pill-health"><span class="dot"></span><span class="txt">checking…</span></span>
  </div>
  <p class="sub">A Rust modeling &amp; simulation engine, imported here as a <strong>library</strong> (git submodule) and run <strong>in-process</strong>. Run a <strong>first-class model</strong> for an interactive player, stream commands to a <strong>solver</strong>, or run any catalogue <strong>simulation</strong> and open the rendered HTML/JSON.</p>
  <div class="actions">
    <a class="btn primary" href="out/">All rendered results &rarr;</a>
    <a class="btn" href="docs/api">API docs</a>
    <a class="btn" href="info">Service info</a>
    <a class="btn" href="models">Models JSON</a>
    <a class="btn" href="streaming">Streaming JSON</a>
    <a class="btn" href="music">Music production</a>
    <a class="btn" href="simulations">Catalogue JSON</a>
  </div>
</div>
<h2>Elevator <span class="muted">— next-event (FEL) sim + dispatch decision models</span></h2>
<div class="grid">
  <div class="sim">
    <div class="label">FEL elevator</div>
    <div class="name">des::fel::elevator</div>
    <div class="desc">A next-event single-car elevator under a LOOK (collective-control) policy. The clock jumps event-to-event (arrival / car-step / doors-close) and skips idle time. Animated shaft with boarding/alighting and live charts.</div>
    <div class="row"><a class="open" href="elevator-fel" target="_blank" rel="noopener">Open animation &#8599;</a></div>
  </div>
  <div class="sim">
    <div class="label">Elevator dispatch MDP</div>
    <div class="name">des/mdp/v1 &middot; value-iteration</div>
    <div class="desc">Fully-observed dispatch as a 12-state MDP (car floor &times; pending call). Value iteration recovers the drive-to-the-call-and-serve policy, animated over the state graph.</div>
    <div class="row"><a class="open" href="elevator-mdp" target="_blank" rel="noopener">Open player &#8599;</a></div>
  </div>
  <div class="sim">
    <div class="label">Elevator dispatch POMDP</div>
    <div class="name">des/pomdp/v1 &middot; belief tracking</div>
    <div class="desc">Dispatch under a noisy hall-call button: hidden demand is empty / waiting / crowded and the button false-triggers and misses. Belief over hidden states drives the hold-vs-dispatch decision.</div>
    <div class="row"><a class="open" href="elevator-pomdp" target="_blank" rel="noopener">Open player &#8599;</a></div>
  </div>
</div>
<h2>Household <span class="muted">— blocking-loss queueing as a Monte-Carlo DES</span></h2>
<div class="grid">
  <div class="sim">
    <div class="label">Bathroom occupancy</div>
    <div class="name">des::bathrooms &middot; finite-source loss</div>
    <div class="desc">8 people share 2 bathrooms, each visiting 3&times;/day for 20 min at random times; if both are busy the arrival is rejected. A Monte-Carlo discrete-event sim recovers P(none / one / both occupied) and checks it against the closed-form binomial. Animated house, gantt scrubber, and convergence chart.</div>
    <div class="row"><a class="open" href="bathrooms" target="_blank" rel="noopener">Open animation &#8599;</a></div>
  </div>
  <div class="sim">
    <div class="label">Bathroom occupancy (framework build)</div>
    <div class="name">des::two_bathrooms &middot; MovingEntity + visual blocks</div>
    <div class="desc">The same 8-people / 2-bathrooms loss system, re-built on the engine's reusable frameworks: people are <code>MovingEntity</code> tokens flowing through <code>StationaryEntity</code> bathrooms, rendered as visual blocks through the shared animation player (the same one the elevator/traffic scenes use), with an occupancy time-series and the binomial-vs-simulated stats table.</div>
    <div class="row"><a class="open" href="two-bathrooms" target="_blank" rel="noopener">Open animation &#8599;</a></div>
  </div>
</div>
<h2>Soccer <span class="muted">— videogame, learning sim, rotation planner</span></h2>
<div class="grid">
  <div class="sim feat">
    <div class="label">Soccer videogame</div>
    <div class="name">out/soccer-sim.html &middot; json/jsonl</div>
    <div class="desc">Playable 2D 11v11 match artifact with MDP/POMDP player learning, ball physics, possession chains, shots, officials, and controller slots.</div>
    <div class="row">
      <a class="open" href="soccer/live" target="_blank" rel="noopener">Live game &#8599;</a>
      <a class="open" href="out/soccer-sim.html" target="_blank" rel="noopener">Static game &#8599;</a>
      <a class="open" href="out/soccer-sim.meta.json" target="_blank" rel="noopener">Metadata JSON &#8599;</a>
      <a class="open" href="out/soccer-sim.frames.jsonl" target="_blank" rel="noopener">Frames JSONL &#8599;</a>
    </div>
  </div>
  <div class="sim feat">
    <div class="label">Interactive planner</div>
    <div class="name">soccer/planner</div>
    <div class="desc">11-a-side (4-4-2), max 7 subs. Mark players AWOL/injured/guest, lock positions, ban roles, set per-position scores and chemistry rules (9/10 if partner in slot Y). Re-solve with IP/MIP; toggle Pitch vs solver view.</div>
    <div class="row"><a class="open" href="soccer/planner" target="_blank" rel="noopener">Open planner &#8599;</a></div>
  </div>
</div>
<h2>Music production <span class="muted">— microtonal generator, breakbeat/DnB album runs, sample-seed workflow</span></h2>
<div class="grid">
  <div class="sim feat">
    <div class="label">Generative song workbench</div>
    <div class="name">music-production</div>
    <div class="desc">Build 3-minute instrumental albums with FFT-backed analysis, synthetic instrument discovery, meter changes, drum fills, reduced percussion gain, and a 10-50s MP4 music-sample-seed variation path.</div>
    <div class="row"><a class="open" href="music" target="_blank" rel="noopener">Open workbench &#8599;</a></div>
  </div>
</div>
<h2>Control &amp; estimation <span class="muted">— back-EMF DC motor, controllability/observability, shadow Gramians</span></h2>
<div id="control" class="grid"></div>
<h2>First-class models <span class="muted">— describe &rarr; run &rarr; interactive player</span></h2>
<div id="models" class="grid"></div>
<h2>Streaming solvers <span class="muted">— JSONL commands in, JSONL frames out</span></h2>
<div id="streaming" class="list"></div>
<h2>Featured</h2>
<div id="featured" class="grid"></div>
<h2>All simulations <span id="count" class="muted"></span></h2>
<div class="filterbar">
  <input id="filter" placeholder="filter by name…" oninput="filterSims()" autocomplete="off" spellcheck="false">
  <span class="kbd">/</span><span class="shown" id="shown"></span>
</div>
<div id="all" class="grid"></div>
</main>
<div id="toast" class="toast"></div>
<script>
const FEATURED=[["main_factory_floor_track3t","Track3t warehouse"],["main_build_site","Build site index"],["main_elevator_highrise","Elevator high-rise"],["main_factmachine_markets","FactMachine markets"],["main_two_disease","Two-disease epidemic"],["main_electric_circuit","Electric circuit"],["main_traffic","Traffic network"],["main_court_mdp","Court MDP"],["main_convolution","Convolution"]];
const CONTROL=[
  ["main_shadow_eval","Shadow Gramians","Probe each plant as a black box: recover controllability/observability Gramians from perturbed shadow copies, cross-check against the analytic model, then re-ask via a nested MDP/POMDP of the motor's speed regimes."],
  ["main_observability_controllability_anim","Obs / ctrl (animated)","Kalman rank tests for controllability & observability of a state-space model, animated step by step."],
  ["main_empirical_control_report","Empirical control report","Monte-Carlo trials + Gramian eigenvalue degrees that quantify how much control and observation authority a system actually has."],
  ["main_dc_motor_anim","DC motor (back-EMF)","Separately-excited DC motor with explicit back-EMF coupling (E = K_e·ω), RK4-integrated and animated."],
  ["main_wind_mppt_anim","Wind MPPT","Maximum-power-point-tracking controller on a wind turbine, animated."]
];
function toast(html){const t=document.getElementById('toast');t.innerHTML=html;t.classList.add('show');clearTimeout(window.__tt);window.__tt=setTimeout(function(){t.classList.remove('show');},6000);}
function esc(s){return String(s||'').replace(/[<>&"]/g,function(ch){return {'<':'&lt;','>':'&gt;','&':'&amp;','"':'&quot;'}[ch];});}
function shortName(href){return String(href||'').split('/').filter(Boolean).pop()||href;}
function artifactAnchor(href,label,cls){return '<a class="'+cls+'" href="'+esc(href)+'" target="_blank" rel="noopener">'+esc(label)+' &#8599;</a>';}
function artifactButtons(artifacts){
  artifacts=artifacts||{};
  const html=(artifacts.html||[]).slice(0,3);
  const json=(artifacts.json||[]).slice(0,3);
  const jsonl=(artifacts.jsonl||[]).slice(0,3);
  const out=[];
  html.forEach(function(h,i){out.push(artifactAnchor(h,i===0?'View results':shortName(h),'open'));});
  json.forEach(function(h){out.push(artifactAnchor(h,'JSON','json'));});
  jsonl.forEach(function(h){out.push(artifactAnchor(h,'JSONL','json'));});
  return out.join('');
}
function setArtifactLinks(el,artifacts){
  if(!el)return;
  el.innerHTML=artifactButtons(artifacts);
}
function simCard(name,label,desc,feat){
  const card=document.createElement('div');card.className=feat?'sim feat':'sim';card.dataset.name=name;
  const lab=document.createElement('div');lab.className='label';lab.textContent=label||name;
  const nm=document.createElement('div');nm.className='name';nm.textContent=name;
  card.appendChild(lab);card.appendChild(nm);
  if(desc){const d=document.createElement('div');d.className='desc';d.textContent=desc;card.appendChild(d);}
  const row=document.createElement('div');row.className='row';
  const st=document.createElement('span');st.className='st';
  const links=document.createElement('span');links.className='links';
  const btn=document.createElement('button');btn.className='run';btn.textContent='Run';
  btn.onclick=function(){run(name,btn,st,links);};
  row.appendChild(st);row.appendChild(links);row.appendChild(btn);
  card.appendChild(row);
  return card;
}
async function run(name,btn,st,links){
  btn.disabled=true;const old=btn.textContent;btn.textContent='Running…';st.className='st';st.textContent='running…';
  setArtifactLinks(links,null);
  try{
    const r=await fetch('simulations/'+encodeURIComponent(name)+'/run?exact=1');
    const d=await r.json();
    const o=(d.ran&&d.ran[0])||{};
    if(d.ok){
      const artifacts=o.artifacts||d.artifacts||{};
      const primary=artifacts.primary||'out/';
      setArtifactLinks(links,artifacts);
      st.className='st ok';st.textContent='\u2713 '+(o.millis!=null?o.millis+' ms':'done');
      const buttons=artifactButtons(artifacts)||('<a href="'+esc(primary)+'">view results &rarr;</a>');
      toast('Ran <code>'+esc(name)+'</code> — '+buttons);
    }
    else{st.className='st err';st.textContent='\u2717 '+(d.error||'failed');}
  }catch(e){st.className='st err';st.textContent='\u2717 '+e;}
  finally{btn.disabled=false;btn.textContent=old;}
}
function filterSims(){
  const q=document.getElementById('filter').value.toLowerCase();
  let shown=0,total=0;
  document.querySelectorAll('#all .sim').forEach(function(c){total++;const m=c.dataset.name.indexOf(q)>=0;c.style.display=m?'':'none';if(m)shown++;});
  document.getElementById('shown').textContent=q?(shown+' / '+total+' shown'):'';
}
function modelCard(m){
  const card=document.createElement('div');card.className='sim';
  const lab=document.createElement('div');lab.className='label';lab.textContent=m.title||m.kind;
  const nm=document.createElement('div');nm.className='name';nm.textContent=m.kind+' \u00b7 '+(m.specSchema||'')+(m.methods&&m.methods.length?' \u00b7 '+m.methods.join(', '):'');
  const desc=document.createElement('div');desc.className='desc';desc.textContent=m.description||'';
  const row=document.createElement('div');row.className='row';
  const js=document.createElement('a');js.className='json';js.textContent='JSON';js.href='models/'+encodeURIComponent(m.kind)+'/run?format=json';js.target='_blank';js.rel='noopener';
  const open=document.createElement('a');open.className='open';open.textContent='Open player \u2197';open.href='models/'+encodeURIComponent(m.kind)+'/run';open.target='_blank';open.rel='noopener';
  row.appendChild(js);row.appendChild(open);
  card.appendChild(lab);card.appendChild(nm);card.appendChild(desc);card.appendChild(row);
  return card;
}
function streamRow(c){
  const row=document.createElement('div');row.className='strow';
  const ops=(c.inputOps&&c.inputOps.length)||0;
  row.innerHTML='POST <code>streaming/'+c.model+'</code><span class="ops">'+ops+' command op(s)</span><br>'+
    (c.description||'').replace(/[<>&]/g,function(ch){return {'<':'&lt;','>':'&gt;','&':'&amp;'}[ch];});
  return row;
}
(async function(){
  try{
    const r=await fetch('models');const d=await r.json();
    const wrap=document.getElementById('models');
    (d.models||[]).forEach(function(m){wrap.appendChild(modelCard(m));});
  }catch(e){document.getElementById('models').textContent='failed to load models';}
  try{
    const r=await fetch('streaming');const d=await r.json();
    const wrap=document.getElementById('streaming');
    (d.streaming||[]).forEach(function(c){wrap.appendChild(streamRow(c));});
  }catch(e){document.getElementById('streaming').textContent='failed to load streaming contracts';}
})();
(function(){
  const c=document.getElementById('control');
  CONTROL.forEach(function(p){c.appendChild(simCard(p[0],p[1],p[2],true));});
})();
(async function(){
  const f=document.getElementById('featured');
  FEATURED.forEach(function(p){f.appendChild(simCard(p[0],p[1],null,true));});
  try{
    const r=await fetch('simulations');const d=await r.json();
    document.getElementById('count').textContent='('+d.count+')';
    const all=document.getElementById('all');
    d.simulations.forEach(function(n){all.appendChild(simCard(n,n.replace(/^main_/,'').replace(/_/g,' ')));});
  }catch(e){document.getElementById('count').textContent='(failed to load)';}
})();
(async function(){
  const el=document.getElementById('health');
  try{
    const r=await fetch('healthz',{cache:'no-store'});
    const d=await r.json();
    el.className='pill-health '+(d&&d.ok?'up':'down');
    el.querySelector('.txt').textContent=d&&d.ok?'healthy':'unhealthy';
  }catch(e){el.className='pill-health down';el.querySelector('.txt').textContent='offline';}
})();
document.addEventListener('keydown',function(e){
  const fi=document.getElementById('filter');
  if(e.key==='/'&&document.activeElement!==fi){e.preventDefault();fi.focus();fi.select();}
  else if(e.key==='Escape'&&document.activeElement===fi){fi.value='';filterSims();fi.blur();}
});
</script>
</body>
</html>"####;

pub(crate) const MUSIC_PRODUCTION_HTML: &str = r####"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>DES music production</title>
<style>
:root{color-scheme:dark;--bg:#090d16;--panel:#101724;--panel2:#151b24;--line:#273140;--ink:#eef3f8;--dim:#9ba7b5;--accent:#24a0ed;--hot:#e65f7a;--ok:#39d98a}
*{box-sizing:border-box}
body{margin:0;background:linear-gradient(180deg,#090d16,#10151f 48%,#0c1018);color:var(--ink);font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
main{max-width:1180px;margin:0 auto;padding:24px 18px 56px}
.top{display:flex;align-items:flex-start;justify-content:space-between;gap:14px;margin-bottom:18px}
.crumb{color:var(--dim);text-decoration:none;font-size:.86rem}
h1{font-size:1.65rem;margin:3px 0 7px;letter-spacing:0}
.sub{max-width:76ch;color:var(--dim);line-height:1.5;margin:0}
.pill{display:inline-flex;align-items:center;gap:7px;border:1px solid var(--line);border-radius:999px;padding:6px 10px;background:#0e141e;color:#b9c6d4;font-size:.78rem;white-space:nowrap}
.dot{width:8px;height:8px;border-radius:50%;background:var(--ok)}
.layout{display:grid;grid-template-columns:minmax(280px,360px) 1fr;gap:14px;align-items:start}
.panel{border:1px solid var(--line);background:var(--panel);border-radius:12px;padding:15px}
.panel h2{font-size:.96rem;margin:0 0 12px;color:#dce7f2;letter-spacing:0}
label{display:block;color:#bac5d1;font-size:.8rem;margin:12px 0 5px}
input,select,textarea{width:100%;font:inherit;color:var(--ink);background:#0b111b;border:1px solid #2d3949;border-radius:8px;padding:9px 10px}
textarea{min-height:108px;resize:vertical}
input[type=range]{padding:0;accent-color:var(--accent)}
.row{display:grid;grid-template-columns:1fr 1fr;gap:10px}
.actions{display:flex;gap:9px;flex-wrap:wrap;margin-top:14px}
button,a.btn{font:inherit;font-size:.86rem;border-radius:8px;border:1px solid #344153;background:#151d29;color:#eef3f8;padding:9px 12px;text-decoration:none;cursor:pointer}
button.primary{background:var(--accent);border-color:var(--accent);color:#041018}
button:hover,a.btn:hover{border-color:#58b8f2}
.meters{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-bottom:14px}
.meter{border:1px solid var(--line);background:var(--panel2);border-radius:10px;padding:12px;min-height:74px}
.meter b{display:block;font-size:1.12rem;margin-bottom:5px;color:#fff}
.meter span{font-size:.75rem;color:var(--dim)}
.wide{display:grid;grid-template-columns:1fr 1fr;gap:14px}
pre{margin:0;white-space:pre-wrap;word-break:break-word;background:#080d14;border:1px solid #263242;border-radius:10px;padding:13px;color:#c9e6ff;font-size:.82rem;line-height:1.45;min-height:132px}
.timeline{height:76px;border:1px solid var(--line);border-radius:10px;background:#0b111b;position:relative;overflow:hidden;margin-top:6px}
.section{position:absolute;top:0;bottom:0;border-right:1px solid rgba(255,255,255,.18)}
.section:nth-child(1){background:#1b4d89}.section:nth-child(2){background:#75415f}.section:nth-child(3){background:#2d705d}.section:nth-child(4){background:#725d24}.section:nth-child(5){background:#41386b}
.section span{position:absolute;left:8px;bottom:7px;font-size:.7rem;color:#f3f7fb}
.seed-status{font-size:.8rem;color:var(--dim);min-height:1.2em;margin-top:8px}
.seed-status.ok{color:var(--ok)}.seed-status.err{color:var(--hot)}
.auth-panel{border:1px solid var(--line);background:#0c131d;border-radius:8px;padding:10px;margin-top:10px}
.auth-panel[hidden]{display:none}
.auth-panel textarea{min-height:74px}
.result{font-size:.8rem;color:#c9e6ff;line-height:1.45;margin-top:10px;word-break:break-word}
.result a{color:#76c7ff}
.result.ok{color:var(--ok)}.result.err{color:var(--hot)}
.note{font-size:.78rem;color:var(--dim);line-height:1.45;margin-top:10px}
@media(max-width:860px){.layout,.wide{grid-template-columns:1fr}.meters{grid-template-columns:1fr 1fr}.top{flex-direction:column}}
</style>
</head>
<body>
<main>
  <div class="top">
    <div>
      <a class="crumb" href="./">des-rs</a>
      <h1>Music Production Workbench</h1>
      <p class="sub">Microtonal, mostly instrumental generation with synthetic instruments, FFT spectrum checks, breakbeat and drum-n-bass album recipes, richer meter changes, legal sample provenance, and a 10-50 second MP4/link music-sample-seed path.</p>
    </div>
    <span class="pill"><span class="dot"></span>ready for local renders</span>
  </div>

  <div class="layout">
    <section class="panel">
      <h2>Render Setup</h2>
      <label for="mode">Mode</label>
      <select id="mode">
        <option value="album-more">10-track breakbeat/DnB album</option>
        <option value="album">10-track broad microtonal album</option>
        <option value="sample">MP4 music-sample-seed variation</option>
      </select>
      <div class="row">
        <div>
          <label for="seed">Seed</label>
          <input id="seed" value="20260602" inputmode="numeric">
        </div>
        <div>
          <label for="duration">Song seconds</label>
          <input id="duration" type="number" min="30" max="480" step="1" value="180">
        </div>
      </div>
      <label for="percussion">Main percussion gain</label>
      <input id="percussion" type="range" min="80" max="90" value="84">
	      <label for="variation">Drum variation target</label>
	      <input id="variation" type="range" min="10" max="20" value="10">
	      <label for="sourceUrl">Link seed</label>
	      <input id="sourceUrl" type="url" placeholder="https://www.youtube.com/watch?v=... or https://x.com/...">
	      <div class="row">
	        <div>
	          <label for="sourceAccess">Link access</label>
	          <select id="sourceAccess">
	            <option value="public">Public</option>
	            <option value="authenticated">Authenticated</option>
	          </select>
	        </div>
	        <div>
	          <label for="sourcePlatform">Source</label>
	          <select id="sourcePlatform">
	            <option value="auto">Auto</option>
	            <option value="youtube">YouTube</option>
	            <option value="x">X.com</option>
	            <option value="facebook">Facebook</option>
	            <option value="instagram">Instagram</option>
	            <option value="direct">Direct media</option>
	          </select>
	        </div>
	      </div>
	      <div id="authPanel" class="auth-panel" hidden>
	        <label for="authHeaderName">Source auth header name</label>
	        <input id="authHeaderName" autocomplete="off" spellcheck="false" value="Authorization">
	        <label for="authHeader">Source auth header value</label>
	        <input id="authHeader" type="password" autocomplete="off" spellcheck="false" placeholder="shared secret or bearer token">
	        <label for="cookieHeader">Cookie header</label>
	        <textarea id="cookieHeader" autocomplete="off" spellcheck="false" placeholder="name=value; name2=value2"></textarea>
	        <label for="sourceCookies">yt-dlp cookies.txt</label>
	        <input id="sourceCookies" type="file" accept=".txt,text/plain">
	      </div>
	      <label for="sample">MP4 seed upload (10-50s)</label>
	      <input id="sample" type="file" accept="video/mp4,audio/mp4,.mp4">
      <div id="sampleStatus" class="seed-status"></div>
      <label for="prompt">Prompt / direction</label>
      <textarea id="prompt" placeholder="Expand the seed into faster jungle, keep the melody bright, use 13/16 stutter fills, soften the main drums, add massive synth pressure."></textarea>
      <div class="actions">
        <button class="primary" onclick="renderSampleSeed()">Render via server</button>
        <button onclick="update()">Refresh recipe</button>
        <button onclick="copyCommand()">Copy command</button>
        <a class="btn" href="out/" target="_blank" rel="noopener">Open output</a>
      </div>
	      <div id="serverResult" class="result"></div>
	      <p class="note">Use sources you own or are licensed to transform. Links use direct HTTP for media files and yt-dlp when available for YouTube, Facebook, Instagram, X, and similar pages.</p>
    </section>

    <div>
      <div class="meters">
        <div class="meter"><b id="tracks">10</b><span>tracks</span></div>
        <div class="meter"><b id="meter">7/8+</b><span>meter changes</span></div>
        <div class="meter"><b id="drums">10%</b><span>less repetition target</span></div>
        <div class="meter"><b id="gain">0.84</b><span>percussion gain</span></div>
      </div>
      <section class="panel">
        <h2>Song Shape</h2>
        <div class="timeline" aria-label="song timeline">
          <div class="section" style="left:0;width:16%"><span>intro</span></div>
          <div class="section" style="left:16%;width:22%"><span>pressure</span></div>
          <div class="section" style="left:38%;width:28%"><span>collage</span></div>
          <div class="section" style="left:66%;width:20%"><span>swerve</span></div>
          <div class="section" style="left:86%;width:14%"><span>outro</span></div>
        </div>
      </section>
      <div class="wide" style="margin-top:14px">
        <section class="panel">
          <h2>Command</h2>
          <pre id="command"></pre>
        </section>
        <section class="panel">
          <h2>Manifest Preview</h2>
          <pre id="manifest"></pre>
        </section>
      </div>
    </div>
  </div>
</main>
<script>
const $=id=>document.getElementById(id);
let sampleOk=false;
let sampleName="";
const savedPrompt=localStorage.getItem("desMusicPrompt")||"";
$("prompt").value=savedPrompt;
function clampInt(value,fallback){const n=parseInt(value,10);return Number.isFinite(n)?n:fallback;}
function shellQuote(value){return "'"+String(value).replace(/'/g,"'\\''")+"'";}
function hashText(value){
  let h=0x811c9dc5;
  for(const ch of value){h^=ch.charCodeAt(0);h=Math.imul(h,0x01000193)>>>0;}
  return h>>>0;
}
function promptTags(value){
  const l=value.toLowerCase();
  const tags=[];
  [["expand",["expand","longer arc","build out"]],["alter",["alter","mutate","transform"]],["slice",["slice","chop","cut-up","collage"]],["melody",["melody","melodic","hook","theme"]],["massive-synth",["massive synth","big synth","wall of synth"]],["space",["space","reverb","wide","dub"]],["less-drums",["less drums","softer drums","lower drums"]],["more-drums",["more drums","drum fills","busier drums"]]].forEach(([tag,words])=>{if(words.some(w=>l.includes(w)))tags.push(tag);});
  return tags;
}
function promptText(){return $("prompt").value.trim();}
function sourceUrl(){return $("sourceUrl").value.trim();}
function sourceAccess(){return $("sourceAccess").value;}
function sourcePlatform(){return $("sourcePlatform").value;}
function authCredentials(){
  const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
  const authHeaderName=($("authHeaderName").value.trim()||"Authorization");
  const authHeader=$("authHeader").value.trim();
  const cookieHeader=$("cookieHeader").value.trim();
  return {
    auth_header_name: authHeaderName,
    auth_header: Boolean(authHeader),
    cookie_header: Boolean(cookieHeader),
    cookies_file: cookieFile ? cookieFile.name : null,
    has: Boolean(authHeader||cookieHeader||cookieFile)
  };
}
function updateAuthVisibility(){
  $("authPanel").hidden=sourceAccess()!=="authenticated";
}
function command(){
  const mode=$("mode").value;
  const duration=clampInt($("duration").value,180);
  const seed=clampInt($("seed").value,20260602);
  if(mode==="sample"){
    const url=sourceUrl();
    const source=url?"out/music-sample-seed-source.mp4":(sampleName||"/absolute/path/to/seed.mp4");
    const prompt=promptText();
    const access=sourceAccess();
    const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
    const authHeaderName=($("authHeaderName").value.trim()||"Authorization");
    const authHeader=$("authHeader").value.trim();
    const promptPath="out/music-sample-seed-prompt.txt";
    const cookieFlag=access==="authenticated"?` --cookies "${cookieFile?cookieFile.name:"/absolute/path/to/cookies.txt"}"`:"";
    const headerFlag=access==="authenticated"&&authHeader?` --add-header ${shellQuote(authHeaderName+": "+authHeader)}`:"";
    const urlPrefix=url?`mkdir -p out\nyt-dlp --no-playlist --force-overwrites --merge-output-format mp4${cookieFlag}${headerFlag} -o ${source} ${shellQuote(url)}\n`:"";
    const promptPrefix=prompt?`mkdir -p out\nprintf %s ${shellQuote(prompt)} > ${promptPath}\n`:"";
    const promptFlag=prompt?` --prompt-file ${promptPath}`:"";
    return `${urlPrefix}${promptPrefix}cargo run --bin main_music_production -- --sample-seed "${source}" out/music-sample-seed-variation.wav ${duration}${promptFlag}`;
  }
  const out=mode==="album-more"?"out/music-production-ten-more-breaks":"out/music-production-ten-songs";
  const flag=mode==="album-more"?"--album-more":"--album";
  return `cargo run --bin main_music_production -- ${flag} ${out} ${seed} ${duration}`;
}
function update(){
  updateAuthVisibility();
  const mode=$("mode").value;
  const duration=clampInt($("duration").value,180);
  const percussion=(clampInt($("percussion").value,84)/100).toFixed(2);
  const variation=clampInt($("variation").value,10);
  const prompt=promptText();
  const url=sourceUrl();
  const access=sourceAccess();
  const auth=authCredentials();
  localStorage.setItem("desMusicPrompt", $("prompt").value);
  $("tracks").textContent=mode==="sample"?"1":"10";
  $("drums").textContent=variation+"%";
  $("gain").textContent=percussion;
  $("meter").textContent=mode==="sample"?"seeded":"7/8+";
  $("command").textContent=command();
  $("manifest").textContent=JSON.stringify({
    mode,
    duration_seconds: duration,
    percussion_gain: Number(percussion),
    drum_repetition_reduction_target: variation/100,
    synthesis: ["microtonal", "pitch-bend", "FFT spectrum", "invented instruments"],
    http_endpoint: mode==="sample" ? "POST music/sample-seed" : null,
    sample_seed: mode==="sample" ? {
      required_seconds: "10-50",
      valid_loaded_file: sampleOk,
      file: sampleName || null,
      source_url: url || null,
      source_platform: sourcePlatform(),
      access,
      authenticated: access==="authenticated",
      auth: access==="authenticated" ? {
        auth_header_name: auth.auth_header_name,
        auth_header: auth.auth_header,
        cookie_header: auth.cookie_header,
        cookies_file: auth.cookies_file
      } : null
    } : null,
    prompt: prompt ? { chars: [...prompt].length, hash: hashText(prompt), tags: promptTags(prompt) } : null
  }, null, 2);
}
$("sample").addEventListener("change", function(){
  const file=this.files&&this.files[0];
  sampleOk=false;sampleName=file?file.name:"";
  const status=$("sampleStatus");
  if(!file){status.className="seed-status";status.textContent="";update();return;}
  const url=URL.createObjectURL(file);
  const video=document.createElement("video");
  video.preload="metadata";
  video.onloadedmetadata=function(){
    URL.revokeObjectURL(url);
    const d=video.duration||0;
    sampleOk=d>=10&&d<=50;
    status.className="seed-status "+(sampleOk?"ok":"err");
    status.textContent=sampleOk?`loaded ${file.name} (${d.toFixed(2)}s)`:`${file.name} is ${d.toFixed(2)}s; expected 10-50s`;
    update();
  };
  video.onerror=function(){URL.revokeObjectURL(url);status.className="seed-status err";status.textContent="could not read MP4 metadata";update();};
  video.src=url;
});
["mode","seed","duration","percussion","variation","prompt","sourceUrl","sourceAccess","sourcePlatform","authHeaderName","authHeader","cookieHeader"].forEach(id=>$(id).addEventListener("input",update));
$("sourceCookies").addEventListener("change",update);
async function copyCommand(){
  const text=command();
  try{await navigator.clipboard.writeText(text);$("command").textContent=text+"\n\ncopied";}
  catch(e){$("command").textContent=text;}
}
function escapeHtml(value){return String(value).replace(/[&<>"']/g,ch=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[ch]));}
async function renderSampleSeed(){
  const result=$("serverResult");
  const file=$("sample").files&&$("sample").files[0];
  const url=sourceUrl();
  if($("mode").value!=="sample"){
    result.className="result err";
    result.textContent="Switch mode to MP4 music-sample-seed variation.";
    return;
  }
  if(!file&&!url){
    result.className="result err";
    result.textContent="Choose a 10-50s MP4 seed or paste a public media link first.";
    return;
  }
  const access=sourceAccess();
  const auth=authCredentials();
  if(url&&access==="authenticated"&&!auth.has){
    result.className="result err";
    result.textContent="Add an Authorization header, Cookie header, or yt-dlp cookies.txt for an authenticated link.";
    return;
  }
  const fd=new FormData();
  if(file) fd.append("sample",file,file.name);
  if(url) fd.append("source_url",url);
  fd.append("source_auth_mode",access);
  fd.append("source_platform",sourcePlatform());
  const authHeaderName=($("authHeaderName").value.trim()||"Authorization");
  const authHeader=$("authHeader").value.trim();
  const cookieHeader=$("cookieHeader").value.trim();
  const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
  if(authHeaderName) fd.append("source_auth_header_name",authHeaderName);
  if(authHeader) fd.append("source_auth_header",authHeader);
  if(cookieHeader) fd.append("source_cookie_header",cookieHeader);
  if(cookieFile) fd.append("source_cookies",cookieFile,cookieFile.name);
  fd.append("prompt",$("prompt").value);
  fd.append("duration_seconds",String(clampInt($("duration").value,180)));
  fd.append("title","music-sample-seed variation");
  result.className="result";
  result.textContent="rendering on des-rs...";
  try{
    const r=await fetch("music/sample-seed",{method:"POST",body:fd});
    const d=await r.json();
    if(!r.ok||!d.ok){throw new Error(d.error||("HTTP "+r.status));}
    result.className="result ok";
    result.innerHTML=`Wrote <a href="${escapeHtml(d.wav_url)}" target="_blank" rel="noopener">${escapeHtml(d.wav_url)}</a><br>genre ${escapeHtml(d.summary.genre)} · bpm ${Number(d.summary.bpm).toFixed(1)} · prompt hash ${d.prompt&&d.prompt.hash?d.prompt.hash:"none"}`;
    $("manifest").textContent=JSON.stringify(d,null,2);
  }catch(e){
    result.className="result err";
    result.textContent="render failed: "+e.message;
  }
}
update();
</script>
</body>
</html>"####;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_page_links_live_soccer_simulation() {
        assert!(LANDING_HTML.contains("href=\"soccer/live\""));
        assert!(LANDING_HTML.contains("Live game"));
        assert!(LANDING_HTML.contains("href=\"out/soccer-sim.html\""));
        assert!(LANDING_HTML.contains("Static game"));
    }
}
