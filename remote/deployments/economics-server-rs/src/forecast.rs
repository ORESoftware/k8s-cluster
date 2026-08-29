use crate::catalog::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn snapshot_series_or_sample(state: &AppState) -> Vec<MarketSeries> {
    let stored = state
        .series_store
        .read()
        .map(|store| store.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if stored.is_empty() {
        sample_market_series()
    } else {
        stored
    }
}

pub(crate) fn forecast_from_request(
    state: &AppState,
    mut request: ForecastRequest,
) -> Result<ForecastResponse, String> {
    if request.series.as_ref().map(Vec::is_empty).unwrap_or(true) {
        request.series = Some(snapshot_series_or_sample(state));
    }
    generate_forecast(&state.config, request)
}

pub(crate) fn generate_forecast(
    config: &Config,
    request: ForecastRequest,
) -> Result<ForecastResponse, String> {
    let request_id = request_id(request.request_id.as_ref(), "economics-forecast");
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let horizon_months = request
        .horizon_months
        .unwrap_or(config.projection_months)
        .clamp(1, 120);
    let confidence_level = clamp(
        request.confidence_level.unwrap_or(config.confidence_level),
        0.50,
        0.995,
    );
    let scenario = request
        .scenario
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    let series = request
        .series
        .ok_or_else(|| "series must be provided or previously ingested".to_string())?;
    validate_macro_context(request.macro_context.as_ref())?;
    validate_macro_fiscal_context(request.macro_fiscal_context.as_ref())?;
    validate_venture_capital_context(request.venture_capital_context.as_ref())?;
    validate_series(&series)?;
    let macro_context = request.macro_context.unwrap_or_default();
    let weights = normalize_weights(request.theory_weights.as_ref());
    let mut projections = Vec::with_capacity(series.len());
    let mut warnings = Vec::new();

    for item in &series {
        let stats = series_stats(item, config.history_years)?;
        let prior = theory_prior(item, &macro_context, &scenario);
        let scenario_adjustment = scenario_adjustment(&item.asset_class, &scenario);
        let drift = weights.data * stats.data_drift
            + weights.macro_theory * prior.drift
            + weights.momentum * stats.momentum
            + weights.mean_reversion * stats.mean_reversion
            + weights.carry * prior.carry
            + weights.valuation * prior.valuation
            - weights.jump_stress * prior.jump_stress
            + scenario_adjustment;
        let class_floor = class_volatility_floor(&item.asset_class);
        let annualized_volatility = stats
            .volatility_per_period
            .mul_add(stats.periods_per_year.sqrt(), 0.0)
            .max(class_floor);
        let points = forecast_points(
            stats.last_price,
            drift,
            annualized_volatility,
            horizon_months,
            confidence_level,
        );
        let terminal = points
            .last()
            .map(|point| point.expected)
            .unwrap_or(stats.last_price);
        let expected_return_18m = terminal / stats.last_price - 1.0;
        let signal = signal_for(expected_return_18m, annualized_volatility);
        let mut rationale = prior.rationale;
        if scenario != "base" {
            rationale.push(format!(
                "scenario `{scenario}` adjustment {:.2}%",
                scenario_adjustment * 100.0
            ));
        }
        let display_name = item
            .display_name
            .clone()
            .unwrap_or_else(|| item.instrument_id.clone());
        let currency = item.currency.clone().unwrap_or_else(|| "USD".to_string());
        projections.push(Projection {
            instrument_id: item.instrument_id.clone(),
            display_name,
            asset_class: item.asset_class.clone(),
            currency,
            last_price: round4(stats.last_price),
            annualized_drift: round6(drift),
            annualized_volatility: round6(annualized_volatility),
            expected_return_18m: round6(expected_return_18m),
            signal,
            rationale,
            components: vec![
                component(
                    "dataDrift",
                    stats.data_drift,
                    weights.data,
                    "mean(log returns) annualized",
                ),
                component(
                    "macroTheory",
                    prior.drift,
                    weights.macro_theory,
                    "CAPM/Taylor/Fisher/UIP/PPP/Hotelling prior by asset class",
                ),
                component(
                    "momentum",
                    stats.momentum,
                    weights.momentum,
                    "recent log return annualized",
                ),
                component(
                    "meanReversion",
                    stats.mean_reversion,
                    weights.mean_reversion,
                    "OU-style pull toward long-run log-price mean",
                ),
                component(
                    "carry",
                    prior.carry,
                    weights.carry,
                    "carry, convenience yield, storage, duration, or rate income",
                ),
                component(
                    "valuation",
                    prior.valuation,
                    weights.valuation,
                    "valuation gap and adoption/saturation pressure",
                ),
                component(
                    "jumpStress",
                    -prior.jump_stress,
                    weights.jump_stress,
                    "fat-tail stress haircut",
                ),
            ],
            points,
        });
    }

    if series
        .iter()
        .any(|item| item.source.as_deref() == Some("built-in-sample"))
    {
        warnings.push(
            "using built-in demonstration data; ingest or pull real API series for live analysis"
                .to_string(),
        );
    }

    Ok(ForecastResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        history_years: config.history_years,
        horizon_months,
        confidence_level,
        scenario,
        generated_at_ms: now_ms(),
        des_engine: des_surface_descriptor(),
        equations: equation_catalog(),
        projections,
        warnings,
    })
}

