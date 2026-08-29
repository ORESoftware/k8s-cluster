use axum::response::Html;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;

use crate::shared::{shared_header, SHARED_HEADER_BOOT_JS};

#[derive(Deserialize)]
pub(crate) struct JelloSampleQuery {
    pub(crate) product: Option<String>,
}

pub(crate) fn jello_document() -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" data-dd-mode="dark" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                    meta name="theme-color" content="#f8fbff";
                    title { "Athlet-O performance gelatin" }
                    script { (PreEscaped(SHARED_HEADER_BOOT_JS)) }
                    style { (PreEscaped(JELLO_CSS)) }
                    link rel="stylesheet" href="/assets/web-home/shared-header.css";
                    script defer="defer" src="https://unpkg.com/htmx.org@2.0.4" {}
                    script defer="defer" src="/assets/web-home/shared-header.js" {}
                }
                body {
                    (shared_header("jello"))
                    (PreEscaped(JELLO_BODY))
                }
            }
        }
        .into_string(),
    )
}

pub(crate) fn jello_sample_markup(product: Option<&str>) -> Markup {
    let (class_name, badge, title, description, chips) = match product {
        Some("recover") => (
            "recover",
            "R-O",
            "Recover-O cooldown box",
            "Berry-orange recovery wobble for the ride home, with minerals, vitamin C, fiber, and live cultures.",
            &["Gelatin protein", "Magnesium", "Potassium", "Probiotics"][..],
        ),
        Some("pregame") => (
            "pregame",
            "P-G",
            "Pre-Game-O tunnel box",
            "Citrus-punch prep cup for pre-game rituals, packed with electrolytes, vitamin C, and no sugar rush.",
            &["Sodium", "Potassium", "Vitamin C", "Zero sugar"][..],
        ),
        _ => (
            "athlet",
            "A-O",
            "Athlet-O starter box",
            "Lime-citrus protein wobble for daily training bags, bus rides, and after-school lift sessions.",
            &[
                "20g gelatin protein",
                "Inulin fiber",
                "Vitamin C",
                "Electrolytes",
            ][..],
        ),
    };

    html! {
        div class=(format!("sample-card {class_name}")) {
            div class="sample-badge" { (badge) }
            div {
                h3 { (title) }
                p { (description) }
                div class="sample-stack" {
                    @for chip in chips {
                        span { (chip) }
                    }
                }
            }
        }
    }
}

const JELLO_CSS: &str = r###"
:root {
  color-scheme: light;
  --ink: #12323a;
  --muted: #516872;
  --paper: #f8fbff;
  --paper-2: #ffffff;
  --line: rgba(18, 50, 58, 0.16);
  --green: #53d86a;
  --green-dark: #168943;
  --aqua: #27c9c3;
  --blue: #355dff;
  --coral: #ff6f61;
  --yellow: #ffd84d;
  --berry: #d9498b;
  --shadow: 0 22px 55px rgba(18, 50, 58, 0.16);
}

* {
  box-sizing: border-box;
}

html {
  scroll-behavior: smooth;
}

body {
  margin: 0;
  min-width: 320px;
  background: var(--paper);
  color: var(--ink);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}

a {
  color: inherit;
}

.jello-page {
  overflow: hidden;
}

.jello-hero {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 0.95fr) minmax(360px, 1.05fr);
  min-height: calc(100vh - var(--dd-site-header-height, 72px));
  gap: 28px;
  align-items: center;
  padding: 56px clamp(22px, 5%, 76px) 42px;
  background: #f8fbff;
}

.jello-hero::before {
  content: "";
  position: absolute;
  inset: auto 0 0 0;
  height: 96px;
  background:
    repeating-linear-gradient(
      90deg,
      rgba(255, 216, 77, 0.52) 0 56px,
      rgba(83, 216, 106, 0.34) 56px 112px,
      rgba(39, 201, 195, 0.32) 112px 168px,
      rgba(255, 111, 97, 0.34) 168px 224px
    );
  opacity: 0.72;
}

.hero-copy,
.hero-stage {
  position: relative;
  z-index: 1;
}

.brand-lockup {
  display: inline-flex;
  align-items: center;
  gap: 12px;
  color: var(--ink);
  text-decoration: none;
}

.brand-mark {
  display: inline-grid;
  width: 64px;
  height: 64px;
  place-items: center;
  border: 3px solid var(--ink);
  border-radius: 18px;
  background: var(--yellow);
  box-shadow: 8px 8px 0 var(--ink);
}

