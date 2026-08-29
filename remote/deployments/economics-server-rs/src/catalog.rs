use std::collections::BTreeSet;

use des_engine::service::{EndpointKind, ServiceBuilder, ServiceInfo};
use serde_json::{json, Value};

use crate::dashboard::*;
use crate::forecast::*;
use crate::pipeline::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn des_surface_descriptor() -> Value {
    let surface = des_engine::sdk::surface();
    json!({
        "crate": surface.crate_name,
        "version": surface.version,
        "modules": surface.modules,
        "path": "remote/submodules/discrete-event-system.rs",
        "usage": "Forecast service embeds the DES SDK surface for acausal equations, MDP/POMDP, optimization, simulation, and service discovery."
    })
}

pub(crate) fn des_service_descriptor() -> Value {
    let mut builder = ServiceBuilder::new(ServiceInfo {
        name: SERVICE_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Economics dashboard and theory/data forecast service backed by des_engine."
            .to_string(),
    });
    builder
        .endpoint("GET", "/", "Dashboard shell.", EndpointKind::Service)
        .endpoint(
            "GET",
            "/dashboard.json",
            "Dashboard data and projections.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/forecast",
            "Run an economics forecast.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/ingest",
            "Ingest normalized market history.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/sources/public",
            "Known public data source templates with parsers and documentation links.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/sources/pull",
            "Fetch sourceId templates or bounded custom market history from an approved API URL.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/sentiment/sources",
            "Social/news sentiment provider catalog and credential status.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/sentiment/analyze",
            "Analyze supplied social/news text snippets for market sentiment.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/macro/indicators",
            "Fiscal, GDP, debt, spending, borrowing, and labor indicator context.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/vc/investment",
            "Venture-capital firm, deal, sector-flow, and credential placeholder context.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/recommendations",
            "Rank top company and commodity buy/sell-or-dump candidates.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/audit/hardening",
            "Runtime hardening posture, bounds, and residual-risk audit.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/pipelines/catalog",
            "Spark, Airflow, Databricks, data lake, and NATS pipeline integration catalog.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/observability",
            "Prometheus, Loki, Grafana, and explicit-only OTel telemetry posture.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/integrations/health",
            "Redacted readiness and degradation status for economics integrations.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/pipelines/plan",
            "Create redacted big-data pipeline job intents for economics refresh work.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/pipelines/submit",
            "Submit eligible job intents to the internal Spark pipeline server when enabled.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/model/equations",
            "Equation and theory catalogue.",
            EndpointKind::Service,
        );
    serde_json::to_value(builder.build()).unwrap_or_else(|_| json!({}))
}

