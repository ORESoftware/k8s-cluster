use serde_json::{json, Value};

use crate::catalog::*;
use crate::forecast::*;
use crate::recommendations::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn dashboard_payload(state: &AppState) -> Result<Value, String> {
    let series = snapshot_series_or_sample(state);
    let request = ForecastRequest {
        request_id: Some("dashboard".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        horizon_months: Some(state.config.projection_months),
        confidence_level: Some(state.config.confidence_level),
        scenario: Some("base".to_string()),
        series: Some(series.clone()),
        macro_context: None,
        macro_fiscal_context: Some(default_macro_fiscal_context()),
        venture_capital_context: Some(sample_venture_capital_context()),
        theory_weights: None,
    };
    let forecast = generate_forecast(&state.config, request)?;
    let recommendations = generate_recommendations(
        &state.config,
        RecommendationRequest {
            request_id: Some("dashboard-recommendations".to_string()),
            schema_version: Some(SCHEMA_VERSION.to_string()),
            horizon_months: Some(state.config.projection_months),
            company_limit: Some(20),
            commodity_limit: Some(30),
            scenario: Some("base".to_string()),
            series: Some(series.clone()),
            macro_context: None,
            macro_fiscal_context: Some(default_macro_fiscal_context()),
            venture_capital_context: Some(sample_venture_capital_context()),
            sentiment_context: None,
        },
    )?;
    Ok(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "series": series,
        "forecast": forecast,
        "recommendations": recommendations,
        "macroFiscalContext": default_macro_fiscal_context(),
        "ventureCapitalContext": sample_venture_capital_context(),
        "sources": source_catalog(),
        "equations": equation_catalog(),
        "desEngine": des_surface_descriptor(),
        "atMs": now_ms()
    }))
}

pub(crate) fn default_macro_fiscal_context() -> MacroFiscalContext {
    MacroFiscalContext {
        country: Some("US".to_string()),
        period: Some("built-in-demo-current".to_string()),
        gdp: Some(29_000_000_000_000.0),
        gdp_growth: Some(0.021),
        national_debt: Some(36_000_000_000_000.0),
        debt_to_gdp: Some(1.24),
        deficit: Some(1_800_000_000_000.0),
        deficit_to_gdp: Some(0.062),
        receipts: Some(5_000_000_000_000.0),
        outlays: Some(6_800_000_000_000.0),
        borrowing: Some(1_900_000_000_000.0),
        net_interest_outlays: Some(950_000_000_000.0),
        labor_force_participation: Some(0.626),
        prime_age_participation: Some(0.836),
        unemployment_rate: Some(0.040),
        payroll_growth: Some(0.014),
        wage_growth: Some(0.041),
        productivity_growth: Some(0.015),
    }
}