.brand-mark svg {
  width: 48px;
  height: 48px;
}

.brand-name {
  font-weight: 950;
  font-size: 2.2rem;
  line-height: 1;
  letter-spacing: 0;
}

.eyebrow {
  width: fit-content;
  margin: 42px 0 16px;
  padding: 8px 14px;
  border: 2px solid var(--ink);
  border-radius: 999px;
  background: #ffffff;
  color: var(--green-dark);
  font-weight: 900;
  text-transform: uppercase;
  letter-spacing: 0;
}

h1,
h2,
h3,
p {
  margin-top: 0;
}

.jello-hero h1 {
  max-width: 760px;
  margin-bottom: 20px;
  font-size: 4.8rem;
  line-height: 0.94;
  letter-spacing: 0;
}

.lede {
  max-width: 680px;
  margin-bottom: 26px;
  color: var(--muted);
  font-size: 1.28rem;
  line-height: 1.6;
}

.hero-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
}

.hero-actions a,
.retailer-row a {
  display: inline-flex;
  min-height: 44px;
  align-items: center;
  justify-content: center;
  border: 2px solid var(--ink);
  border-radius: 999px;
  color: var(--ink);
  font-weight: 900;
  text-decoration: none;
  box-shadow: 4px 4px 0 var(--ink);
  transition: transform 120ms ease, box-shadow 120ms ease;
}

.hero-actions a {
  padding: 12px 18px;
  background: var(--green);
}

.hero-actions a:nth-child(2) {
  background: #ffffff;
}

.hero-actions a:hover,
.retailer-row a:hover {
  transform: translate(2px, 2px);
  box-shadow: 2px 2px 0 var(--ink);
}

.hero-stage {
  display: grid;
  min-height: 560px;
  align-items: end;
}

.shelf {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 18px;
  align-items: end;
}

.hero-pack {
  display: grid;
  min-height: 440px;
  align-items: end;
}

.hero-pack:nth-child(2) {
  min-height: 510px;
}

.cup-visual {
  width: 100%;
  aspect-ratio: 5 / 6;
  filter: drop-shadow(0 24px 20px rgba(18, 50, 58, 0.2));
}

.jello-section {
  padding: 52px clamp(22px, 5%, 76px);
}

.section-heading {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 22px;
  margin-bottom: 24px;
}

.section-heading h2 {
  max-width: 720px;
  margin-bottom: 0;
  font-size: 2.45rem;
  line-height: 1.05;
  letter-spacing: 0;
}

.section-heading p {
  max-width: 520px;
  margin-bottom: 0;
  color: var(--muted);
  line-height: 1.55;
}

.product-line {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 18px;
}

.product-card {
  display: grid;
  grid-template-rows: auto 1fr;
  min-height: 720px;
  border: 2px solid var(--ink);
  border-radius: 8px;
  background: var(--paper-2);
  box-shadow: 8px 8px 0 var(--ink);
  overflow: hidden;
}

.product-visual {
  display: grid;
  min-height: 270px;
  place-items: center;
  padding: 22px;
  border-bottom: 2px solid var(--ink);
}

.athlet .product-visual {
  background: #e9fff0;
}

.recover .product-visual {
  background: #f2edff;
}

.pregame .product-visual {
  background: #fff3df;
}

.product-copy {
  display: grid;
  grid-template-rows: auto auto auto 1fr auto;
  gap: 14px;
  padding: 22px;
}

.product-kicker {
  margin: 0;
  color: var(--muted);
  font-weight: 900;
  text-transform: uppercase;
  letter-spacing: 0;
}

.product-card h3 {
  margin-bottom: 0;
  font-size: 2rem;
  line-height: 1;
  letter-spacing: 0;
}

.tagline {
  margin-bottom: 0;
  color: var(--muted);
  line-height: 1.55;
}

.benefits {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-content: start;
  padding: 0;
  margin: 0;
  list-style: none;
}

.benefits li {
  display: inline-flex;
  align-items: center;
  min-height: 34px;
  padding: 7px 10px;
  border: 1px solid rgba(18, 50, 58, 0.2);
  border-radius: 999px;
  background: #f8fbff;
  color: var(--ink);
  font-weight: 800;
  line-height: 1.1;
}

.formula-list {
  display: grid;
  gap: 10px;
  padding-left: 18px;
  margin: 0;
  color: var(--muted);
  line-height: 1.5;
}