pub(crate) fn equation_catalog() -> Vec<EquationDescriptor> {
    vec![
        EquationDescriptor {
            name: "Geometric Brownian Motion",
            family: "stochastic-asset-pricing",
            equation: "dS/S = mu dt + sigma dW",
            use_case: "Baseline traded-asset projection and log-normal confidence intervals.",
            caveat: "Useful for liquid prices, but fat tails and regime changes require stress overlays.",
        },
        EquationDescriptor {
            name: "Ornstein-Uhlenbeck Mean Reversion",
            family: "stochastic-rates-spreads",
            equation: "dX = theta(m - X)dt + sigma dW",
            use_case: "Rates, spreads, valuation gaps, and commodity inventory/carry deviations.",
            caveat: "Mean level and speed are inferred from data and should be re-estimated by regime.",
        },
        EquationDescriptor {
            name: "Hotelling Rule With Carry",
            family: "commodity-economics",
            equation: "E[dP/P] ~= r + storage_cost - convenience_yield + demand_growth - supply_growth",
            use_case: "Oil, metals, and other storable commodities with inventory and convenience yield.",
            caveat: "Short-run supply shocks can dominate the smooth scarcity path.",
        },
        EquationDescriptor {
            name: "CAPM Expected Return",
            family: "asset-pricing",
            equation: "E[R_i] = R_f + beta_i(E[R_m] - R_f)",
            use_case: "Equity and risk-asset prior when market return and beta are known.",
            caveat: "Single-factor CAPM is a prior, not a complete trading model.",
        },
        EquationDescriptor {
            name: "Fisher Equation",
            family: "rates-inflation",
            equation: "i ~= r + pi_e",
            use_case: "Separates nominal rates into real-rate and expected-inflation components.",
            caveat: "Risk premia and term premia make observed yields richer than the identity.",
        },
        EquationDescriptor {
            name: "Taylor Rule",
            family: "monetary-policy",
            equation: "i = r* + pi + 0.5(pi - pi*) + 0.5 y_gap",
            use_case: "Measures policy tightness versus inflation and output-gap conditions.",
            caveat: "Central banks react to financial stability and politics outside this simple rule.",
        },
        EquationDescriptor {
            name: "Quantity Theory Growth Form",
            family: "monetary-macro",
            equation: "money_growth + velocity_growth ~= inflation + real_growth",
            use_case: "Liquidity impulse for crypto, gold, equities, and broad nominal assets.",
            caveat: "Velocity is unstable, especially around crises and payment-regime shifts.",
        },
        EquationDescriptor {
            name: "Phillips Curve",
            family: "labor-inflation",
            equation: "pi = pi_e - alpha unemployment_gap + supply_shock",
            use_case: "Inflation pressure from labor slack and supply shocks.",
            caveat: "Slope changes over time; use as a weak prior.",
        },
        EquationDescriptor {
            name: "Uncovered Interest Parity",
            family: "foreign-exchange",
            equation: "E[Delta s] ~= i_domestic - i_foreign",
            use_case: "FX drift prior from interest-rate differentials.",
            caveat: "Carry premia and funding stress often violate UIP in tradeable horizons.",
        },
        EquationDescriptor {
            name: "Purchasing Power Parity",
            family: "foreign-exchange",
            equation: "Delta s ~= inflation_domestic - inflation_foreign",
            use_case: "Long-run currency valuation anchor.",
            caveat: "Slow-moving; tariffs, terms of trade, and capital controls matter.",
        },
        EquationDescriptor {
            name: "Expectations Hypothesis Of The Term Structure",
            family: "bonds",
            equation: "long_yield ~= average expected short_rates + term_premium",
            use_case: "Bond price sensitivity to expected policy-rate paths.",
            caveat: "Term premium is time-varying and can dominate forecast errors.",
        },
        EquationDescriptor {
            name: "Supply-Demand Elasticity",
            family: "micro-commodity",
            equation: "Delta P/P ~= (Delta D/D - Delta S/S) / (epsilon_s + abs(epsilon_d))",
            use_case: "Commodity and housing pressure from demand/supply imbalance.",
            caveat: "Elasticities differ sharply by market and horizon.",
        },
        EquationDescriptor {
            name: "Solow Growth Transition",
            family: "macro-growth",
            equation: "dk/dt = s f(k) - (delta + n + g)k",
            use_case: "Slow-moving real growth anchor for macro scenarios.",
            caveat: "Not a short-term trading equation; it anchors regime assumptions.",
        },
        EquationDescriptor {
            name: "Logistic Adoption Diffusion",
            family: "market-discovery",
            equation: "dA/dt = r A(1 - A/K)",
            use_case: "Adoption curves for crypto networks, new commodities, and emerging markets.",
            caveat: "Carrying capacity K is the fragile assumption.",
        },
    ]
}

