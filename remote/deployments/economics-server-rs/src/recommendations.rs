use std::collections::BTreeMap;

use crate::dashboard::*;
use crate::forecast::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn generate_recommendations(
    config: &Config,
    request: RecommendationRequest,
) -> Result<RecommendationsResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    validate_macro_context(request.macro_context.as_ref())?;
    validate_macro_fiscal_context(request.macro_fiscal_context.as_ref())?;
    validate_venture_capital_context(request.venture_capital_context.as_ref())?;
    validate_sentiment_context(request.sentiment_context.as_ref())?;

    let request_id = request_id(request.request_id.as_ref(), "economics-recommendations");
    let horizon_months = request
        .horizon_months
        .unwrap_or(config.projection_months)
        .clamp(1, 120);
    let company_limit = request.company_limit.unwrap_or(20).clamp(1, 20);
    let commodity_limit = request.commodity_limit.unwrap_or(30).clamp(1, 30);
    let scenario = request
        .scenario
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    let macro_context = request.macro_context.unwrap_or_default();
    let macro_fiscal_context = request
        .macro_fiscal_context
        .unwrap_or_else(default_macro_fiscal_context);
    let venture_capital_context = match request.venture_capital_context {
        Some(context) if !context.deals.is_empty() || !context.sector_flows.is_empty() => context,
        _ => sample_venture_capital_context(),
    };
    let sentiment_context = request.sentiment_context.unwrap_or_default();
    let series = request.series.unwrap_or_else(sample_market_series);
    validate_series(&series)?;
    let series_hints = series_signal_hints(config, &series)?;

    let mut company_scores = company_candidates()
        .into_iter()
        .map(|candidate| {
            score_company_candidate(
                &candidate,
                &macro_context,
                &macro_fiscal_context,
                &venture_capital_context,
                &sentiment_context,
                &series_hints,
                &scenario,
                horizon_months,
            )
        })
        .collect::<Vec<_>>();
    company_scores.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut commodity_scores = commodity_candidates()
        .into_iter()
        .map(|candidate| {
            score_commodity_candidate(
                &candidate,
                &macro_context,
                &macro_fiscal_context,
                &sentiment_context,
                &series_hints,
                &scenario,
                horizon_months,
            )
        })
        .collect::<Vec<_>>();
    commodity_scores.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let company_buys = company_scores
        .iter()
        .take(company_limit)
        .enumerate()
        .map(|(index, item)| company_with_action(item, index + 1, "invest"))
        .collect::<Vec<_>>();
    let mut company_dump_source = company_scores.clone();
    company_dump_source.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let company_dumps = company_dump_source
        .iter()
        .take(company_limit)
        .enumerate()
        .map(|(index, item)| company_with_action(item, index + 1, "dump-or-hedge"))
        .collect::<Vec<_>>();

    let commodity_buys = commodity_scores
        .iter()
        .take(commodity_limit)
        .enumerate()
        .map(|(index, item)| commodity_with_action(item, index + 1, "buy"))
        .collect::<Vec<_>>();
    let mut commodity_sell_source = commodity_scores.clone();
    commodity_sell_source.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let commodity_sells_or_dumps = commodity_sell_source
        .iter()
        .take(commodity_limit)
        .enumerate()
        .map(|(index, item)| commodity_with_action(item, index + 1, "sell-or-dump"))
        .collect::<Vec<_>>();

    let mut warnings = vec![
        "rankings are model signals for research workflows, not financial advice".to_string(),
        "built-in candidate universes are placeholders until live market, macro, and private-data adapters are connected".to_string(),
    ];
    if series
        .iter()
        .any(|item| item.source.as_deref() == Some("built-in-sample"))
    {
        warnings.push(
            "using built-in demonstration market series for observed signal hints".to_string(),
        );
    }

    Ok(RecommendationsResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        horizon_months,
        scenario,
        generated_at_ms: now_ms(),
        macro_fiscal_context,
        venture_capital_context,
        data_credential_status: config.market_data_credentials.clone(),
        company_buys,
        company_dumps,
        commodity_buys,
        commodity_sells_or_dumps,
        methodology: vec![
            "company scores blend profitability, growth, balance-sheet strength, valuation, momentum, macro/fiscal/labor pressure, VC sector flow, sentiment, and observed series hints".to_string(),
            "commodity scores blend demand growth, supply tightness, inventory pressure, carry, geopolitical risk, valuation, inflation/real-rate/fiscal effects, sentiment, and observed series hints".to_string(),
            "theoretical priors stay transparent: CAPM/Taylor/Fisher/UIP/PPP/Hotelling/Phillips-style forces are represented through bounded model components".to_string(),
        ],
        warnings,
    })
}

pub(crate) fn series_signal_hints(
    config: &Config,
    series: &[MarketSeries],
) -> Result<BTreeMap<String, f64>, String> {
    let mut hints = BTreeMap::new();
    for item in series {
        let stats = series_stats(item, config.history_years)?;
        let signal = clamp(0.55 * stats.data_drift + 0.45 * stats.momentum, -0.50, 0.50);
        hints.insert(item.instrument_id.to_ascii_lowercase(), signal);
    }
    Ok(hints)
}