.formula-list strong {
  color: var(--ink);
}

.retailer-row {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 9px;
}

.retailer-row a {
  padding: 9px 10px;
  background: #ffffff;
  font-size: 0.92rem;
}

.sampler-band {
  display: grid;
  grid-template-columns: minmax(0, 0.85fr) minmax(360px, 1.15fr);
  gap: 24px;
  align-items: center;
  border-top: 2px solid var(--ink);
  border-bottom: 2px solid var(--ink);
  background: #fff7d7;
}

.sampler-copy h2 {
  max-width: 640px;
  margin-bottom: 16px;
  font-size: 2.35rem;
  line-height: 1.05;
  letter-spacing: 0;
}

.sampler-copy p {
  max-width: 620px;
  color: var(--muted);
  line-height: 1.65;
}

.sampler-panel {
  display: grid;
  gap: 14px;
}

.sampler-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}

.sampler-controls button {
  min-height: 42px;
  padding: 9px 14px;
  border: 2px solid var(--ink);
  border-radius: 999px;
  background: #ffffff;
  color: var(--ink);
  font: inherit;
  font-weight: 900;
  cursor: pointer;
  box-shadow: 3px 3px 0 var(--ink);
}

.sampler-controls button:hover {
  transform: translate(1px, 1px);
  box-shadow: 2px 2px 0 var(--ink);
}

.sampler-result {
  min-height: 232px;
}

.sample-card {
  display: grid;
  grid-template-columns: minmax(120px, 0.45fr) minmax(0, 1fr);
  gap: 18px;
  align-items: center;
  min-height: 232px;
  padding: 20px;
  border: 2px solid var(--ink);
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 8px 8px 0 var(--ink);
}

.sample-badge {
  display: grid;
  aspect-ratio: 1;
  place-items: center;
  border: 2px solid var(--ink);
  border-radius: 999px;
  background: var(--sample-color, var(--green));
  color: #ffffff;
  font-weight: 950;
  text-align: center;
  text-transform: uppercase;
}

.sample-card h3 {
  margin-bottom: 8px;
  font-size: 1.8rem;
  line-height: 1;
  letter-spacing: 0;
}

.sample-card p {
  margin-bottom: 12px;
  color: var(--muted);
  line-height: 1.55;
}

.sample-stack {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.sample-stack span {
  padding: 7px 10px;
  border-radius: 999px;
  background: #eef8ff;
  color: var(--ink);
  font-weight: 850;
}

.sample-card.athlet {
  --sample-color: var(--green-dark);
}

.sample-card.recover {
  --sample-color: var(--berry);
}

.sample-card.pregame {
  --sample-color: var(--blue);
}

.formula-band {
  display: grid;
  grid-template-columns: minmax(0, 0.95fr) minmax(0, 1.05fr);
  gap: 22px;
  align-items: stretch;
  background: #12323a;
  color: #ffffff;
}

.formula-band h2 {
  max-width: 620px;
  margin-bottom: 18px;
  font-size: 2.35rem;
  line-height: 1.08;
  letter-spacing: 0;
}

.formula-band p {
  max-width: 620px;
  color: #d9eef2;
  line-height: 1.7;
}

.formula-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.formula-tile {
  min-height: 156px;
  padding: 18px;
  border: 2px solid rgba(255, 255, 255, 0.34);
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.08);
}

.formula-tile b {
  display: block;
  margin-bottom: 8px;
  color: var(--yellow);
  font-size: 1.05rem;
}

.formula-tile span {
  color: #e7f6f7;
  line-height: 1.5;
}

.store-note {
  padding-top: 24px;
  color: var(--muted);
  line-height: 1.55;
}