pub(crate) fn component(name: &str, value: f64, weight: f64, equation: &str) -> ModelComponent {
    ModelComponent {
        name: name.to_string(),
        value: round6(value),
        weight: round6(weight),
        equation: equation.to_string(),
    }
}

pub(crate) fn series_stats(series: &MarketSeries, history_years: u32) -> Result<SeriesStats, String> {
    let mut observations = series.observations.clone();
    observations.sort_by(|left, right| left.date.cmp(&right.date));
    let last_price = observations
        .last()
        .ok_or_else(|| "series has no observations".to_string())?
        .price;
    let mut returns = Vec::with_capacity(observations.len().saturating_sub(1));
    for pair in observations.windows(2) {
        let left = pair[0].price;
        let right = pair[1].price;
        if left <= 0.0 || right <= 0.0 {
            return Err(format!(
                "series {} contains non-positive prices",
                series.instrument_id
            ));
        }
        returns.push((right / left).ln());
    }
    let periods_per_year =
        ((returns.len() as f64) / f64::from(history_years.max(1))).clamp(4.0, 252.0);
    let mean_return = mean(&returns);
    let variance = if returns.len() > 1 {
        returns
            .iter()
            .map(|value| {
                let diff = value - mean_return;
                diff * diff
            })
            .sum::<f64>()
            / ((returns.len() - 1) as f64)
    } else {
        0.0
    };
    let recent_count = returns.len().min(periods_per_year.round() as usize).max(1);
    let recent_sum = returns
        .iter()
        .rev()
        .take(recent_count)
        .copied()
        .sum::<f64>();
    let momentum = recent_sum * periods_per_year / recent_count as f64;
    let mean_log_price = mean(
        &observations
            .iter()
            .map(|point| point.price.ln())
            .collect::<Vec<_>>(),
    );
    let mean_reversion = (mean_log_price - last_price.ln()) * 0.25;
    Ok(SeriesStats {
        last_price,
        volatility_per_period: variance.sqrt(),
        periods_per_year,
        data_drift: mean_return * periods_per_year,
        momentum,
        mean_reversion,
    })
}

pub(crate) fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