pub(crate) fn score_company_candidate(
    candidate: &CompanyCandidate,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
    vc_context: &VentureCapitalContext,
    sentiment_context: &SentimentSignalContext,
    series_hints: &BTreeMap<String, f64>,
    scenario: &str,
    horizon_months: u32,
) -> CompanyRecommendation {
    let quality = 0.20 * candidate.profitability
        + 0.18 * candidate.growth
        + 0.14 * candidate.balance_sheet
        + 0.12 * candidate.momentum;
    let valuation = -0.16 * candidate.valuation_gap;
    let real_rate = finite_or(macro_context.policy_rate, 0.045)
        - finite_or(
            macro_context.expected_inflation,
            finite_or(macro_context.inflation, 0.030),
        );
    let rate_sensitivity = -0.16 * candidate.beta * real_rate;
    let macro_fiscal = sector_macro_adjustment(candidate.sector, macro_context, fiscal_context)
        + 0.20 * fiscal_equity_bias(fiscal_context);
    let vc_flow = vc_sector_impulse(vc_context, candidate.sector);
    let sentiment = sentiment_adjustment(sentiment_context, candidate.ticker, candidate.sector);
    let observed = series_hints
        .get(&candidate.ticker.to_ascii_lowercase())
        .copied()
        .unwrap_or(0.0)
        * 0.35;
    let scenario_component =
        scenario_company_adjustment(candidate.sector, candidate.beta, scenario, fiscal_context);
    let score = clamp(
        quality
            + valuation
            + rate_sensitivity
            + macro_fiscal
            + vc_flow
            + sentiment
            + observed
            + scenario_component,
        -1.0,
        1.0,
    );
    let horizon_scale = f64::from(horizon_months) / 18.0;
    let expected_return_18m = clamp(score * 0.46 * horizon_scale, -0.85, 0.95);
    let confidence = clamp(
        0.48 + score.abs() * 0.24 + vc_flow.abs() * 0.18 + candidate.balance_sheet.max(0.0) * 0.08,
        0.30,
        0.92,
    );
    CompanyRecommendation {
        rank: 0,
        ticker: candidate.ticker.to_string(),
        company: candidate.company.to_string(),
        sector: candidate.sector.to_string(),
        stage: candidate.stage.to_string(),
        action: "candidate".to_string(),
        score: round6(score),
        expected_return_18m: round6(expected_return_18m),
        confidence: round6(confidence),
        reasons: company_reasons(candidate, score, macro_fiscal, vc_flow, sentiment),
        components: vec![
            recommendation_component("qualityGrowthMomentum", quality, 1.0),
            recommendation_component("valuation", valuation, 1.0),
            recommendation_component("rateSensitivity", rate_sensitivity, 1.0),
            recommendation_component("macroFiscalLabor", macro_fiscal, 1.0),
            recommendation_component("ventureCapitalFlow", vc_flow, 1.0),
            recommendation_component("sentiment", sentiment, 1.0),
            recommendation_component("observedSeriesSignal", observed, 1.0),
            recommendation_component("scenario", scenario_component, 1.0),
        ],
    }
}

pub(crate) fn score_commodity_candidate(
    candidate: &CommodityCandidate,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
    sentiment_context: &SentimentSignalContext,
    series_hints: &BTreeMap<String, f64>,
    scenario: &str,
    horizon_months: u32,
) -> CommodityRecommendation {
    let fundamentals = 0.22 * candidate.demand_growth
        + 0.20 * candidate.supply_tightness
        + 0.16 * candidate.inventory_pressure
        + 0.10 * candidate.carry
        + 0.08 * candidate.geopolitical_risk;
    let valuation = -0.16 * candidate.valuation_gap;
    let macro_component =
        commodity_macro_adjustment(candidate.commodity_class, macro_context, fiscal_context);
    let sentiment = sentiment_adjustment(
        sentiment_context,
        candidate.instrument_id,
        candidate.commodity_class,
    );
    let observed = series_hints
        .get(&candidate.instrument_id.to_ascii_lowercase())
        .copied()
        .unwrap_or(0.0)
        * 0.45;
    let scenario_component = scenario_commodity_adjustment(candidate.commodity_class, scenario);
    let score = clamp(
        fundamentals + valuation + macro_component + sentiment + observed + scenario_component,
        -1.0,
        1.0,
    );
    let horizon_scale = f64::from(horizon_months) / 18.0;
    let expected_return_18m = clamp(score * 0.42 * horizon_scale, -0.80, 0.90);
    let confidence = clamp(
        0.46 + score.abs() * 0.22 + candidate.volatility.max(0.0) * 0.08,
        0.28,
        0.90,
    );
    CommodityRecommendation {
        rank: 0,
        instrument_id: candidate.instrument_id.to_string(),
        commodity: candidate.commodity.to_string(),
        commodity_class: candidate.commodity_class.to_string(),
        action: "candidate".to_string(),
        score: round6(score),
        expected_return_18m: round6(expected_return_18m),
        confidence: round6(confidence),
        reasons: commodity_reasons(candidate, score, macro_component, sentiment),
        components: vec![
            recommendation_component("fundamentals", fundamentals, 1.0),
            recommendation_component("valuation", valuation, 1.0),
            recommendation_component("macroFiscalInflation", macro_component, 1.0),
            recommendation_component("sentiment", sentiment, 1.0),
            recommendation_component("observedSeriesSignal", observed, 1.0),
            recommendation_component("scenario", scenario_component, 1.0),
        ],
    }
}

pub(crate) fn company_with_action(
    item: &CompanyRecommendation,
    rank: usize,
    action: &str,
) -> CompanyRecommendation {
    let mut clone = item.clone();
    clone.rank = rank;
    clone.action = action.to_string();
    clone
}

pub(crate) fn commodity_with_action(
    item: &CommodityRecommendation,
    rank: usize,
    action: &str,
) -> CommodityRecommendation {
    let mut clone = item.clone();
    clone.rank = rank;
    clone.action = action.to_string();
    clone
}

pub(crate) fn recommendation_component(name: &str, value: f64, weight: f64) -> RecommendationComponent {
    RecommendationComponent {
        name: name.to_string(),
        value: round6(value),
        weight: round6(weight),
    }
}

pub(crate) fn company_reasons(
    candidate: &CompanyCandidate,
    score: f64,
    macro_fiscal: f64,
    vc_flow: f64,
    sentiment: f64,
) -> Vec<String> {
    let mut reasons = vec![format!(
        "{} model score {:.2} with growth {:.2}, profitability {:.2}, and valuation gap {:.2}",
        candidate.sector, score, candidate.growth, candidate.profitability, candidate.valuation_gap
    )];
    if macro_fiscal.abs() > 0.02 {
        reasons.push(format!(
            "macro/fiscal/labor contribution {:.2}",
            macro_fiscal
        ));
    }
    if vc_flow.abs() > 0.02 {
        reasons.push(format!("VC sector-flow contribution {:.2}", vc_flow));
    }
    if sentiment.abs() > 0.01 {
        reasons.push(format!("sentiment contribution {:.2}", sentiment));
    }
    if score < 0.0 {
        reasons.push(
            "negative composite score flags dump, hedge, or avoid research priority".to_string(),
        );
    } else {
        reasons
            .push("positive composite score flags invest or deeper diligence priority".to_string());
    }
    reasons
}

pub(crate) fn commodity_reasons(
    candidate: &CommodityCandidate,
    score: f64,
    macro_component: f64,
    sentiment: f64,
) -> Vec<String> {
    let mut reasons = vec![format!(
        "{} score {:.2} from demand {:.2}, supply tightness {:.2}, inventory pressure {:.2}",
        candidate.commodity_class,
        score,
        candidate.demand_growth,
        candidate.supply_tightness,
        candidate.inventory_pressure
    )];
    if macro_component.abs() > 0.02 {
        reasons.push(format!(
            "inflation/real-rate/fiscal contribution {:.2}",
            macro_component
        ));
    }
    if sentiment.abs() > 0.01 {
        reasons.push(format!("sentiment contribution {:.2}", sentiment));
    }
    if score < 0.0 {
        reasons.push(
            "negative composite score flags sell, dump, or avoid research priority".to_string(),
        );
    } else {
        reasons
            .push("positive composite score flags buy or accumulate research priority".to_string());
    }
    reasons
}