pub(crate) fn sample_venture_capital_context() -> VentureCapitalContext {
    VentureCapitalContext {
        period: Some("built-in-demo-current".to_string()),
        sector_flows: vec![
            VentureSectorFlow {
                sector: "artificial-intelligence".to_string(),
                deal_count: 640,
                invested_capital: 96_000_000_000.0,
                yoy_growth: 0.42,
                dry_powder: Some(120_000_000_000.0),
                exit_liquidity: Some(0.28),
                confidence: Some(0.70),
            },
            VentureSectorFlow {
                sector: "cybersecurity".to_string(),
                deal_count: 310,
                invested_capital: 24_000_000_000.0,
                yoy_growth: 0.18,
                dry_powder: Some(36_000_000_000.0),
                exit_liquidity: Some(0.34),
                confidence: Some(0.64),
            },
            VentureSectorFlow {
                sector: "climate-energy".to_string(),
                deal_count: 420,
                invested_capital: 38_000_000_000.0,
                yoy_growth: 0.12,
                dry_powder: Some(48_000_000_000.0),
                exit_liquidity: Some(0.22),
                confidence: Some(0.60),
            },
            VentureSectorFlow {
                sector: "biotech-healthcare".to_string(),
                deal_count: 520,
                invested_capital: 44_000_000_000.0,
                yoy_growth: 0.06,
                dry_powder: Some(70_000_000_000.0),
                exit_liquidity: Some(0.30),
                confidence: Some(0.62),
            },
            VentureSectorFlow {
                sector: "fintech".to_string(),
                deal_count: 360,
                invested_capital: 28_000_000_000.0,
                yoy_growth: -0.04,
                dry_powder: Some(54_000_000_000.0),
                exit_liquidity: Some(0.18),
                confidence: Some(0.56),
            },
            VentureSectorFlow {
                sector: "industrial-automation".to_string(),
                deal_count: 250,
                invested_capital: 21_000_000_000.0,
                yoy_growth: 0.16,
                dry_powder: Some(29_000_000_000.0),
                exit_liquidity: Some(0.26),
                confidence: Some(0.58),
            },
        ],
        deals: vec![
            VentureCapitalDealSignal {
                firm: "sample-growth-fund".to_string(),
                company: "Anthropic".to_string(),
                sector: "artificial-intelligence".to_string(),
                stage: "late-private".to_string(),
                amount: 4_000_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.58),
            },
            VentureCapitalDealSignal {
                firm: "sample-infrastructure-fund".to_string(),
                company: "Databricks".to_string(),
                sector: "data-infrastructure".to_string(),
                stage: "late-private".to_string(),
                amount: 1_800_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.56),
            },
            VentureCapitalDealSignal {
                firm: "sample-fintech-fund".to_string(),
                company: "Stripe".to_string(),
                sector: "fintech".to_string(),
                stage: "late-private".to_string(),
                amount: 900_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.52),
            },
            VentureCapitalDealSignal {
                firm: "sample-defense-tech-fund".to_string(),
                company: "Anduril".to_string(),
                sector: "defense-industrials".to_string(),
                stage: "late-private".to_string(),
                amount: 1_500_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.54),
            },
            VentureCapitalDealSignal {
                firm: "sample-energy-transition-fund".to_string(),
                company: "Commonwealth Fusion Systems".to_string(),
                sector: "climate-energy".to_string(),
                stage: "growth".to_string(),
                amount: 850_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.50),
            },
            VentureCapitalDealSignal {
                firm: "sample-biotech-fund".to_string(),
                company: "Generate Biomedicines".to_string(),
                sector: "biotech-healthcare".to_string(),
                stage: "growth".to_string(),
                amount: 600_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.48),
            },
        ],
    }
}

pub(crate) fn macro_indicator_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "macroFiscalContext": default_macro_fiscal_context(),
        "credentialStatus": &config.market_data_credentials,
        "providers": [
            {
                "id": "fred",
                "credentialEnv": ["ECONOMICS_FRED_API_KEY"],
                "signals": ["federal debt", "debt-to-GDP", "rates", "money supply", "labor participation"]
            },
            {
                "id": "bea",
                "credentialEnv": ["ECONOMICS_BEA_API_KEY"],
                "signals": ["GDP", "gross domestic income", "productivity-compatible national accounts"]
            },
            {
                "id": "bls",
                "credentialEnv": ["ECONOMICS_BLS_API_KEY"],
                "signals": ["labor force participation", "unemployment", "payrolls", "wages", "productivity"]
            },
            {
                "id": "treasury-fiscaldata",
                "credentialEnv": ["ECONOMICS_TREASURY_API_KEY"],
                "signals": ["receipts", "outlays", "deficits", "borrowing", "debt outstanding", "interest outlays"]
            },
            {
                "id": "census-eia",
                "credentialEnv": ["ECONOMICS_CENSUS_API_KEY", "ECONOMICS_EIA_API_KEY"],
                "signals": ["trade", "construction", "inventory", "energy supply-demand"]
            }
        ],
        "placeholderMode": "built-in sample context is returned until live provider fetchers are attached"
    })
}