pub(crate) fn source_catalog() -> Vec<SourceDescriptor> {
    vec![
        SourceDescriptor {
            id: "fred",
            name: "Federal Reserve Economic Data",
            asset_classes: &["rates", "macro", "housing", "money-market", "commodities"],
            auth: "optional API key",
            notes: "Policy rates, CPI/PCE, yield curves, M2, housing, commodity benchmarks.",
        },
        SourceDescriptor {
            id: "treasury",
            name: "US Treasury FiscalData and yield feeds",
            asset_classes: &["bonds", "rates", "money-market"],
            auth: "public",
            notes: "Treasury yield curve, bills, notes, auction and debt datasets.",
        },
        SourceDescriptor {
            id: "bls-bea-census",
            name: "BLS, BEA, Census",
            asset_classes: &["macro", "labor", "real-estate", "trade"],
            auth: "public or optional key",
            notes: "Employment, CPI/PPI, GDP, income, construction, trade, and housing series.",
        },
        SourceDescriptor {
            id: "fiscal-labor",
            name: "Treasury FiscalData, BEA, BLS, CBO, OECD fiscal/labor feeds",
            asset_classes: &["macro", "fiscal", "labor", "debt", "spending", "gdp"],
            auth: "public or optional ECONOMICS_FRED_API_KEY / BEA / BLS placeholders",
            notes: "National borrowing, outlays, receipts, deficits, debt-to-GDP, GDP growth, labor participation, payrolls, wages, and productivity.",
        },
        SourceDescriptor {
            id: "vc-private-markets",
            name: "Crunchbase, PitchBook, CB Insights, Dealroom, Preqin, SEC filings",
            asset_classes: &["venture-capital", "private-markets", "equities", "securities"],
            auth: "ECONOMICS_CRUNCHBASE_API_KEY, ECONOMICS_PITCHBOOK_API_KEY, ECONOMICS_CB_INSIGHTS_API_KEY, ECONOMICS_DEALROOM_API_KEY, ECONOMICS_PREQIN_API_KEY",
            notes: "VC firm investment, deal velocity, sector flow, dry powder, late-stage marks, exit liquidity, and private-to-public market read-throughs.",
        },
        SourceDescriptor {
            id: "eia-opec",
            name: "EIA, OPEC, IEA-style energy feeds",
            asset_classes: &["oil", "energy", "commodities"],
            auth: "public or private key",
            notes: "Crude, refined products, storage, production, consumption, and flows.",
        },
        SourceDescriptor {
            id: "metals",
            name: "LBMA, CME, exchange and vendor metals feeds",
            asset_classes: &["gold", "silver", "metals", "commodities"],
            auth: "public/private",
            notes: "Spot/futures curves, vault/inventory data, lease/carry proxies.",
        },
        SourceDescriptor {
            id: "crypto",
            name: "CoinGecko, Coinbase, Kraken, Binance US",
            asset_classes: &["crypto", "fx"],
            auth: "public 365-day CoinGecko window or ECONOMICS_COINGECKO_API_KEY/private exchange keys",
            notes: "Spot, order-book, volume, market-cap, funding, and exchange metadata.",
        },
        SourceDescriptor {
            id: "x-twitter",
            name: "X / Twitter API",
            asset_classes: &[
                "sentiment",
                "equities",
                "crypto",
                "commodities",
                "forex",
                "macro",
            ],
            auth: "ECONOMICS_X_BEARER_TOKEN or OAuth 1.0a key/secret placeholders",
            notes: "Market chatter, breaking-news velocity, cashtag/hashtag momentum, influencer and source clustering.",
        },
        SourceDescriptor {
            id: "reddit",
            name: "Reddit API",
            asset_classes: &[
                "sentiment",
                "equities",
                "crypto",
                "commodities",
                "real-estate",
                "macro",
            ],
            auth: "ECONOMICS_REDDIT_CLIENT_ID, ECONOMICS_REDDIT_CLIENT_SECRET, ECONOMICS_REDDIT_USER_AGENT",
            notes: "Subreddit discussion, retail crowd attention, ticker mentions, local real-estate chatter, and topic shifts.",
        },
        SourceDescriptor {
            id: "news-social",
            name: "NewsAPI, RSS, GDELT, Stocktwits, forums",
            asset_classes: &[
                "sentiment",
                "news",
                "equities",
                "crypto",
                "commodities",
                "forex",
                "macro",
            ],
            auth: "ECONOMICS_NEWS_API_KEY, ECONOMICS_GDELT_API_KEY, ECONOMICS_STOCKTWITS_TOKEN",
            notes: "Public/private news and social streams for narrative, event, and entity-level sentiment features.",
        },
        SourceDescriptor {
            id: "equities",
            name: "Polygon, Alpaca, IEX, Nasdaq Data Link, Stooq",
            asset_classes: &["equities", "securities", "etf", "indices"],
            auth: "public/private",
            notes: "OHLCV, corporate actions, indices, sectors, and securities metadata.",
        },
        SourceDescriptor {
            id: "forex",
            name: "ECB, BIS, broker FX APIs",
            asset_classes: &["forex", "currency", "rates"],
            auth: "public/private",
            notes: "Exchange rates, effective exchange rates, forwards, and carry data.",
        },
        SourceDescriptor {
            id: "global-macro",
            name: "World Bank, IMF, OECD, WTO",
            asset_classes: &["macro", "trade", "currency", "country-risk"],
            auth: "public/private",
            notes:
                "Country macro, trade, debt, inflation, productivity, and balance-of-payments data.",
        },
        SourceDescriptor {
            id: "real-estate",
            name: "FHFA, Case-Shiller, Census, private property feeds",
            asset_classes: &["real-estate", "housing", "credit"],
            auth: "public/private",
            notes:
                "Prices, rents, permits, starts, inventory, mortgage rates, and regional supply.",
        },
    ]
}