pub(crate) fn fiscal_equity_bias(fiscal_context: &MacroFiscalContext) -> f64 {
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let payroll_growth = finite_or(fiscal_context.payroll_growth, 0.014);
    let unemployment = finite_or(fiscal_context.unemployment_rate, 0.040);
    clamp(
        1.5 * gdp_growth + productivity + payroll_growth
            - fiscal_stress(fiscal_context)
            - 0.4 * unemployment,
        -0.20,
        0.20,
    )
}

pub(crate) fn fiscal_stress(fiscal_context: &MacroFiscalContext) -> f64 {
    let gdp = finite_or(fiscal_context.gdp, 29_000_000_000_000.0).max(1.0);
    let deficit_to_gdp = finite_or(
        fiscal_context.deficit_to_gdp,
        finite_or(fiscal_context.deficit, 1_800_000_000_000.0) / gdp,
    );
    let debt_to_gdp = finite_or(
        fiscal_context.debt_to_gdp,
        finite_or(fiscal_context.national_debt, 36_000_000_000_000.0) / gdp,
    );
    let borrowing_to_gdp = finite_or(fiscal_context.borrowing, 1_900_000_000_000.0) / gdp;
    let interest_to_gdp = finite_or(fiscal_context.net_interest_outlays, 950_000_000_000.0) / gdp;
    clamp(
        0.55 * deficit_to_gdp
            + 0.08 * debt_to_gdp
            + 0.60 * borrowing_to_gdp
            + 1.10 * interest_to_gdp,
        0.0,
        0.35,
    )
}

pub(crate) fn sector_macro_adjustment(
    sector: &str,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = sector.to_ascii_lowercase();
    let inflation = finite_or(macro_context.inflation, 0.030);
    let expected_inflation = finite_or(macro_context.expected_inflation, inflation);
    let real_rate = finite_or(macro_context.policy_rate, 0.045) - expected_inflation;
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let labor = finite_or(fiscal_context.labor_force_participation, 0.626) - 0.620;
    let wage_growth = finite_or(fiscal_context.wage_growth, 0.041);
    let deficit = finite_or(fiscal_context.deficit_to_gdp, 0.062);
    let stress = fiscal_stress(fiscal_context);
    let base = 1.2 * gdp_growth + 0.8 * productivity + 0.6 * labor - 0.8 * stress;
    let adjustment = if lower.contains("technology")
        || lower.contains("artificial")
        || lower.contains("software")
        || lower.contains("semiconductor")
    {
        base + 2.2 * productivity - 1.8 * real_rate
    } else if lower.contains("financial") || lower.contains("fintech") {
        base + 0.9 * real_rate - 0.8 * stress
    } else if lower.contains("energy") || lower.contains("materials") {
        base + 1.6 * inflation + 0.8 * deficit
    } else if lower.contains("industrial") || lower.contains("defense") {
        base + 0.7 * deficit + 0.8 * productivity
    } else if lower.contains("consumer") || lower.contains("retail") {
        base + 1.1 * wage_growth + 0.9 * labor - 0.7 * inflation
    } else if lower.contains("real-estate") || lower.contains("utilities") {
        base - 2.4 * real_rate - 0.5 * stress
    } else if lower.contains("health") || lower.contains("biotech") {
        0.5 * base - 0.4 * stress + 0.5 * productivity
    } else {
        base - 0.8 * real_rate
    };
    clamp(adjustment, -0.18, 0.18)
}

pub(crate) fn commodity_macro_adjustment(
    commodity_class: &str,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = commodity_class.to_ascii_lowercase();
    let inflation = finite_or(macro_context.inflation, 0.030);
    let expected_inflation = finite_or(macro_context.expected_inflation, inflation);
    let real_rate = finite_or(macro_context.policy_rate, 0.045) - expected_inflation;
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let stress = fiscal_stress(fiscal_context);
    let labor = finite_or(fiscal_context.labor_force_participation, 0.626) - 0.620;
    let base = 1.1 * gdp_growth + 1.3 * inflation - 1.0 * real_rate + 0.35 * stress;
    let adjustment = if lower.contains("precious")
        || lower.contains("gold")
        || lower.contains("silver")
    {
        base + 2.4 * inflation - 2.2 * real_rate + 0.6 * stress
    } else if lower.contains("energy") {
        base + 1.7 * gdp_growth + 0.8 * inflation
    } else if lower.contains("industrial") || lower.contains("battery") || lower.contains("bulk") {
        base + 1.5 * gdp_growth + 1.2 * productivity
    } else if lower.contains("agriculture") || lower.contains("food") || lower.contains("livestock")
    {
        base + 1.2 * inflation + 0.5 * labor
    } else if lower.contains("carbon") || lower.contains("freight") {
        base + 1.0 * productivity + 0.8 * gdp_growth
    } else {
        base
    };
    clamp(adjustment, -0.22, 0.24)
}

pub(crate) fn vc_sector_impulse(context: &VentureCapitalContext, sector: &str) -> f64 {
    let mut score = 0.0;
    let mut weight = 0.0;
    for flow in &context.sector_flows {
        if sector_matches(sector, &flow.sector) {
            let confidence = finite_or(flow.confidence, 0.50).clamp(0.0, 1.0);
            let capital_score = (flow.invested_capital.max(0.0) / 10_000_000_000.0)
                .ln_1p()
                .min(3.0)
                / 3.0;
            let flow_score = 0.55 * flow.yoy_growth
                + 0.25 * capital_score
                + 0.20 * finite_or(flow.exit_liquidity, 0.20);
            score += flow_score * confidence;
            weight += confidence;
        }
    }
    for deal in &context.deals {
        if sector_matches(sector, &deal.sector) {
            let confidence = finite_or(deal.confidence, 0.45).clamp(0.0, 1.0);
            let stage_boost = if deal.stage.to_ascii_lowercase().contains("late") {
                0.08
            } else if deal.stage.to_ascii_lowercase().contains("growth") {
                0.05
            } else {
                0.02
            };
            let amount_score = (deal.amount.max(0.0) / 1_000_000_000.0).ln_1p().min(2.0) / 2.0;
            score += (0.20 * amount_score + stage_boost) * confidence;
            weight += confidence;
        }
    }
    if weight <= f64::EPSILON {
        0.0
    } else {
        clamp(score / weight, -0.18, 0.24)
    }
}