pub(crate) fn vc_investment_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "ventureCapitalContext": sample_venture_capital_context(),
        "credentialStatus": &config.market_data_credentials,
        "providers": [
            {
                "id": "crunchbase",
                "credentialEnv": ["ECONOMICS_CRUNCHBASE_API_KEY"],
                "signals": ["funding rounds", "company sectors", "investor participation", "stage"]
            },
            {
                "id": "pitchbook",
                "credentialEnv": ["ECONOMICS_PITCHBOOK_API_KEY"],
                "signals": ["VC firm investment", "deal terms", "private valuations", "exit/liquidity"]
            },
            {
                "id": "cb-insights",
                "credentialEnv": ["ECONOMICS_CB_INSIGHTS_API_KEY"],
                "signals": ["sector momentum", "private market narratives", "company tracking"]
            },
            {
                "id": "dealroom-preqin",
                "credentialEnv": ["ECONOMICS_DEALROOM_API_KEY", "ECONOMICS_PREQIN_API_KEY"],
                "signals": ["global private-market flows", "dry powder", "fundraising", "late-stage marks"]
            },
            {
                "id": "sec",
                "credentialEnv": ["ECONOMICS_SEC_API_KEY"],
                "signals": ["D filings", "S-1 filings", "insider and issuer disclosures"]
            }
        ],
        "recommendationsRoute": "POST /recommendations",
        "placeholderMode": "built-in sample VC flow context is returned until live provider fetchers are attached"
    })
}

pub(crate) const DASHBOARD_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Economics Dashboard</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #101213;
      --panel: #171a1d;
      --panel-2: #20252a;
      --line: #303840;
      --text: #f1f4f2;
      --muted: #9ba8a2;
      --green: #50c878;
      --blue: #64a6ff;
      --gold: #e3b341;
      --red: #f26d6d;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font: 14px/1.45 Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }
    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      min-height: 64px;
      padding: 0 20px;
      border-bottom: 1px solid var(--line);
      background: #121517;
    }
    h1 { margin: 0; font-size: 18px; font-weight: 700; letter-spacing: 0; }
    .sub { color: var(--muted); font-size: 12px; }
    main {
      display: grid;
      grid-template-columns: 320px minmax(0, 1fr);
      gap: 0;
      min-height: calc(100vh - 64px);
    }
    aside {
      border-right: 1px solid var(--line);
      background: var(--panel);
      padding: 16px;
      overflow: auto;
    }
    section {
      min-width: 0;
      padding: 16px;
      overflow: auto;
    }
    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      margin-bottom: 12px;
    }
    button, select {
      border: 1px solid var(--line);
      background: var(--panel-2);
      color: var(--text);
      height: 34px;
      border-radius: 6px;
      padding: 0 10px;
    }
    button { cursor: pointer; }
    .metric-grid {
      display: grid;
      grid-template-columns: repeat(4, minmax(150px, 1fr));
      gap: 10px;
      margin-bottom: 12px;
    }
    .metric, .chart, .table-wrap, .equations {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
    }
    .metric { padding: 12px; min-height: 78px; }
    .metric strong { display: block; font-size: 20px; }
    .metric span { color: var(--muted); font-size: 12px; }
    .watchlist { display: grid; gap: 8px; }
    .watch {
      width: 100%;
      text-align: left;
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 4px 8px;
      min-height: 56px;
    }
    .watch.active { border-color: var(--blue); }
    .watch small { color: var(--muted); }
    .signal { font-size: 12px; color: var(--green); }
    .signal.risk { color: var(--red); }
    .chart { padding: 12px; margin-bottom: 12px; }
    canvas { width: 100%; height: 360px; display: block; }
    .table-wrap { overflow: auto; }
    table { width: 100%; border-collapse: collapse; min-width: 760px; }
    th, td { padding: 10px 12px; border-bottom: 1px solid var(--line); text-align: left; white-space: nowrap; }
    th { color: var(--muted); font-size: 12px; font-weight: 600; }
    .equations { padding: 12px; margin-top: 12px; }
    .eq-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
    .eq {
      border: 1px solid var(--line);
      border-radius: 6px;
      padding: 10px;
      background: #14181a;
    }
    .eq code { color: var(--gold); white-space: normal; }
    @media (max-width: 900px) {
      main { grid-template-columns: 1fr; }
      aside { border-right: 0; border-bottom: 1px solid var(--line); max-height: 320px; }
      .metric-grid { grid-template-columns: repeat(2, minmax(130px, 1fr)); }
      .eq-list { grid-template-columns: 1fr; }
      canvas { height: 300px; }
    }
  </style>