pub(crate) fn public_source_templates() -> Vec<PublicSourceTemplate> {
    vec![
        PublicSourceTemplate {
            id: "treasury-debt-to-penny",
            provider: "US Treasury FiscalData",
            name: "US total public debt outstanding",
            asset_class: "debt",
            instrument_id: "US-PUBLIC-DEBT",
            display_name: "US Total Public Debt Outstanding",
            currency: "USD",
            source: "treasury-fiscaldata",
            url: "https://api.fiscaldata.treasury.gov/services/api/fiscal_service/v2/accounting/od/debt_to_penny?fields=record_date,tot_pub_debt_out_amt&filter=record_date:gte:2011-01-01&sort=record_date&page%5Bsize%5D=8000",
            host: "api.fiscaldata.treasury.gov",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/data"),
            date_field: Some("record_date"),
            price_field: Some("tot_pub_debt_out_amt"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fiscaldata.treasury.gov/datasets/debt-to-the-penny/",
            notes: "Official Treasury borrowing series for national-debt context.",
        },
        PublicSourceTemplate {
            id: "worldbank-us-gdp-current-usd",
            provider: "World Bank Indicators API",
            name: "US GDP current USD",
            asset_class: "macro",
            instrument_id: "US-GDP-CURRENT-USD",
            display_name: "US GDP Current USD",
            currency: "USD",
            source: "worldbank",
            url: "https://api.worldbank.org/v2/country/US/indicator/NY.GDP.MKTP.CD?format=json&per_page=70",
            host: "api.worldbank.org",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/1"),
            date_field: Some("date"),
            price_field: Some("value"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "annual",
            documentation_url: "https://datahelpdesk.worldbank.org/knowledgebase/articles/889392-about-the-indicators-api-documentation",
            notes: "Public GDP anchor for macro/fiscal projections.",
        },
        PublicSourceTemplate {
            id: "worldbank-us-labor-participation",
            provider: "World Bank Indicators API",
            name: "US labor force participation rate",
            asset_class: "labor",
            instrument_id: "US-LABOR-PARTICIPATION",
            display_name: "US Labor Force Participation Rate",
            currency: "PCT",
            source: "worldbank",
            url: "https://api.worldbank.org/v2/country/US/indicator/SL.TLF.CACT.ZS?format=json&per_page=70",
            host: "api.worldbank.org",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/1"),
            date_field: Some("date"),
            price_field: Some("value"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "annual",
            documentation_url: "https://datahelpdesk.worldbank.org/knowledgebase/articles/889392-about-the-indicators-api-documentation",
            notes: "Public workforce participation proxy for labor pressure.",
        },
        PublicSourceTemplate {
            id: "coingecko-bitcoin-usd",
            provider: "CoinGecko API",
            name: "Bitcoin market chart USD",
            asset_class: "crypto",
            instrument_id: "BTC-USD",
            display_name: "Bitcoin USD",
            currency: "USD",
            source: "coingecko",
            url: "https://api.coingecko.com/api/v3/coins/bitcoin/market_chart?vs_currency=usd&days=365&interval=daily",
            host: "api.coingecko.com",
            parser: SourceParser::JsonTupleArray,
            root_pointer: Some("/prices"),
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: Some(0),
            price_index: Some(1),
            volume_index: None,
            cadence: "daily",
            documentation_url: "https://docs.coingecko.com/reference/endpoint-overview",
            notes: "Public unauthenticated crypto history is provider-limited to the past 365 days; longer windows require a provider key or private market-data feed.",
        },
        PublicSourceTemplate {
            id: "coingecko-ethereum-usd",
            provider: "CoinGecko API",
            name: "Ethereum market chart USD",
            asset_class: "crypto",
            instrument_id: "ETH-USD",
            display_name: "Ethereum USD",
            currency: "USD",
            source: "coingecko",
            url: "https://api.coingecko.com/api/v3/coins/ethereum/market_chart?vs_currency=usd&days=365&interval=daily",
            host: "api.coingecko.com",
            parser: SourceParser::JsonTupleArray,
            root_pointer: Some("/prices"),
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: Some(0),
            price_index: Some(1),
            volume_index: None,
            cadence: "daily",
            documentation_url: "https://docs.coingecko.com/reference/endpoint-overview",
            notes: "Public unauthenticated crypto history is provider-limited to the past 365 days; longer windows require a provider key or private market-data feed.",
        },
        PublicSourceTemplate {
            id: "fred-dgs10",
            provider: "Federal Reserve Economic Data",
            name: "10-year Treasury constant maturity rate",
            asset_class: "rates",
            instrument_id: "DGS10",
            display_name: "10-Year Treasury Constant Maturity Rate",
            currency: "PCT",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DGS10&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DGS10"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DGS10",
            notes: "Public rate anchor for duration, discount-rate, and dollar-pressure features.",
        },
        PublicSourceTemplate {
            id: "fred-wti-oil",
            provider: "Federal Reserve Economic Data",
            name: "WTI crude oil spot price",
            asset_class: "oil",
            instrument_id: "DCOILWTICO",
            display_name: "WTI Crude Oil Spot Price",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DCOILWTICO&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DCOILWTICO"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DCOILWTICO",
            notes: "Public oil benchmark for commodity and inflation scenarios.",
        },
        PublicSourceTemplate {
            id: "fred-gold",
            provider: "Federal Reserve Economic Data",
            name: "Gold fixing price USD",
            asset_class: "gold",
            instrument_id: "GOLDAMGBD228NLBM",
            display_name: "Gold Fixing Price USD",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=GOLDAMGBD228NLBM&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("GOLDAMGBD228NLBM"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/GOLDAMGBD228NLBM",
            notes: "Public precious-metals benchmark for real-rate and safe-haven modeling.",
        },
        PublicSourceTemplate {
            id: "fred-silver",
            provider: "Federal Reserve Economic Data",
            name: "Silver price USD",
            asset_class: "silver",
            instrument_id: "SLVPRUSD",
            display_name: "Silver Price USD",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=SLVPRUSD&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("SLVPRUSD"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/SLVPRUSD",
            notes: "Public silver benchmark for industrial and precious-metals modeling.",
        },
        PublicSourceTemplate {
            id: "fred-sp500",
            provider: "Federal Reserve Economic Data",
            name: "S&P 500 index",
            asset_class: "equities",
            instrument_id: "SP500",
            display_name: "S&P 500 Index",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("SP500"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/SP500",
            notes: "Public equity benchmark for broad market momentum and risk-premium context.",
        },
        PublicSourceTemplate {
            id: "fred-mortgage30",
            provider: "Federal Reserve Economic Data",
            name: "30-year fixed mortgage average",
            asset_class: "real-estate",
            instrument_id: "MORTGAGE30US",
            display_name: "30-Year Fixed Rate Mortgage Average",
            currency: "PCT",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=MORTGAGE30US&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("MORTGAGE30US"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "weekly",
            documentation_url: "https://fred.stlouisfed.org/series/MORTGAGE30US",
            notes: "Public real-estate financing pressure series.",
        },
        PublicSourceTemplate {
            id: "fred-usd-eur",
            provider: "Federal Reserve Economic Data",
            name: "US dollar to euro exchange rate",
            asset_class: "forex",
            instrument_id: "DEXUSEU",
            display_name: "US Dollar to Euro Exchange Rate",
            currency: "USD/EUR",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DEXUSEU&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DEXUSEU"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DEXUSEU",
            notes: "Public FX benchmark for dollar-strength and carry scenarios.",
        },
    ]
}

pub(crate) fn public_source_template(id: &str) -> Option<PublicSourceTemplate> {
    public_source_templates()
        .into_iter()
        .find(|template| template.id == id)
}

pub(crate) fn public_source_ids() -> Vec<&'static str> {
    public_source_templates()
        .into_iter()
        .map(|template| template.id)
        .collect()
}

pub(crate) fn public_source_hosts() -> Vec<&'static str> {
    let mut hosts = public_source_templates()
        .into_iter()
        .map(|template| template.host)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    hosts.sort_unstable();
    hosts
}

pub(crate) fn public_source_catalog_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "sources": public_source_templates(),
        "pullRoute": "POST /sources/pull",
        "usage": {
            "sourceId": "Pass one of these ids to POST /sources/pull with no url to fetch and parse a known public source.",
            "adHoc": "Pass url plus instrumentId, assetClass, parser, and field/index metadata for authenticated custom API pulls."
        },
        "egressPolicy": {
            "privateUrlsAllowed": config.allow_private_source_urls,
            "allowedSourceHosts": config.allowed_source_hosts,
            "knownPublicHosts": public_source_hosts(),
            "redirectFollowing": false
        },
        "atMs": now_ms()
    })
}

pub(crate) fn observability_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "prometheus": {
            "metricsRoute": "GET /metrics",
            "contentType": "text/plain; version=0.0.4",
            "scrapePort": 8114,
            "lowCardinalityMetrics": true,
            "counters": [
                "dd_economics_server_http_requests_total",
                "dd_economics_server_forecasts_total",
                "dd_economics_server_ingest_requests_total",
                "dd_economics_server_source_pull_total",
                "dd_economics_server_source_pull_success_total",
                "dd_economics_server_source_pull_failure_total",
                "dd_economics_server_source_pull_bytes_total",
                "dd_economics_server_source_pull_stored_points_total",
                "dd_economics_server_sentiment_requests_total",
                "dd_economics_server_recommendation_requests_total",
                "dd_economics_server_pipeline_plan_requests_total",
                "dd_economics_server_pipeline_submit_requests_total",
                "dd_economics_server_pipeline_publish_attempts_total",
                "dd_economics_server_pipeline_publish_success_total",
                "dd_economics_server_pipeline_publish_failure_total",
                "dd_economics_server_pipeline_submit_success_total",
                "dd_economics_server_pipeline_submit_failure_total",
                "dd_economics_server_integration_health_requests_total",
                "dd_economics_server_observability_requests_total",
                "dd_economics_server_auth_failures_total",
                "dd_economics_server_errors_total",
                "dd_economics_server_nats_messages_total",
                "dd_economics_server_nats_published_total"
            ],
            "gauges": [
                "dd_economics_server_source_pull_last_success_unix_seconds"
            ]
        },
        "loki": {
            "collectionBoundary": "container stdout/stderr through Promtail",
            "structuredLogSchema": "dd.log.v1",
            "eventNames": [
                "economics.server.start",
                "economics.auth.failure",
                "economics.source_pull.ok",
                "economics.source_pull.error",
                "economics.nats.loop.disabled",
                "economics.nats.loop.start",
                "economics.nats.subscribe.error",
                "economics.nats.request.oversize",
                "economics.nats.forecast.error",
                "economics.nats.request.invalid",
                "economics.pipeline.plan.encode.error",
                "economics.pipeline.plan.publish.skipped",
                "economics.pipeline.plan.publish.ok",
                "economics.pipeline.plan.publish.error",
                "economics.pipeline.submit.ok",
                "economics.pipeline.submit.rejected",
                "economics.pipeline.submit.error"
            ],
            "labelGuidance": "Promtail should promote only low-cardinality fields such as schema, severity_text, service, namespace, and app labels."
        },
        "otel": {
            "mode": "explicit-only",
            "autoInstrumentation": false,
            "runtimeMonkeyPatching": false,
            "serviceName": env_value("OTEL_SERVICE_NAME", SERVICE_NAME),
            "serviceNamespace": env_value("OTEL_SERVICE_NAMESPACE", "remote-dev"),
            "resourceAttributesConfigured": optional_env("OTEL_RESOURCE_ATTRIBUTES").is_some(),
            "otlpEndpointConfigured": optional_env("OTEL_EXPORTER_OTLP_ENDPOINT").is_some(),
            "collector": "dd-otel-collector handles explicit OTLP and Prometheus scrape pipelines; this service exposes Prometheus metrics and dd.log.v1 logs without auto-instrumentation."
        },
        "grafana": {
            "dashboardUid": env_value("ECONOMICS_GRAFANA_DASHBOARD_UID", "dd-economics-server"),
            "suggestedPanels": [
                "request, error, and auth-failure rates",
                "source pull success/failure, bytes, stored points, and last success timestamp",
                "forecast/recommendation/pipeline plan/publish/submit rates",
                "integration health request rate and degraded dependency count",
                "Loki dd.log.v1 warning/error stream filtered by resource_service_name",
                "pod readiness/restarts from k8s resource exporter"
            ]
        },
        "runtime": {
            "natsConfigured": state.nats.is_some(),
            "publicSourceTemplateCount": public_source_templates().len(),
            "knownPublicSourceHosts": public_source_hosts(),
            "sourcePullAllowedHosts": state.config.allowed_source_hosts,
            "sourceAuthHeaderEnvAllowlistCount": state.config.allowed_source_auth_envs.len(),
            "integrationHealthRoute": "GET /integrations/health",
            "storedSeries": state.series_store.read().map(|store| store.len()).unwrap_or(0)
        },
        "atMs": now_ms()
    })
}