pub(crate) fn sector_matches(candidate_sector: &str, signal_sector: &str) -> bool {
    let candidate = candidate_sector.to_ascii_lowercase();
    let signal = signal_sector.to_ascii_lowercase();
    if candidate.contains(&signal) || signal.contains(&candidate) {
        return true;
    }
    let aliases: &[&str] = if candidate.contains("technology") || candidate.contains("software") {
        &[
            "ai",
            "artificial",
            "software",
            "data",
            "cloud",
            "cyber",
            "semiconductor",
        ]
    } else if candidate.contains("health") || candidate.contains("biotech") {
        &["biotech", "health", "pharma", "life-science"]
    } else if candidate.contains("energy") {
        &["energy", "climate", "fusion", "grid", "battery"]
    } else if candidate.contains("financial") || candidate.contains("fintech") {
        &["fintech", "payments", "banking", "financial"]
    } else if candidate.contains("industrial") || candidate.contains("defense") {
        &[
            "industrial",
            "automation",
            "defense",
            "manufacturing",
            "robotics",
        ]
    } else if candidate.contains("consumer") {
        &["consumer", "retail", "marketplace"]
    } else {
        &[]
    };
    aliases
        .iter()
        .any(|alias| candidate.contains(alias) || signal.contains(alias))
}

pub(crate) fn sentiment_adjustment(
    context: &SentimentSignalContext,
    instrument_id: &str,
    sector_or_class: &str,
) -> f64 {
    let average = finite_or(context.average_sentiment, 0.0);
    let instrument = lookup_context_score(context.instrument_scores.as_ref(), instrument_id)
        .unwrap_or(average * 0.5);
    let sector = lookup_context_score(context.sector_scores.as_ref(), sector_or_class)
        .unwrap_or(average * 0.5);
    clamp(
        0.07 * instrument + 0.05 * sector + 0.03 * average,
        -0.15,
        0.15,
    )
}

pub(crate) fn lookup_context_score(map: Option<&BTreeMap<String, f64>>, key: &str) -> Option<f64> {
    let key_lower = key.to_ascii_lowercase();
    map.and_then(|scores| {
        scores.iter().find_map(|(candidate, value)| {
            let candidate_lower = candidate.to_ascii_lowercase();
            if candidate_lower == key_lower
                || candidate_lower.contains(&key_lower)
                || key_lower.contains(&candidate_lower)
            {
                value.is_finite().then_some(*value)
            } else {
                None
            }
        })
    })
}

pub(crate) fn scenario_company_adjustment(
    sector: &str,
    beta: f64,
    scenario: &str,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = sector.to_ascii_lowercase();
    let adjustment = match scenario {
        "liquidity-crunch" => -0.08 * beta - 0.05 * fiscal_stress(fiscal_context),
        "oil-shock" if lower.contains("energy") => 0.10,
        "oil-shock" if lower.contains("consumer") || lower.contains("transport") => -0.08,
        "dollar-strength" if lower.contains("materials") || lower.contains("energy") => -0.05,
        "deflation" if lower.contains("utilities") || lower.contains("health") => 0.03,
        "deflation" => -0.05 * beta,
        "soft-landing" => 0.04 + 0.02 * beta,
        _ => 0.0,
    };
    clamp(adjustment, -0.14, 0.14)
}

pub(crate) fn scenario_commodity_adjustment(commodity_class: &str, scenario: &str) -> f64 {
    let lower = commodity_class.to_ascii_lowercase();
    let adjustment = match scenario {
        "oil-shock" if lower.contains("energy") => 0.18,
        "oil-shock" if lower.contains("agriculture") || lower.contains("food") => 0.05,
        "liquidity-crunch" => -0.06,
        "dollar-strength" if lower.contains("precious") || lower.contains("industrial") => -0.07,
        "deflation" if lower.contains("precious") => -0.04,
        "deflation" => -0.08,
        "soft-landing" if lower.contains("industrial") || lower.contains("energy") => 0.06,
        _ => 0.0,
    };
    clamp(adjustment, -0.18, 0.18)
}