pub(crate) fn normalize_weights(input: Option<&TheoryWeights>) -> NormalizedWeights {
    let raw = [
        input.and_then(|w| w.data).unwrap_or(0.42).max(0.0),
        input.and_then(|w| w.macro_theory).unwrap_or(0.28).max(0.0),
        input.and_then(|w| w.momentum).unwrap_or(0.14).max(0.0),
        input
            .and_then(|w| w.mean_reversion)
            .unwrap_or(0.08)
            .max(0.0),
        input.and_then(|w| w.carry).unwrap_or(0.04).max(0.0),
        input.and_then(|w| w.valuation).unwrap_or(0.03).max(0.0),
        input.and_then(|w| w.jump_stress).unwrap_or(0.01).max(0.0),
    ];
    let sum = raw.iter().sum::<f64>().max(f64::EPSILON);
    NormalizedWeights {
        data: raw[0] / sum,
        macro_theory: raw[1] / sum,
        momentum: raw[2] / sum,
        mean_reversion: raw[3] / sum,
        carry: raw[4] / sum,
        valuation: raw[5] / sum,
        jump_stress: raw[6] / sum,
    }
}

pub(crate) fn theory_prior(
    series: &MarketSeries,
    macro_context: &MacroContext,
    scenario: &str,
) -> TheoryPrior {
    let asset_class = series.asset_class.to_ascii_lowercase();
    let features = series.features.clone().unwrap_or_default();
    let policy_rate = macro_context.policy_rate.unwrap_or(0.045);
    let foreign_policy_rate = macro_context.foreign_policy_rate.unwrap_or(0.025);
    let inflation = macro_context.inflation.unwrap_or(0.030);
    let foreign_inflation = macro_context.foreign_inflation.unwrap_or(0.020);
    let expected_inflation = macro_context.expected_inflation.unwrap_or(inflation);
    let money_growth = macro_context.money_supply_growth.unwrap_or(0.045);
    let real_growth = macro_context.real_growth.unwrap_or(0.020);
    let output_gap = macro_context.output_gap.unwrap_or(0.0);
    let unemployment_gap = macro_context.unemployment_gap.unwrap_or(0.0);
    let risk_free_rate = macro_context.risk_free_rate.unwrap_or(policy_rate);
    let market_return = macro_context.market_return.unwrap_or(0.080);
    let neutral_real_rate = 0.010;
    let inflation_target = 0.020;
    let taylor_rate =
        neutral_real_rate + inflation + 0.5 * (inflation - inflation_target) + 0.5 * output_gap;
    let policy_tightness = policy_rate - taylor_rate;
    let real_rate = policy_rate - expected_inflation;
    let liquidity_impulse = (money_growth - real_growth).clamp(-0.15, 0.20);
    let phillips_pressure = (-0.4 * unemployment_gap).clamp(-0.05, 0.05);
    let beta = features.beta.unwrap_or_else(|| default_beta(&asset_class));
    let mut rationale = vec![
        format!("Taylor tightness {:.2}%", policy_tightness * 100.0),
        format!("Fisher real-rate proxy {:.2}%", real_rate * 100.0),
        format!("liquidity impulse {:.2}%", liquidity_impulse * 100.0),
    ];

    let supply_growth = features.supply_growth.unwrap_or(0.020);
    let demand_growth = features.demand_growth.unwrap_or(real_growth + output_gap);
    let storage_cost = features.storage_cost.unwrap_or(0.015);
    let convenience_yield = features.convenience_yield.unwrap_or(0.010);
    let carry = features.carry.unwrap_or(0.0);
    let duration = features.duration.unwrap_or(6.0);
    let valuation_gap = features.valuation_gap.unwrap_or(0.0);

    let (drift, carry_component, valuation_component, jump_stress) = if asset_class
        .contains("equity")
        || asset_class.contains("security")
        || asset_class.contains("index")
        || asset_class.contains("etf")
    {
        let capm = risk_free_rate + beta * (market_return - risk_free_rate);
        rationale.push(format!("CAPM prior {:.2}%", capm * 100.0));
        (
            capm - 0.8 * policy_tightness + 0.2 * liquidity_impulse + 0.2 * output_gap,
            carry,
            -0.25 * valuation_gap,
            0.10,
        )
    } else if asset_class.contains("bond") || asset_class.contains("treasury") {
        let rate_shock = policy_tightness + 0.35 * (inflation - expected_inflation);
        rationale.push(format!("duration {:.1} years", duration));
        (
            risk_free_rate - duration * rate_shock * 0.25,
            risk_free_rate,
            -0.15 * valuation_gap,
            0.04,
        )
    } else if asset_class.contains("money") || asset_class.contains("cash") {
        (policy_rate, policy_rate, 0.0, 0.01)
    } else if asset_class.contains("fx")
        || asset_class.contains("forex")
        || asset_class.contains("currency")
    {
        let uip = policy_rate - foreign_policy_rate;
        let ppp = inflation - foreign_inflation;
        rationale.push(format!(
            "UIP {:.2}% and PPP {:.2}%",
            uip * 100.0,
            ppp * 100.0
        ));
        (
            0.6 * uip + 0.4 * ppp,
            carry + uip,
            -0.1 * valuation_gap,
            0.06,
        )
    } else if asset_class.contains("gold")
        || asset_class.contains("silver")
        || asset_class.contains("precious")
    {
        let inflation_surprise = inflation - expected_inflation;
        (
            -1.2 * real_rate + 0.8 * inflation_surprise + 0.3 * liquidity_impulse,
            carry - storage_cost + convenience_yield,
            -0.20 * valuation_gap,
            0.09,
        )
    } else if asset_class.contains("crypto") {
        (
            0.10 + 1.6 * liquidity_impulse - 1.1 * real_rate + 0.3 * output_gap,
            carry,
            -0.35 * valuation_gap,
            0.25,
        )
    } else if asset_class.contains("real-estate")
        || asset_class.contains("housing")
        || asset_class.contains("property")
    {
        let rent_growth = inflation + real_growth + output_gap;
        (
            rent_growth - 2.4 * real_rate + 0.4 * liquidity_impulse,
            carry,
            -0.30 * valuation_gap,
            0.08,
        )
    } else if asset_class.contains("oil")
        || asset_class.contains("energy")
        || asset_class.contains("commodity")
        || asset_class.contains("metal")
    {
        let elasticity_pressure = (demand_growth - supply_growth) / 0.7;
        let hotelling = policy_rate + storage_cost - convenience_yield + elasticity_pressure;
        rationale.push(format!("Hotelling/carry prior {:.2}%", hotelling * 100.0));
        (
            hotelling + 0.35 * (inflation - inflation_target) + phillips_pressure,
            carry - storage_cost + convenience_yield,
            -0.15 * valuation_gap,
            0.14,
        )
    } else {
        (
            risk_free_rate
                + beta * (market_return - risk_free_rate) * 0.5
                + 0.2 * liquidity_impulse,
            carry,
            -0.20 * valuation_gap,
            0.08,
        )
    };

    let scenario_risk = if matches!(scenario, "liquidity-crunch" | "oil-shock" | "deflation") {
        jump_stress * 1.5
    } else {
        jump_stress
    };

    TheoryPrior {
        drift: clamp(drift, -0.80, 0.80),
        carry: clamp(carry_component, -0.40, 0.40),
        valuation: clamp(valuation_component, -0.40, 0.40),
        jump_stress: scenario_risk,
        rationale,
    }
}