pub(crate) fn sentiment_source_catalog(credentials: &SentimentCredentialStatus) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "credentialStatus": credentials,
        "providers": [
            {
                "id": "x-twitter",
                "name": "X / Twitter",
                "credentialEnv": [
                    "ECONOMICS_X_BEARER_TOKEN",
                    "ECONOMICS_X_API_KEY",
                    "ECONOMICS_X_API_SECRET",
                    "ECONOMICS_X_ACCESS_TOKEN",
                    "ECONOMICS_X_ACCESS_TOKEN_SECRET"
                ],
                "configured": credentials.x_bearer_token || (
                    credentials.x_api_key
                        && credentials.x_api_secret
                        && credentials.x_access_token
                        && credentials.x_access_token_secret
                ),
                "signals": ["cashtags", "hashtags", "source velocity", "topic drift", "breaking-news attention"]
            },
            {
                "id": "reddit",
                "name": "Reddit",
                "credentialEnv": [
                    "ECONOMICS_REDDIT_CLIENT_ID",
                    "ECONOMICS_REDDIT_CLIENT_SECRET",
                    "ECONOMICS_REDDIT_USER_AGENT"
                ],
                "configured": credentials.reddit_client_id
                    && credentials.reddit_client_secret
                    && credentials.reddit_user_agent,
                "signals": ["subreddit momentum", "ticker mentions", "retail crowd sentiment", "regional chatter"]
            },
            {
                "id": "newsapi",
                "name": "News API / private news feed",
                "credentialEnv": ["ECONOMICS_NEWS_API_KEY"],
                "configured": credentials.news_api_key,
                "signals": ["entity news tone", "event velocity", "headline surprise"]
            },
            {
                "id": "stocktwits",
                "name": "Stocktwits",
                "credentialEnv": ["ECONOMICS_STOCKTWITS_TOKEN"],
                "configured": credentials.stocktwits_token,
                "signals": ["cashtag stream sentiment", "watcher momentum", "retail alerting"]
            },
            {
                "id": "gdelt",
                "name": "GDELT / open web events",
                "credentialEnv": ["ECONOMICS_GDELT_API_KEY"],
                "configured": credentials.gdelt_api_key,
                "signals": ["global media tone", "country/event intensity", "trade and conflict narratives"]
            }
        ],
        "analyzeRoute": "POST /sentiment/analyze",
        "placeholderMode": "live provider fetchers are not implemented yet; POST supplied documents for bounded keyword sentiment scoring"
    })
}