pub(crate) fn company_candidates() -> Vec<CompanyCandidate> {
    vec![
        CompanyCandidate {
            ticker: "NVDA",
            company: "NVIDIA",
            sector: "technology-semiconductor-ai",
            stage: "public",
            beta: 1.7,
            profitability: 0.92,
            growth: 0.95,
            balance_sheet: 0.72,
            valuation_gap: 0.38,
            momentum: 0.88,
        },
        CompanyCandidate {
            ticker: "MSFT",
            company: "Microsoft",
            sector: "technology-software-cloud-ai",
            stage: "public",
            beta: 1.0,
            profitability: 0.90,
            growth: 0.62,
            balance_sheet: 0.86,
            valuation_gap: 0.20,
            momentum: 0.56,
        },
        CompanyCandidate {
            ticker: "AVGO",
            company: "Broadcom",
            sector: "technology-semiconductor-infrastructure",
            stage: "public",
            beta: 1.3,
            profitability: 0.82,
            growth: 0.62,
            balance_sheet: 0.55,
            valuation_gap: 0.18,
            momentum: 0.63,
        },
        CompanyCandidate {
            ticker: "GOOGL",
            company: "Alphabet",
            sector: "technology-advertising-ai",
            stage: "public",
            beta: 1.1,
            profitability: 0.78,
            growth: 0.48,
            balance_sheet: 0.88,
            valuation_gap: -0.05,
            momentum: 0.42,
        },
        CompanyCandidate {
            ticker: "AMZN",
            company: "Amazon",
            sector: "technology-cloud-consumer",
            stage: "public",
            beta: 1.4,
            profitability: 0.54,
            growth: 0.58,
            balance_sheet: 0.48,
            valuation_gap: 0.10,
            momentum: 0.45,
        },
        CompanyCandidate {
            ticker: "META",
            company: "Meta Platforms",
            sector: "technology-advertising-ai",
            stage: "public",
            beta: 1.2,
            profitability: 0.84,
            growth: 0.52,
            balance_sheet: 0.76,
            valuation_gap: 0.02,
            momentum: 0.48,
        },
        CompanyCandidate {
            ticker: "AMD",
            company: "Advanced Micro Devices",
            sector: "technology-semiconductor-ai",
            stage: "public",
            beta: 1.8,
            profitability: 0.44,
            growth: 0.64,
            balance_sheet: 0.50,
            valuation_gap: 0.30,
            momentum: 0.36,
        },
        CompanyCandidate {
            ticker: "AAPL",
            company: "Apple",
            sector: "technology-consumer-hardware",
            stage: "public",
            beta: 1.0,
            profitability: 0.86,
            growth: 0.22,
            balance_sheet: 0.66,
            valuation_gap: 0.16,
            momentum: 0.20,
        },
        CompanyCandidate {
            ticker: "TSLA",
            company: "Tesla",
            sector: "consumer-energy-automation",
            stage: "public",
            beta: 2.0,
            profitability: 0.34,
            growth: 0.38,
            balance_sheet: 0.42,
            valuation_gap: 0.44,
            momentum: -0.08,
        },
        CompanyCandidate {
            ticker: "ASML",
            company: "ASML",
            sector: "technology-semiconductor-equipment",
            stage: "public",
            beta: 1.2,
            profitability: 0.80,
            growth: 0.44,
            balance_sheet: 0.72,
            valuation_gap: 0.12,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "JPM",
            company: "JPMorgan Chase",
            sector: "financials-banking",
            stage: "public",
            beta: 1.1,
            profitability: 0.70,
            growth: 0.22,
            balance_sheet: 0.66,
            valuation_gap: -0.04,
            momentum: 0.28,
        },
        CompanyCandidate {
            ticker: "GS",
            company: "Goldman Sachs",
            sector: "financials-capital-markets",
            stage: "public",
            beta: 1.3,
            profitability: 0.58,
            growth: 0.18,
            balance_sheet: 0.48,
            valuation_gap: 0.02,
            momentum: 0.18,
        },
        CompanyCandidate {
            ticker: "V",
            company: "Visa",
            sector: "financials-payments-fintech",
            stage: "public",
            beta: 0.9,
            profitability: 0.88,
            growth: 0.36,
            balance_sheet: 0.78,
            valuation_gap: 0.12,
            momentum: 0.24,
        },
        CompanyCandidate {
            ticker: "MA",
            company: "Mastercard",
            sector: "financials-payments-fintech",
            stage: "public",
            beta: 1.0,
            profitability: 0.86,
            growth: 0.38,
            balance_sheet: 0.70,
            valuation_gap: 0.15,
            momentum: 0.25,
        },
        CompanyCandidate {
            ticker: "BRK.B",
            company: "Berkshire Hathaway",
            sector: "financials-industrials-insurance",
            stage: "public",
            beta: 0.8,
            profitability: 0.62,
            growth: 0.18,
            balance_sheet: 0.92,
            valuation_gap: -0.03,
            momentum: 0.18,
        },
        CompanyCandidate {
            ticker: "LLY",
            company: "Eli Lilly",
            sector: "healthcare-biotech-pharma",
            stage: "public",
            beta: 0.7,
            profitability: 0.76,
            growth: 0.70,
            balance_sheet: 0.58,
            valuation_gap: 0.36,
            momentum: 0.62,
        },
        CompanyCandidate {
            ticker: "UNH",
            company: "UnitedHealth Group",
            sector: "healthcare-services",
            stage: "public",
            beta: 0.7,
            profitability: 0.64,
            growth: 0.24,
            balance_sheet: 0.54,
            valuation_gap: -0.02,
            momentum: -0.12,
        },
        CompanyCandidate {
            ticker: "MRK",
            company: "Merck",
            sector: "healthcare-pharma",
            stage: "public",
            beta: 0.5,
            profitability: 0.66,
            growth: 0.20,
            balance_sheet: 0.58,
            valuation_gap: -0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "PFE",
            company: "Pfizer",
            sector: "healthcare-pharma",
            stage: "public",
            beta: 0.6,
            profitability: 0.28,
            growth: -0.18,
            balance_sheet: 0.36,
            valuation_gap: -0.30,
            momentum: -0.34,
        },
        CompanyCandidate {
            ticker: "XOM",
            company: "Exxon Mobil",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 0.9,
            profitability: 0.64,
            growth: 0.14,
            balance_sheet: 0.72,
            valuation_gap: -0.10,
            momentum: 0.15,
        },
        CompanyCandidate {
            ticker: "CVX",
            company: "Chevron",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 0.9,
            profitability: 0.58,
            growth: 0.08,
            balance_sheet: 0.76,
            valuation_gap: -0.08,
            momentum: 0.06,
        },
        CompanyCandidate {
            ticker: "COP",
            company: "ConocoPhillips",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 1.0,
            profitability: 0.62,
            growth: 0.16,
            balance_sheet: 0.60,
            valuation_gap: -0.02,
            momentum: 0.14,
        },
        CompanyCandidate {
            ticker: "NEE",
            company: "NextEra Energy",
            sector: "utilities-climate-energy",
            stage: "public",
            beta: 0.7,
            profitability: 0.42,
            growth: 0.18,
            balance_sheet: 0.24,
            valuation_gap: 0.08,
            momentum: -0.20,
        },
        CompanyCandidate {
            ticker: "CAT",
            company: "Caterpillar",
            sector: "industrials-machinery",
            stage: "public",
            beta: 1.1,
            profitability: 0.62,
            growth: 0.22,
            balance_sheet: 0.52,
            valuation_gap: 0.04,
            momentum: 0.20,
        },
        CompanyCandidate {
            ticker: "DE",
            company: "Deere",
            sector: "industrials-agriculture-machinery",
            stage: "public",
            beta: 1.0,
            profitability: 0.60,
            growth: 0.10,
            balance_sheet: 0.46,
            valuation_gap: -0.04,
            momentum: 0.02,
        },
        CompanyCandidate {
            ticker: "GE",
            company: "GE Aerospace",
            sector: "industrials-aerospace",
            stage: "public",
            beta: 1.1,
            profitability: 0.56,
            growth: 0.30,
            balance_sheet: 0.42,
            valuation_gap: 0.18,
            momentum: 0.46,
        },
        CompanyCandidate {
            ticker: "RTX",
            company: "RTX",
            sector: "industrials-defense",
            stage: "public",
            beta: 0.8,
            profitability: 0.46,
            growth: 0.14,
            balance_sheet: 0.36,
            valuation_gap: 0.00,
            momentum: 0.12,
        },
        CompanyCandidate {
            ticker: "LMT",
            company: "Lockheed Martin",
            sector: "industrials-defense",
            stage: "public",
            beta: 0.6,
            profitability: 0.52,
            growth: 0.08,
            balance_sheet: 0.40,
            valuation_gap: -0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "COST",
            company: "Costco",
            sector: "consumer-retail",
            stage: "public",
            beta: 0.8,
            profitability: 0.60,
            growth: 0.26,
            balance_sheet: 0.66,
            valuation_gap: 0.28,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "WMT",
            company: "Walmart",
            sector: "consumer-retail",
            stage: "public",
            beta: 0.6,
            profitability: 0.48,
            growth: 0.22,
            balance_sheet: 0.58,
            valuation_gap: 0.08,
            momentum: 0.24,
        },
        CompanyCandidate {
            ticker: "HD",
            company: "Home Depot",
            sector: "consumer-housing-retail",
            stage: "public",
            beta: 1.0,
            profitability: 0.62,
            growth: 0.08,
            balance_sheet: 0.34,
            valuation_gap: 0.08,
            momentum: 0.06,
        },
        CompanyCandidate {
            ticker: "MCD",
            company: "McDonald's",
            sector: "consumer-staples-restaurants",
            stage: "public",
            beta: 0.6,
            profitability: 0.66,
            growth: 0.12,
            balance_sheet: 0.28,
            valuation_gap: 0.10,
            momentum: 0.10,
        },
        CompanyCandidate {
            ticker: "PG",
            company: "Procter & Gamble",
            sector: "consumer-staples",
            stage: "public",
            beta: 0.5,
            profitability: 0.58,
            growth: 0.08,
            balance_sheet: 0.50,
            valuation_gap: 0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "KO",
            company: "Coca-Cola",
            sector: "consumer-staples",
            stage: "public",
            beta: 0.5,
            profitability: 0.60,
            growth: 0.10,
            balance_sheet: 0.46,
            valuation_gap: 0.04,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "PLD",
            company: "Prologis",
            sector: "real-estate-industrial",
            stage: "public",
            beta: 1.0,
            profitability: 0.42,
            growth: 0.14,
            balance_sheet: 0.34,
            valuation_gap: 0.06,
            momentum: -0.08,
        },
        CompanyCandidate {
            ticker: "AMT",
            company: "American Tower",
            sector: "real-estate-infrastructure",
            stage: "public",
            beta: 0.8,
            profitability: 0.44,
            growth: 0.12,
            balance_sheet: 0.28,
            valuation_gap: 0.02,
            momentum: -0.10,
        },
        CompanyCandidate {
            ticker: "COIN",
            company: "Coinbase",
            sector: "financials-crypto",
            stage: "public",
            beta: 2.4,
            profitability: 0.30,
            growth: 0.58,
            balance_sheet: 0.42,
            valuation_gap: 0.26,
            momentum: 0.40,
        },
        CompanyCandidate {
            ticker: "RBLX",
            company: "Roblox",
            sector: "technology-consumer-platform",
            stage: "public",
            beta: 1.6,
            profitability: -0.22,
            growth: 0.28,
            balance_sheet: 0.16,
            valuation_gap: 0.32,
            momentum: -0.12,
        },
        CompanyCandidate {
            ticker: "OPENAI-PRIVATE",
            company: "OpenAI",
            sector: "technology-artificial-intelligence",
            stage: "late-private",
            beta: 1.8,
            profitability: -0.10,
            growth: 0.98,
            balance_sheet: 0.34,
            valuation_gap: 0.55,
            momentum: 0.70,
        },
        CompanyCandidate {
            ticker: "ANTHROPIC-PRIVATE",
            company: "Anthropic",
            sector: "technology-artificial-intelligence",
            stage: "late-private",
            beta: 1.7,
            profitability: -0.18,
            growth: 0.92,
            balance_sheet: 0.30,
            valuation_gap: 0.46,
            momentum: 0.66,
        },
        CompanyCandidate {
            ticker: "DATABRICKS-PRIVATE",
            company: "Databricks",
            sector: "technology-data-infrastructure",
            stage: "late-private",
            beta: 1.5,
            profitability: 0.08,
            growth: 0.72,
            balance_sheet: 0.38,
            valuation_gap: 0.30,
            momentum: 0.52,
        },
        CompanyCandidate {
            ticker: "STRIPE-PRIVATE",
            company: "Stripe",
            sector: "financials-payments-fintech",
            stage: "late-private",
            beta: 1.3,
            profitability: 0.14,
            growth: 0.50,
            balance_sheet: 0.42,
            valuation_gap: 0.20,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "ANDURIL-PRIVATE",
            company: "Anduril",
            sector: "industrials-defense-automation",
            stage: "late-private",
            beta: 1.4,
            profitability: -0.05,
            growth: 0.68,
            balance_sheet: 0.32,
            valuation_gap: 0.24,
            momentum: 0.48,
        },
        CompanyCandidate {
            ticker: "CFS-PRIVATE",
            company: "Commonwealth Fusion Systems",
            sector: "energy-climate",
            stage: "growth-private",
            beta: 1.8,
            profitability: -0.40,
            growth: 0.84,
            balance_sheet: 0.18,
            valuation_gap: 0.42,
            momentum: 0.42,
        },
    ]
}