pub(crate) fn default_beta(asset_class: &str) -> f64 {
    if asset_class.contains("crypto") {
        1.8
    } else if asset_class.contains("equity") || asset_class.contains("security") {
        1.0
    } else if asset_class.contains("real-estate") {
        0.7
    } else if asset_class.contains("commodity") || asset_class.contains("oil") {
        0.6
    } else if asset_class.contains("bond") || asset_class.contains("money") {
        0.1
    } else {
        0.5
    }
}

pub(crate) fn class_volatility_floor(asset_class: &str) -> f64 {
    let lower = asset_class.to_ascii_lowercase();
    if lower.contains("crypto") {
        0.55
    } else if lower.contains("oil") || lower.contains("energy") {
        0.32
    } else if lower.contains("equity") || lower.contains("security") || lower.contains("index") {
        0.18
    } else if lower.contains("gold") || lower.contains("silver") || lower.contains("commodity") {
        0.20
    } else if lower.contains("fx") || lower.contains("forex") || lower.contains("currency") {
        0.09
    } else if lower.contains("bond") || lower.contains("treasury") {
        0.08
    } else if lower.contains("real-estate") || lower.contains("housing") {
        0.10
    } else {
        0.12
    }
}

pub(crate) fn scenario_adjustment(asset_class: &str, scenario: &str) -> f64 {
    let lower = asset_class.to_ascii_lowercase();
    match scenario {
        "oil-shock" => {
            if lower.contains("oil") || lower.contains("energy") {
                0.22
            } else if lower.contains("equity") || lower.contains("real-estate") {
                -0.07
            } else if lower.contains("gold") || lower.contains("silver") {
                0.06
            } else {
                -0.02
            }
        }
        "liquidity-crunch" => {
            if lower.contains("crypto") {
                -0.28
            } else if lower.contains("equity") || lower.contains("real-estate") {
                -0.16
            } else if lower.contains("bond") || lower.contains("treasury") {
                0.04
            } else if lower.contains("gold") {
                0.05
            } else {
                -0.05
            }
        }
        "dollar-strength" => {
            if lower.contains("fx") || lower.contains("currency") {
                0.08
            } else if lower.contains("gold")
                || lower.contains("silver")
                || lower.contains("commodity")
            {
                -0.06
            } else {
                -0.02
            }
        }
        "deflation" => {
            if lower.contains("bond") || lower.contains("treasury") {
                0.08
            } else if lower.contains("money") {
                0.02
            } else {
                -0.10
            }
        }
        "soft-landing" => {
            if lower.contains("equity") || lower.contains("real-estate") {
                0.06
            } else if lower.contains("crypto") {
                0.08
            } else {
                0.02
            }
        }
        _ => 0.0,
    }
}