</head>
<body>
  <header>
    <div>
      <h1>Economics Dashboard</h1>
      <div class="sub">15Y history model | 18M projection | DES-backed theory surface</div>
    </div>
    <div class="sub" id="status">loading</div>
  </header>
  <main>
    <aside>
      <div class="toolbar">
        <strong>Markets</strong>
        <button id="refresh">Refresh</button>
      </div>
      <div class="watchlist" id="watchlist"></div>
    </aside>
    <section>
      <div class="toolbar">
        <div>
          <strong id="selected-title">Projection</strong>
          <div class="sub" id="selected-sub"></div>
        </div>
        <select id="scenario">
          <option value="base">Base</option>
          <option value="soft-landing">Soft landing</option>
          <option value="liquidity-crunch">Liquidity crunch</option>
          <option value="oil-shock">Oil shock</option>
          <option value="dollar-strength">Dollar strength</option>
          <option value="deflation">Deflation</option>
        </select>
      </div>
      <div class="metric-grid" id="metrics"></div>
      <div class="chart"><canvas id="chart" width="1200" height="420"></canvas></div>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Instrument</th>
              <th>Class</th>
              <th>Signal</th>
              <th>Last</th>
              <th>18M Return</th>
              <th>Drift</th>
              <th>Volatility</th>
            </tr>
          </thead>
          <tbody id="projection-rows"></tbody>
        </table>
      </div>
      <div class="equations">
        <strong>Equation Layer</strong>
        <div class="eq-list" id="equations"></div>
      </div>
    </section>
  </main>
  <script>
    const state = { data: null, selected: 0 };
    const colors = ["#64a6ff", "#50c878", "#e3b341", "#f26d6d"];

    function fmtPct(v) { return (v * 100).toFixed(2) + "%"; }
    function fmtNum(v) {
      if (Math.abs(v) >= 1000) return Number(v).toLocaleString(undefined, { maximumFractionDigits: 0 });
      return Number(v).toLocaleString(undefined, { maximumFractionDigits: 3 });
    }

    async function load() {
      const res = await fetch("dashboard.json", { headers: { "accept": "application/json" } });
      if (!res.ok) throw new Error("HTTP " + res.status);
      state.data = await res.json();
      state.selected = Math.min(state.selected, state.data.forecast.projections.length - 1);
      render();
    }

    async function applyScenario(scenario) {
      const res = await fetch("forecast", {
        method: "POST",
        headers: { "accept": "application/json", "content-type": "application/json" },
        body: JSON.stringify({
          schemaVersion: "economics.forecast.v1",
          requestId: "dashboard-" + scenario,
          scenario,
          horizonMonths: state.data.forecast.horizonMonths,
          confidenceLevel: state.data.forecast.confidenceLevel,
          series: state.data.series
        })
      });
      if (!res.ok) throw new Error("HTTP " + res.status);
      state.data.forecast = await res.json();
      state.selected = Math.min(state.selected, state.data.forecast.projections.length - 1);
      render();
    }

    function render() {
      const projections = state.data.forecast.projections;
      const selected = projections[state.selected] || projections[0];
      document.getElementById("status").textContent = new Date().toLocaleTimeString();
      document.getElementById("selected-title").textContent = selected.displayName;
      document.getElementById("selected-sub").textContent = selected.assetClass + " | " + selected.currency;
      renderWatchlist(projections);
      renderMetrics(selected, projections);
      renderTable(projections);
      renderEquations(state.data.equations.slice(0, 6));
      drawChart(selected);
    }

    function renderWatchlist(projections) {
      const list = document.getElementById("watchlist");
      list.innerHTML = "";
      projections.forEach((p, idx) => {
        const btn = document.createElement("button");
        btn.className = "watch" + (idx === state.selected ? " active" : "");
        btn.onclick = () => { state.selected = idx; render(); };
        const risk = p.signal.includes("reduce") ? " risk" : "";
        btn.innerHTML = "<span>" + p.displayName + "<br><small>" + p.instrumentId + "</small></span>" +
          "<span class=\"signal" + risk + "\">" + p.signal + "</span>";
        list.appendChild(btn);
      });
    }

    function renderMetrics(selected, projections) {
      const best = projections.slice().sort((a, b) => b.expectedReturn18m - a.expectedReturn18m)[0];
      const worst = projections.slice().sort((a, b) => a.expectedReturn18m - b.expectedReturn18m)[0];
      const metrics = [
        ["Selected 18M", fmtPct(selected.expectedReturn18m), selected.signal],
        ["Annual Drift", fmtPct(selected.annualizedDrift), "weighted theory/data"],
        ["Annual Vol", fmtPct(selected.annualizedVolatility), "interval width"],
        ["Best/Worst", best.instrumentId + " / " + worst.instrumentId, fmtPct(best.expectedReturn18m) + " / " + fmtPct(worst.expectedReturn18m)]
      ];
      document.getElementById("metrics").innerHTML = metrics.map(m =>
        "<div class=\"metric\"><span>" + m[0] + "</span><strong>" + m[1] + "</strong><span>" + m[2] + "</span></div>"
      ).join("");
    }

    function renderTable(projections) {
      document.getElementById("projection-rows").innerHTML = projections.map((p, idx) =>
        "<tr data-idx=\"" + idx + "\"><td>" + p.displayName + "</td><td>" + p.assetClass + "</td><td>" +
        p.signal + "</td><td>" + fmtNum(p.lastPrice) + "</td><td>" + fmtPct(p.expectedReturn18m) +
        "</td><td>" + fmtPct(p.annualizedDrift) + "</td><td>" + fmtPct(p.annualizedVolatility) + "</td></tr>"
      ).join("");
    }

    function renderEquations(equations) {
      document.getElementById("equations").innerHTML = equations.map(eq =>
        "<div class=\"eq\"><strong>" + eq.name + "</strong><br><code>" + eq.equation +
        "</code><div class=\"sub\">" + eq.family + "</div></div>"
      ).join("");
    }

    function drawChart(p) {
      const canvas = document.getElementById("chart");
      const ctx = canvas.getContext("2d");
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = "#121517";
      ctx.fillRect(0, 0, w, h);
      const pts = p.points;
      const values = pts.flatMap(x => [x.lower, x.expected, x.upper]);
      const min = Math.min(...values) * 0.98;
      const max = Math.max(...values) * 1.02;
      const x = i => 48 + (w - 80) * (i / Math.max(1, pts.length - 1));
      const y = v => h - 36 - (h - 72) * ((v - min) / Math.max(1e-9, max - min));
      ctx.strokeStyle = "#303840";
      ctx.lineWidth = 1;
      for (let i = 0; i < 5; i++) {
        const yy = 24 + i * (h - 72) / 4;
        ctx.beginPath(); ctx.moveTo(40, yy); ctx.lineTo(w - 24, yy); ctx.stroke();
      }
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.upper)]), colors[2], 1);
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.lower)]), colors[3], 1);
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.expected)]), colors[0], 3);
      ctx.fillStyle = "#9ba8a2";
      ctx.font = "14px ui-monospace, Menlo, monospace";
      ctx.fillText(fmtNum(max), 8, 28);
      ctx.fillText(fmtNum(min), 8, h - 22);
      ctx.fillStyle = "#f1f4f2";
      ctx.fillText(p.instrumentId + " expected path", 48, 24);
    }

    function drawLine(ctx, points, color, width) {
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.beginPath();
      points.forEach(([x, y], idx) => idx ? ctx.lineTo(x, y) : ctx.moveTo(x, y));
      ctx.stroke();
    }

    document.getElementById("refresh").onclick = () => load().catch(err => {
      document.getElementById("status").textContent = err.message;
    });
    document.getElementById("scenario").onchange = (event) => {
      applyScenario(event.target.value).catch(err => {
        document.getElementById("status").textContent = err.message;
      });
    };
    load().catch(err => { document.getElementById("status").textContent = err.message; });
  </script>
</body>
</html>
"##;