pub(crate) fn commodity_candidates() -> Vec<CommodityCandidate> {
    vec![
        CommodityCandidate {
            instrument_id: "CL=F",
            commodity: "WTI crude oil",
            commodity_class: "energy",
            supply_tightness: 0.18,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: -0.02,
            geopolitical_risk: 0.38,
            valuation_gap: 0.02,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "BZ=F",
            commodity: "Brent crude oil",
            commodity_class: "energy",
            supply_tightness: 0.16,
            demand_growth: 0.12,
            inventory_pressure: 0.06,
            carry: -0.02,
            geopolitical_risk: 0.42,
            valuation_gap: 0.03,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "NG=F",
            commodity: "Natural gas",
            commodity_class: "energy",
            supply_tightness: -0.10,
            demand_growth: 0.18,
            inventory_pressure: -0.12,
            carry: -0.05,
            geopolitical_risk: 0.24,
            valuation_gap: -0.12,
            volatility: 0.58,
        },
        CommodityCandidate {
            instrument_id: "RB=F",
            commodity: "Gasoline",
            commodity_class: "energy-refined",
            supply_tightness: 0.12,
            demand_growth: 0.08,
            inventory_pressure: 0.10,
            carry: -0.04,
            geopolitical_risk: 0.22,
            valuation_gap: 0.04,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "HO=F",
            commodity: "Heating oil",
            commodity_class: "energy-refined",
            supply_tightness: 0.10,
            demand_growth: 0.06,
            inventory_pressure: 0.06,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: 0.02,
            volatility: 0.32,
        },
        CommodityCandidate {
            instrument_id: "LNG",
            commodity: "Liquefied natural gas",
            commodity_class: "energy",
            supply_tightness: 0.14,
            demand_growth: 0.28,
            inventory_pressure: 0.08,
            carry: -0.06,
            geopolitical_risk: 0.32,
            valuation_gap: 0.08,
            volatility: 0.46,
        },
        CommodityCandidate {
            instrument_id: "U3O8",
            commodity: "Uranium",
            commodity_class: "energy",
            supply_tightness: 0.46,
            demand_growth: 0.34,
            inventory_pressure: 0.30,
            carry: 0.02,
            geopolitical_risk: 0.28,
            valuation_gap: 0.22,
            volatility: 0.42,
        },
        CommodityCandidate {
            instrument_id: "THERMAL-COAL",
            commodity: "Thermal coal",
            commodity_class: "energy",
            supply_tightness: -0.08,
            demand_growth: -0.12,
            inventory_pressure: -0.04,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: -0.18,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "GC=F",
            commodity: "Gold",
            commodity_class: "precious-metals",
            supply_tightness: 0.10,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: -0.03,
            geopolitical_risk: 0.30,
            valuation_gap: 0.10,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "SI=F",
            commodity: "Silver",
            commodity_class: "precious-industrial-metals",
            supply_tightness: 0.20,
            demand_growth: 0.22,
            inventory_pressure: 0.16,
            carry: -0.03,
            geopolitical_risk: 0.22,
            valuation_gap: 0.06,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "PL=F",
            commodity: "Platinum",
            commodity_class: "precious-industrial-metals",
            supply_tightness: 0.18,
            demand_growth: 0.06,
            inventory_pressure: 0.12,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: -0.08,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "PA=F",
            commodity: "Palladium",
            commodity_class: "precious-industrial-metals",
            supply_tightness: -0.06,
            demand_growth: -0.12,
            inventory_pressure: -0.04,
            carry: -0.02,
            geopolitical_risk: 0.26,
            valuation_gap: -0.24,
            volatility: 0.40,
        },
        CommodityCandidate {
            instrument_id: "HG=F",
            commodity: "Copper",
            commodity_class: "industrial-metals",
            supply_tightness: 0.28,
            demand_growth: 0.30,
            inventory_pressure: 0.22,
            carry: -0.01,
            geopolitical_risk: 0.18,
            valuation_gap: 0.12,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "ALUMINUM",
            commodity: "Aluminum",
            commodity_class: "industrial-metals",
            supply_tightness: 0.06,
            demand_growth: 0.16,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.02,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "NICKEL",
            commodity: "Nickel",
            commodity_class: "battery-industrial-metals",
            supply_tightness: -0.18,
            demand_growth: 0.20,
            inventory_pressure: -0.16,
            carry: -0.02,
            geopolitical_risk: 0.22,
            valuation_gap: -0.18,
            volatility: 0.38,
        },
        CommodityCandidate {
            instrument_id: "ZINC",
            commodity: "Zinc",
            commodity_class: "industrial-metals",
            supply_tightness: 0.04,
            demand_growth: 0.10,
            inventory_pressure: 0.02,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: -0.05,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "LEAD",
            commodity: "Lead",
            commodity_class: "industrial-metals",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.02,
            carry: -0.01,
            geopolitical_risk: 0.10,
            valuation_gap: -0.08,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "TIN",
            commodity: "Tin",
            commodity_class: "industrial-metals",
            supply_tightness: 0.22,
            demand_growth: 0.14,
            inventory_pressure: 0.18,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: 0.06,
            volatility: 0.32,
        },
        CommodityCandidate {
            instrument_id: "IRON-ORE",
            commodity: "Iron ore",
            commodity_class: "bulk-industrial",
            supply_tightness: -0.04,
            demand_growth: 0.06,
            inventory_pressure: -0.05,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.10,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "STEEL-HRC",
            commodity: "Hot-rolled coil steel",
            commodity_class: "bulk-industrial",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.06,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "LITHIUM",
            commodity: "Lithium carbonate",
            commodity_class: "battery-metals",
            supply_tightness: -0.22,
            demand_growth: 0.34,
            inventory_pressure: -0.24,
            carry: -0.03,
            geopolitical_risk: 0.20,
            valuation_gap: -0.32,
            volatility: 0.52,
        },
        CommodityCandidate {
            instrument_id: "COBALT",
            commodity: "Cobalt",
            commodity_class: "battery-metals",
            supply_tightness: -0.10,
            demand_growth: 0.16,
            inventory_pressure: -0.08,
            carry: -0.03,
            geopolitical_risk: 0.38,
            valuation_gap: -0.18,
            volatility: 0.40,
        },
        CommodityCandidate {
            instrument_id: "GRAPHITE",
            commodity: "Graphite",
            commodity_class: "battery-metals",
            supply_tightness: 0.24,
            demand_growth: 0.32,
            inventory_pressure: 0.18,
            carry: -0.02,
            geopolitical_risk: 0.34,
            valuation_gap: 0.10,
            volatility: 0.36,
        },
        CommodityCandidate {
            instrument_id: "RARE-EARTHS",
            commodity: "Rare earth basket",
            commodity_class: "battery-industrial-metals",
            supply_tightness: 0.30,
            demand_growth: 0.26,
            inventory_pressure: 0.20,
            carry: -0.03,
            geopolitical_risk: 0.42,
            valuation_gap: 0.18,
            volatility: 0.44,
        },
        CommodityCandidate {
            instrument_id: "CORN",
            commodity: "Corn",
            commodity_class: "agriculture-food",
            supply_tightness: -0.04,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.10,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "WHEAT",
            commodity: "Wheat",
            commodity_class: "agriculture-food",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.30,
            valuation_gap: -0.04,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "SOYBEANS",
            commodity: "Soybeans",
            commodity_class: "agriculture-food",
            supply_tightness: 0.02,
            demand_growth: 0.10,
            inventory_pressure: 0.00,
            carry: -0.01,
            geopolitical_risk: 0.16,
            valuation_gap: -0.06,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "SOYMEAL",
            commodity: "Soybean meal",
            commodity_class: "agriculture-food",
            supply_tightness: 0.04,
            demand_growth: 0.12,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.04,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "SOYOIL",
            commodity: "Soybean oil",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.12,
            demand_growth: 0.16,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: 0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "RICE",
            commodity: "Rice",
            commodity_class: "agriculture-food",
            supply_tightness: 0.08,
            demand_growth: 0.06,
            inventory_pressure: 0.06,
            carry: 0.00,
            geopolitical_risk: 0.12,
            valuation_gap: 0.00,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "OATS",
            commodity: "Oats",
            commodity_class: "agriculture-food",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.03,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.08,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "COFFEE",
            commodity: "Coffee",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.22,
            demand_growth: 0.12,
            inventory_pressure: 0.16,
            carry: -0.02,
            geopolitical_risk: 0.16,
            valuation_gap: 0.12,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "COCOA",
            commodity: "Cocoa",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.42,
            demand_growth: 0.10,
            inventory_pressure: 0.36,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: 0.34,
            volatility: 0.50,
        },
        CommodityCandidate {
            instrument_id: "SUGAR",
            commodity: "Sugar",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.06,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: 0.00,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "COTTON",
            commodity: "Cotton",
            commodity_class: "agriculture-softs",
            supply_tightness: -0.06,
            demand_growth: 0.04,
            inventory_pressure: -0.04,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.12,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "ORANGE-JUICE",
            commodity: "Frozen concentrated orange juice",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.34,
            demand_growth: 0.02,
            inventory_pressure: 0.28,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: 0.30,
            volatility: 0.48,
        },
        CommodityCandidate {
            instrument_id: "LIVE-CATTLE",
            commodity: "Live cattle",
            commodity_class: "livestock-food",
            supply_tightness: 0.26,
            demand_growth: 0.08,
            inventory_pressure: 0.20,
            carry: 0.00,
            geopolitical_risk: 0.08,
            valuation_gap: 0.14,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "LEAN-HOGS",
            commodity: "Lean hogs",
            commodity_class: "livestock-food",
            supply_tightness: -0.04,
            demand_growth: 0.06,
            inventory_pressure: -0.04,
            carry: 0.00,
            geopolitical_risk: 0.06,
            valuation_gap: -0.08,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "LUMBER",
            commodity: "Lumber",
            commodity_class: "housing-industrial",
            supply_tightness: -0.12,
            demand_growth: 0.02,
            inventory_pressure: -0.10,
            carry: -0.02,
            geopolitical_risk: 0.08,
            valuation_gap: -0.18,
            volatility: 0.44,
        },
        CommodityCandidate {
            instrument_id: "RUBBER",
            commodity: "Rubber",
            commodity_class: "industrial-agriculture",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.10,
            valuation_gap: -0.04,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "PALM-OIL",
            commodity: "Palm oil",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.10,
            demand_growth: 0.14,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: 0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "CANOLA",
            commodity: "Canola",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.04,
            demand_growth: 0.10,
            inventory_pressure: 0.02,
            carry: -0.01,
            geopolitical_risk: 0.10,
            valuation_gap: -0.03,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "MILK",
            commodity: "Class III milk",
            commodity_class: "dairy-food",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.02,
            carry: 0.00,
            geopolitical_risk: 0.04,
            valuation_gap: -0.04,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "BUTTER",
            commodity: "Butter",
            commodity_class: "dairy-food",
            supply_tightness: 0.06,
            demand_growth: 0.04,
            inventory_pressure: 0.04,
            carry: 0.00,
            geopolitical_risk: 0.04,
            valuation_gap: 0.02,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "CARBON-EUA",
            commodity: "EU carbon allowances",
            commodity_class: "carbon",
            supply_tightness: 0.18,
            demand_growth: 0.18,
            inventory_pressure: 0.12,
            carry: 0.02,
            geopolitical_risk: 0.10,
            valuation_gap: 0.06,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "CARBON-CCA",
            commodity: "California carbon allowances",
            commodity_class: "carbon",
            supply_tightness: 0.10,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: 0.02,
            geopolitical_risk: 0.06,
            valuation_gap: 0.02,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "FREIGHT-BDI",
            commodity: "Dry bulk freight",
            commodity_class: "freight-industrial",
            supply_tightness: 0.06,
            demand_growth: 0.14,
            inventory_pressure: 0.02,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: -0.02,
            volatility: 0.46,
        },
        CommodityCandidate {
            instrument_id: "POTASH",
            commodity: "Potash",
            commodity_class: "fertilizer-agriculture",
            supply_tightness: 0.08,
            demand_growth: 0.10,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.24,
            valuation_gap: -0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "PHOSPHATE",
            commodity: "Phosphate",
            commodity_class: "fertilizer-agriculture",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.06,
            carry: -0.01,
            geopolitical_risk: 0.22,
            valuation_gap: 0.00,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "UREA",
            commodity: "Urea",
            commodity_class: "fertilizer-agriculture-energy",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.22,
            valuation_gap: -0.08,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "AMMONIA",
            commodity: "Ammonia",
            commodity_class: "fertilizer-energy",
            supply_tightness: 0.00,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.20,
            valuation_gap: -0.08,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "ETHANOL",
            commodity: "Ethanol",
            commodity_class: "energy-agriculture",
            supply_tightness: -0.04,
            demand_growth: 0.08,
            inventory_pressure: -0.04,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.10,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "METHANOL",
            commodity: "Methanol",
            commodity_class: "chemicals-energy",
            supply_tightness: -0.02,
            demand_growth: 0.10,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.10,
            valuation_gap: -0.08,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "POLYETHYLENE",
            commodity: "Polyethylene",
            commodity_class: "chemicals-industrial",
            supply_tightness: -0.10,
            demand_growth: 0.06,
            inventory_pressure: -0.08,
            carry: -0.02,
            geopolitical_risk: 0.08,
            valuation_gap: -0.12,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "PROPANE",
            commodity: "Propane",
            commodity_class: "energy",
            supply_tightness: 0.00,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.14,
            valuation_gap: -0.08,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "JET-FUEL",
            commodity: "Jet fuel",
            commodity_class: "energy-refined",
            supply_tightness: 0.08,
            demand_growth: 0.14,
            inventory_pressure: 0.04,
            carry: -0.03,
            geopolitical_risk: 0.16,
            valuation_gap: 0.00,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "NAPHTHA",
            commodity: "Naphtha",
            commodity_class: "energy-chemicals",
            supply_tightness: -0.02,
            demand_growth: 0.10,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.06,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "SUSTAINABLE-AVIATION-FUEL",
            commodity: "Sustainable aviation fuel credits",
            commodity_class: "energy-carbon",
            supply_tightness: 0.24,
            demand_growth: 0.34,
            inventory_pressure: 0.18,
            carry: 0.01,
            geopolitical_risk: 0.08,
            valuation_gap: 0.18,
            volatility: 0.38,
        },
        CommodityCandidate {
            instrument_id: "REC",
            commodity: "Renewable energy certificates",
            commodity_class: "carbon-energy",
            supply_tightness: 0.08,
            demand_growth: 0.16,
            inventory_pressure: 0.04,
            carry: 0.01,
            geopolitical_risk: 0.06,
            valuation_gap: 0.02,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "WATER-RIGHTS",
            commodity: "Water rights proxy",
            commodity_class: "scarcity-resource",
            supply_tightness: 0.28,
            demand_growth: 0.18,
            inventory_pressure: 0.20,
            carry: 0.00,
            geopolitical_risk: 0.10,
            valuation_gap: 0.16,
            volatility: 0.30,
        },
    ]
}