pub(crate) fn forecast_points(
    last_price: f64,
    drift: f64,
    annualized_volatility: f64,
    horizon_months: u32,
    confidence_level: f64,
) -> Vec<ForecastPoint> {
    let z = z_score(confidence_level);
    (1..=horizon_months)
        .map(|month| {
            let t = f64::from(month) / 12.0;
            let expected = last_price * (drift * t).exp();
            let center = (drift - 0.5 * annualized_volatility * annualized_volatility) * t;
            let width = z * annualized_volatility * t.sqrt();
            ForecastPoint {
                month,
                label: format!("M+{month}"),
                expected: round4(expected),
                lower: round4((last_price * (center - width).exp()).max(0.0)),
                upper: round4(last_price * (center + width).exp()),
            }
        })
        .collect()
}

pub(crate) fn z_score(confidence_level: f64) -> f64 {
    if confidence_level >= 0.99 {
        2.576
    } else if confidence_level >= 0.975 {
        2.241
    } else if confidence_level >= 0.95 {
        1.960
    } else if confidence_level >= 0.90 {
        1.645
    } else if confidence_level >= 0.80 {
        1.282
    } else {
        1.000
    }
}

pub(crate) fn signal_for(expected_return: f64, annualized_volatility: f64) -> String {
    let risk_adjusted = expected_return / annualized_volatility.max(0.05);
    if risk_adjusted > 0.65 {
        "accumulate".to_string()
    } else if risk_adjusted > 0.20 {
        "watch-uptrend".to_string()
    } else if risk_adjusted < -0.45 {
        "reduce-or-hedge".to_string()
    } else {
        "neutral".to_string()
    }
}

pub(crate) fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

pub(crate) fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