pub(crate) fn schema_descriptor() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "defaults": {
            "historyYears": DEFAULT_HISTORY_YEARS,
            "projectionMonths": DEFAULT_PROJECTION_MONTHS,
            "confidenceLevel": 0.90
        },
        "request": {
            "series": "Optional array of instrument time series. If omitted, the service uses ingested in-memory series or the built-in demonstration basket.",
            "macroContext": "Optional policy, inflation, growth, liquidity, and rate context.",
            "macroFiscalContext": "Optional country fiscal/labor context: GDP, borrowing, spending, debt, deficits, interest outlays, workforce participation, payrolls, wages, and productivity.",
            "ventureCapitalContext": "Optional private-market context with VC firm deal signals and sector flows.",
            "theoryWeights": "Optional blend weights for data, macro theory, momentum, mean reversion, carry, valuation, and jump stress.",
            "scenario": "base, oil-shock, liquidity-crunch, dollar-strength, deflation, soft-landing, or custom label."
        },
        "response": {
            "projections": "Per-instrument forecasts with monthly expected/lower/upper values.",
            "components": "Model contribution ledger showing data and equation priors.",
            "equations": "Transparent list of the accepted equation families used as priors."
        },
        "sentiment": {
            "sources": "GET /sentiment/sources reports placeholder credential env names and configured status for X/Twitter, Reddit, news, Stocktwits, and GDELT.",
            "analyze": "POST /sentiment/analyze accepts supplied social/news snippets and returns bounded placeholder sentiment scores by source."
        },
        "sources": {
            "catalog": "GET /sources reports broad provider families.",
            "publicTemplates": "GET /sources/public reports sourceId templates for known public APIs and CSV feeds.",
            "pull": "POST /sources/pull accepts authenticated sourceId pulls or bounded ad-hoc API pulls."
        },
        "macro": {
            "indicators": "GET /macro/indicators reports built-in fiscal/labor sample context and public/private credential placeholders."
        },
        "recommendations": {
            "route": "POST /recommendations",
            "companies": "Returns top 20 invest candidates and top 20 dump/hedge candidates from the model universe.",
            "commodities": "Returns top 30 buy candidates and top 30 sell-or-dump candidates from major tradable commodities."
        },
        "pipelines": {
            "catalog": "GET /pipelines/catalog reports Spark, Airflow, Databricks, data lake, and NATS integration status without returning secrets.",
            "integrations": "GET /integrations/health reports redacted ready/degraded/disabled status for auth, egress, source credentials, Spark, Airflow, Databricks, NATS, runtime-config, data lake, and DES dependencies.",
            "plan": "POST /pipelines/plan creates redacted job intents for Spark pipeline server, Spark feature builds, Airflow DAG triggers, Databricks run-now payloads, and NATS public-data pipeline events.",
            "submit": "POST /pipelines/submit submits only spark-pipeline-server intents and only when ECONOMICS_ENABLE_PIPELINE_SUBMIT=true.",
            "audit": "GET /audit/hardening reports auth, request bounds, SSRF controls, secret handling, and residual risks."
        },
        "observability": {
            "route": "GET /observability",
            "prometheus": "GET /metrics exposes low-cardinality counters and gauges.",
            "loki": "stdout/stderr emits compact dd.log.v1 JSON records for Promtail/Loki.",
            "otel": "explicit-only posture; no auto-instrumentation or runtime monkey-patching."
        }
    })
}