@media (max-width: 1120px) {
  .jello-hero {
    grid-template-columns: 1fr;
    min-height: auto;
  }

  .hero-stage {
    min-height: auto;
  }

  .shelf {
    max-width: 780px;
  }

  .product-line {
    grid-template-columns: 1fr;
  }

  .product-card {
    min-height: auto;
  }

  .product-copy {
    grid-template-rows: auto;
  }

  .formula-band {
    grid-template-columns: 1fr;
  }

  .sampler-band {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 720px) {
  .jello-hero {
    padding-top: 28px;
    padding-bottom: 18px;
    gap: 12px;
  }

  .brand-mark {
    width: 54px;
    height: 54px;
    border-radius: 15px;
  }

  .brand-name {
    font-size: 1.7rem;
  }

  .eyebrow {
    margin-top: 22px;
    margin-bottom: 12px;
    padding: 7px 12px;
  }

  .jello-hero h1 {
    margin-bottom: 14px;
    font-size: 2.45rem;
  }

  .lede {
    margin-bottom: 16px;
    font-size: 1rem;
    line-height: 1.5;
  }

  .hero-actions a {
    min-height: 40px;
    padding: 10px 14px;
  }

  .shelf {
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
  }

  .hero-pack,
  .hero-pack:nth-child(2) {
    min-height: 148px;
  }

  .cup-visual {
    max-height: 180px;
  }

  .section-heading {
    display: grid;
  }

  .section-heading h2,
  .formula-band h2 {
    font-size: 2rem;
  }

  .product-visual {
    min-height: 230px;
  }

  .retailer-row,
  .formula-grid {
    grid-template-columns: 1fr;
  }

  .sample-card {
    grid-template-columns: 1fr;
  }

  .sample-badge {
    max-width: 168px;
  }
}
"###;

const JELLO_BODY: &str = r###"
<main class="jello-page">
  <section class="jello-hero" aria-labelledby="jello-title">
    <div class="hero-copy">
      <a class="brand-lockup" href="/jello" aria-label="Athlet-O home">
        <span class="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 64 64" role="presentation" focusable="false">
            <path d="M13 42c0-15 8-29 19-29s19 14 19 29c0 9-7 14-19 14S13 51 13 42Z" fill="#53d86a" stroke="#12323a" stroke-width="4"/>
            <path d="M24 36c0-6 3-12 8-12s8 6 8 12-3 9-8 9-8-3-8-9Z" fill="#f8fbff" stroke="#12323a" stroke-width="4"/>
            <path d="M20 15c4 6 20 6 24 0" fill="none" stroke="#12323a" stroke-width="4" stroke-linecap="round"/>
          </svg>
        </span>
        <span class="brand-name">Athlet-O</span>
      </a>
      <p class="eyebrow">Performance gelatin cups</p>
      <h1 id="jello-title">Wobble hard. Recover clean.</h1>
      <p class="lede">A jello-ish sports snack built with gelatin protein, inulin fiber, vitamin C, electrolytes, probiotics, and stevia instead of sugar.</p>
      <div class="hero-actions" aria-label="Athlet-O page links">
        <a href="#products">Shop the lineup</a>
        <a href="#formula">See the formula</a>
      </div>
    </div>
    <div class="hero-stage" aria-label="Athlet-O product lineup">
      <div class="shelf">
        <div class="hero-pack" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="img" aria-label="Athlet-O green protein gelatin cup">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#53d86a" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#ffd84d" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#f8fbff" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="148" text-anchor="middle" font-size="30" font-weight="900" fill="#12323a">Athlet-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#168943">protein</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">20g</text>
          </svg>
        </div>
        <div class="hero-pack" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="img" aria-label="Recover-O berry recovery gelatin cup">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#d9498b" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#27c9c3" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#ffffff" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="148" text-anchor="middle" font-size="29" font-weight="900" fill="#12323a">Recover-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#d9498b">rebuild</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">C + salts</text>
          </svg>
        </div>
        <div class="hero-pack" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="img" aria-label="Pre-Game-O citrus pre-game gelatin cup">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#ff6f61" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#355dff" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#fff3df" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="146" text-anchor="middle" font-size="25" font-weight="900" fill="#12323a">Pre-Game-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#355dff">hydrate</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">zero sugar</text>
          </svg>
        </div>
      </div>
    </div>
  </section>

  <section class="jello-section" id="products" aria-labelledby="products-title">
    <div class="section-heading">
      <h2 id="products-title">Three cups, three locker-room jobs.</h2>
      <p>Gelatin gives each cup its bounce and protein base. Inulin brings the fiber. Stevia keeps the sugar out. The rest is built for sweat, travel, and second halves.</p>
    </div>
    <div class="product-line">
      <article class="product-card athlet">
        <div class="product-visual" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="presentation" focusable="false">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#53d86a" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#ffd84d" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#f8fbff" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="148" text-anchor="middle" font-size="30" font-weight="900" fill="#12323a">Athlet-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#168943">protein</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">20g</text>
          </svg>
        </div>
        <div class="product-copy">
          <p class="product-kicker">Daily training</p>
          <h3>Athlet-O</h3>
          <p class="tagline">The flagship cup: lime-citrus wobble with protein, fiber, vitamin C, electrolytes, and probiotic cultures.</p>
          <ul class="benefits" aria-label="Athlet-O benefits">
            <li>Gelatin protein</li>
            <li>Inulin fiber</li>
            <li>No sugar</li>
            <li>Stevia sweetened</li>
            <li>Vitamin C</li>
            <li>Electrolytes</li>
            <li>Probiotics</li>
          </ul>
          <div class="retailer-row" aria-label="Athlet-O retailer links">
            <a href="https://www.amazon.com/s?k=Athlet-O+protein+jello" target="_blank" rel="noopener noreferrer">Amazon</a>
            <a href="https://www.wholefoodsmarket.com/search?text=Athlet-O" target="_blank" rel="noopener noreferrer">Whole Foods</a>
            <a href="https://www.target.com/s?searchTerm=Athlet-O" target="_blank" rel="noopener noreferrer">Target</a>
            <a href="https://www.walmart.com/search?q=Athlet-O+protein+jello" target="_blank" rel="noopener noreferrer">Walmart</a>
          </div>
        </div>
      </article>

      <article class="product-card recover">
        <div class="product-visual" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="presentation" focusable="false">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#d9498b" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#27c9c3" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#ffffff" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="148" text-anchor="middle" font-size="29" font-weight="900" fill="#12323a">Recover-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#d9498b">rebuild</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">C + salts</text>
          </svg>
        </div>
        <div class="product-copy">
          <p class="product-kicker">Post-workout</p>
          <h3>Recover-O</h3>
          <p class="tagline">Berry-orange cool-down gelatin for the ride home, the ice bath, and the morning-after training log.</p>
          <ul class="benefits" aria-label="Recover-O benefits">
            <li>Gelatin protein</li>
            <li>Added vitamin C</li>
            <li>Magnesium</li>
            <li>Potassium</li>
            <li>Prebiotic fiber</li>
            <li>Live cultures</li>
            <li>Zero sugar</li>
          </ul>
          <div class="retailer-row" aria-label="Recover-O retailer links">
            <a href="https://www.amazon.com/s?k=Recover-O+recovery+jello" target="_blank" rel="noopener noreferrer">Amazon</a>
            <a href="https://www.wholefoodsmarket.com/search?text=Recover-O" target="_blank" rel="noopener noreferrer">Whole Foods</a>
            <a href="https://www.target.com/s?searchTerm=Recover-O" target="_blank" rel="noopener noreferrer">Target</a>
            <a href="https://www.costco.com/CatalogSearch?keyword=Recover-O" target="_blank" rel="noopener noreferrer">Costco</a>
          </div>
        </div>
      </article>

      <article class="product-card pregame">
        <div class="product-visual" aria-hidden="true">
          <svg class="cup-visual" viewBox="0 0 260 312" role="presentation" focusable="false">
            <path d="M43 68h174l-23 224H66L43 68Z" fill="#ff6f61" stroke="#12323a" stroke-width="7"/>
            <path d="M35 48h190v42H35z" fill="#355dff" stroke="#12323a" stroke-width="7"/>
            <path d="M69 112h122v126H69z" fill="#fff3df" stroke="#12323a" stroke-width="6"/>
            <text x="130" y="146" text-anchor="middle" font-size="25" font-weight="900" fill="#12323a">Pre-Game-O</text>
            <text x="130" y="184" text-anchor="middle" font-size="20" font-weight="800" fill="#355dff">hydrate</text>
            <text x="130" y="218" text-anchor="middle" font-size="18" font-weight="800" fill="#12323a">zero sugar</text>
          </svg>
        </div>
        <div class="product-copy">
          <p class="product-kicker">Before the whistle</p>
          <h3>Pre-Game-O</h3>
          <p class="tagline">Citrus-punch gelatin for pre-game rituals: bright vitamin C, easy electrolytes, fiber, and no sugar rush.</p>
          <ul class="benefits" aria-label="Pre-Game-O benefits">
            <li>Sodium</li>
            <li>Potassium</li>
            <li>Vitamin C</li>
            <li>Inulin fiber</li>
            <li>Stevia sweetened</li>
            <li>Light protein</li>
            <li>No sugar</li>
          </ul>
          <div class="retailer-row" aria-label="Pre-Game-O retailer links">
            <a href="https://www.amazon.com/s?k=Pre-Game-O+electrolyte+jello" target="_blank" rel="noopener noreferrer">Amazon</a>
            <a href="https://www.wholefoodsmarket.com/search?text=Pre-Game-O" target="_blank" rel="noopener noreferrer">Whole Foods</a>
            <a href="https://www.target.com/s?searchTerm=Pre-Game-O" target="_blank" rel="noopener noreferrer">Target</a>
            <a href="https://www.gnc.com/search?q=Pre-Game-O" target="_blank" rel="noopener noreferrer">GNC</a>
          </div>
        </div>
      </article>
    </div>
    <p class="store-note">Retail links open retailer searches for this concept lineup.</p>
  </section>

  <section class="jello-section sampler-band" aria-labelledby="sampler-title">
    <div class="sampler-copy">
      <h2 id="sampler-title">Build a snack-table sample pack.</h2>
      <p>Pick the cup for the moment and the flavor brief lands ready for the sideline cooler.</p>
    </div>
    <div class="sampler-panel">
      <div class="sampler-controls" aria-label="Sample pack choices">
        <button type="button" hx-get="/jello/sample?product=athlet" hx-target="#sampler-result" hx-swap="innerHTML">Athlet-O</button>
        <button type="button" hx-get="/jello/sample?product=recover" hx-target="#sampler-result" hx-swap="innerHTML">Recover-O</button>
        <button type="button" hx-get="/jello/sample?product=pregame" hx-target="#sampler-result" hx-swap="innerHTML">Pre-Game-O</button>
      </div>
      <div id="sampler-result" class="sampler-result" hx-get="/jello/sample?product=athlet" hx-trigger="load" hx-swap="innerHTML">
        <div class="sample-card athlet">
          <div class="sample-badge">A-O</div>
          <div>
            <h3>Athlet-O starter box</h3>
            <p>Lime-citrus protein wobble for daily training bags, bus rides, and after-school lift sessions.</p>
            <div class="sample-stack">
              <span>20g gelatin protein</span>
              <span>Inulin fiber</span>
              <span>Vitamin C</span>
              <span>Electrolytes</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>

  <section class="jello-section formula-band" id="formula" aria-labelledby="formula-title">
    <div>
      <h2 id="formula-title">Built like a sports drink moved into a snack cup.</h2>
      <p>Each concept cup starts with a bouncy gelatin base, then stacks athlete-friendly add-ins without the syrupy sugar crash.</p>
    </div>
    <div class="formula-grid">
      <div class="formula-tile"><b>Protein bounce</b><span>Gelatin gives the signature wobble and a compact protein payload.</span></div>
      <div class="formula-tile"><b>Fiber assist</b><span>Inulin brings prebiotic fiber while keeping the texture smooth.</span></div>
      <div class="formula-tile"><b>Hydration salts</b><span>Sodium, potassium, and magnesium help the cup earn its gym-bag spot.</span></div>
      <div class="formula-tile"><b>Bright support</b><span>Vitamin C and probiotic cultures round out the everyday performance stack.</span></div>
    </div>
  </section>
</main>
"###;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jello_sample_markup_renders_each_product() {
        let athlet = jello_sample_markup(Some("athlet")).into_string();
        assert!(athlet.contains("sample-card athlet"));
        assert!(athlet.contains("Athlet-O starter box"));
        assert!(athlet.contains("20g gelatin protein"));

        let recover = jello_sample_markup(Some("recover")).into_string();
        assert!(recover.contains("sample-card recover"));
        assert!(recover.contains("Recover-O cooldown box"));
        assert!(recover.contains("Magnesium"));

        let pregame = jello_sample_markup(Some("pregame")).into_string();
        assert!(pregame.contains("sample-card pregame"));
        assert!(pregame.contains("Pre-Game-O tunnel box"));
        assert!(pregame.contains("Zero sugar"));
    }

    #[test]
    fn jello_sample_markup_falls_back_to_athlet() {
        for product in [None, Some("unknown-product"), Some("")] {
            let markup = jello_sample_markup(product).into_string();
            assert!(markup.contains("Athlet-O starter box"), "fallback for {product:?}");
        }
    }

    #[test]
    fn jello_document_serves_athlet_o_branding() {
        let Html(page) = jello_document();
        assert!(page.contains("<title>Athlet-O performance gelatin</title>"));
        assert!(page.contains("Wobble hard. Recover clean."));
        assert!(page.contains("/jello/sample?product=athlet"));
        assert!(page.contains("Recover-O"));
        assert!(page.contains("Pre-Game-O"));
    }
}