pub(crate) fn sample_market_series() -> Vec<MarketSeries> {
    vec![
        synthetic_series("CL=F", "WTI crude oil", "oil", 71.0, 0.015, 0.30, 0.0),
        synthetic_series("GC=F", "Gold spot proxy", "gold", 2050.0, 0.055, 0.18, 1.1),
        synthetic_series(
            "SI=F",
            "Silver spot proxy",
            "silver",
            25.0,
            0.045,
            0.28,
            2.2,
        ),
        synthetic_series("BTC-USD", "Bitcoin", "crypto", 65_000.0, 0.13, 0.65, 3.0),
        synthetic_series("SPY", "US equities", "equities", 520.0, 0.075, 0.18, 4.1),
        synthetic_series(
            "UST10Y",
            "10Y treasury price proxy",
            "bonds",
            98.0,
            0.025,
            0.08,
            5.0,
        ),
        synthetic_series(
            "USD-EUR",
            "USD/EUR FX proxy",
            "forex",
            0.92,
            0.005,
            0.09,
            5.8,
        ),
        synthetic_series(
            "CSHPI",
            "US home price proxy",
            "real-estate",
            310.0,
            0.045,
            0.10,
            6.4,
        ),
        synthetic_series(
            "CORN",
            "Corn commodity proxy",
            "commodity",
            460.0,
            0.020,
            0.24,
            7.0,
        ),
    ]
}

pub(crate) fn synthetic_series(
    instrument_id: &str,
    display_name: &str,
    asset_class: &str,
    terminal_price: f64,
    annual_drift: f64,
    annual_volatility: f64,
    phase: f64,
) -> MarketSeries {
    let months = (DEFAULT_HISTORY_YEARS * 12) as usize;
    let monthly_drift = annual_drift / 12.0;
    let monthly_vol = annual_volatility / 12.0_f64.sqrt();
    let mut log_price = terminal_price.ln() - monthly_drift * months as f64;
    let mut observations = Vec::with_capacity(months + 1);
    for idx in 0..=months {
        let cycle = ((idx as f64 / 9.0) + phase).sin() * monthly_vol * 0.7;
        let regime = ((idx as f64 / 37.0) + phase).cos() * monthly_vol * 0.35;
        if idx > 0 {
            log_price += monthly_drift + cycle + regime;
        }
        observations.push(MarketObservation {
            date: format!("T-{:03}M", months - idx),
            price: round4(log_price.exp()),
            volume: Some(round4(
                1_000_000.0 * (1.0 + ((idx as f64 / 5.0) + phase).sin() * 0.25),
            )),
        });
    }
    let observed_terminal = observations
        .last()
        .map(|point| point.price)
        .unwrap_or(terminal_price);
    let scale = terminal_price / observed_terminal;
    for point in &mut observations {
        point.price = round4(point.price * scale);
    }
    MarketSeries {
        instrument_id: instrument_id.to_string(),
        display_name: Some(display_name.to_string()),
        asset_class: asset_class.to_string(),
        currency: Some("USD".to_string()),
        source: Some("built-in-sample".to_string()),
        observations,
        features: Some(default_features(asset_class)),
    }
}

pub(crate) fn default_features(asset_class: &str) -> AssetFeatures {
    let lower = asset_class.to_ascii_lowercase();
    AssetFeatures {
        beta: Some(default_beta(&lower)),
        duration: if lower.contains("bond") {
            Some(7.0)
        } else {
            None
        },
        carry: if lower.contains("money") {
            Some(0.045)
        } else {
            Some(0.0)
        },
        convenience_yield: if lower.contains("oil") || lower.contains("commodity") {
            Some(0.018)
        } else {
            None
        },
        storage_cost: if lower.contains("oil") || lower.contains("commodity") {
            Some(0.020)
        } else {
            None
        },
        supply_growth: Some(0.020),
        demand_growth: Some(0.026),
        inventory_ratio: None,
        valuation_gap: Some(0.0),
    }
}