pub(crate) fn example_request() -> ForecastRequest {
    ForecastRequest {
        request_id: Some("example-economics-forecast".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        horizon_months: Some(DEFAULT_PROJECTION_MONTHS),
        confidence_level: Some(0.90),
        scenario: Some("base".to_string()),
        series: Some(sample_market_series()),
        macro_context: Some(MacroContext {
            policy_rate: Some(0.045),
            foreign_policy_rate: Some(0.025),
            inflation: Some(0.031),
            foreign_inflation: Some(0.021),
            expected_inflation: Some(0.026),
            money_supply_growth: Some(0.045),
            real_growth: Some(0.020),
            output_gap: Some(0.004),
            unemployment_gap: Some(-0.003),
            risk_free_rate: Some(0.040),
            market_return: Some(0.082),
        }),
        macro_fiscal_context: Some(default_macro_fiscal_context()),
        venture_capital_context: Some(sample_venture_capital_context()),
        theory_weights: None,
    }
}

pub(crate) fn service_descriptor(state: &AppState) -> Value {
    let stored_series = state
        .series_store
        .read()
        .map(|store| store.len())
        .unwrap_or(0);
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "defaults": {
            "historyYears": state.config.history_years,
            "projectionMonths": state.config.projection_months,
            "confidenceLevel": state.config.confidence_level
        },
        "authRequired": !state.config.allow_unauthenticated,
        "storedSeries": stored_series,
        "endpoints": {
            "dashboard": "GET /",
            "dashboardJson": "GET /dashboard.json",
            "forecast": "POST /forecast",
            "ingest": "POST /ingest",
            "sources": "GET /sources",
            "publicSources": "GET /sources/public",
            "pullSource": "POST /sources/pull",
            "sentimentSources": "GET /sentiment/sources",
            "sentimentAnalyze": "POST /sentiment/analyze",
            "macroIndicators": "GET /macro/indicators",
            "vcInvestment": "GET /vc/investment",
            "recommendations": "POST /recommendations",
            "auditHardening": "GET /audit/hardening",
            "pipelineCatalog": "GET /pipelines/catalog",
            "pipelinePlan": "POST /pipelines/plan",
            "pipelineSubmit": "POST /pipelines/submit",
            "observability": "GET /observability",
            "integrationHealth": "GET /integrations/health",
            "equations": "GET /model/equations",
            "schema": "GET /schema",
            "example": "GET /example",
            "desEngine": "GET /engine/des",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics"
        },
        "nats": {
            "requestSubject": state.config.request_subject,
            "queueGroup": state.config.queue_group,
            "resultSubject": state.config.result_subject,
            "marketEventSubject": state.config.market_event_subject,
            "runtimeEventSubject": state.config.runtime_event_subject,
            "pipelineIntentSubject": state.config.pipeline_intent_subject
        },
        "desEngine": des_surface_descriptor(),
        "sentiment": {
            "credentialStatus": &state.config.sentiment_credentials,
            "sourcesRoute": "GET /sentiment/sources",
            "analyzeRoute": "POST /sentiment/analyze"
        },
        "marketData": {
            "credentialStatus": &state.config.market_data_credentials,
            "publicSourcesRoute": "GET /sources/public",
            "macroRoute": "GET /macro/indicators",
            "vcRoute": "GET /vc/investment",
            "recommendationsRoute": "POST /recommendations"
        },
        "pipelines": {
            "status": pipeline_integration_status(state),
            "catalogRoute": "GET /pipelines/catalog",
            "planRoute": "POST /pipelines/plan",
            "submitRoute": "POST /pipelines/submit",
            "integrationHealthRoute": "GET /integrations/health",
            "auditRoute": "GET /audit/hardening"
        },
        "integrations": integration_health_payload(state),
        "observability": observability_payload(state),
        "equationCount": equation_catalog().len(),
        "sourceCount": source_catalog().len(),
        "publicSourceTemplateCount": public_source_templates().len(),
        "atMs": now_ms()
    })
}
