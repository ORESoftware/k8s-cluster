use std::{net::IpAddr, sync::atomic::Ordering};

use serde_json::{json, Value};

use crate::catalog::*;
use crate::forecast::*;
use crate::nats::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) async fn pull_source(state: &AppState, request: ApiPullRequest) -> Result<ApiPullResponse, String> {
    let mut request = request;
    let source_template = apply_public_source_template(&mut request)?;
    validate_api_pull_request(&request, source_template.as_ref())?;
    let url = request.url.as_deref().ok_or_else(|| {
        "url is required unless sourceId names a public source template".to_string()
    })?;
    let parsed_url =
        reqwest::Url::parse(url.trim()).map_err(|error| format!("url is invalid: {error}"))?;
    if let Some(template) = source_template.as_ref() {
        validate_public_source_url(&parsed_url, template)?;
    } else {
        validate_source_url_for_config(&parsed_url, &state.config)?;
    }
    let mut http_request = state.http.get(parsed_url.clone());
    if let Some(env_name) = request.auth_header_env.as_deref() {
        let env_name = validate_source_auth_env(&state.config, env_name)?;
        let header_value = optional_env(&env_name)
            .ok_or_else(|| format!("auth header env var {env_name} is not configured"))?;
        let header_name = validate_source_auth_header_name(
            request
                .auth_header_name
                .as_deref()
                .unwrap_or("authorization"),
        )?;
        let header_value = reqwest::header::HeaderValue::from_str(&header_value)
            .map_err(|_| "auth header value contains invalid bytes".to_string())?;
        http_request = http_request.header(header_name, header_value);
    }
    let response = http_request
        .send()
        .await
        .map_err(|error| format!("source fetch failed: {error}"))?;
    let status = response.status();
    if let Some(len) = response.content_length() {
        if len as usize > MAX_SOURCE_FETCH_BYTES {
            return Err(format!(
                "source response is too large: {len} bytes, max {MAX_SOURCE_FETCH_BYTES}"
            ));
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("source body read failed: {error}"))?;
    if bytes.len() > MAX_SOURCE_FETCH_BYTES {
        return Err(format!(
            "source response is too large: {} bytes, max {MAX_SOURCE_FETCH_BYTES}",
            bytes.len()
        ));
    }
    if !status.is_success() {
        return Err(format!("source returned HTTP {status}"));
    }
    let request_id = request_id(request.request_id.as_ref(), "source-pull");
    let mut stored_points = 0usize;
    let mut warnings = Vec::new();
    let mut instrument_id = request.instrument_id.clone();
    let mut quality = None;
    let parser = request.parser;
    let should_parse = parser.is_some()
        || (request.instrument_id.is_some()
            && request.asset_class.is_some()
            && request.date_field.is_some()
            && request.price_field.is_some());
    if should_parse {
        let (series, report) = series_from_bytes(&request, &bytes)?;
        stored_points = series.observations.len();
        instrument_id = Some(series.instrument_id.clone());
        validate_series(std::slice::from_ref(&series))?;
        quality = Some(report);
        let mut store = state
            .series_store
            .write()
            .map_err(|_| "series store lock poisoned".to_string())?;
        store.insert(series.instrument_id.clone(), series);
    } else {
        warnings.push(
            "response fetched but not stored; provide sourceId or instrumentId, assetClass, parser, and field/index metadata to parse a series"
                .to_string(),
        );
    }
    let host = parsed_url.host_str().unwrap_or("unknown").to_string();
    let response = ApiPullResponse {
        ok: true,
        request_id,
        source_id: request.source_id.clone(),
        source: request
            .source
            .unwrap_or_else(|| "ad-hoc-api".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        parser,
        url_host: host,
        http_status: status.as_u16(),
        bytes: bytes.len(),
        stored_points,
        instrument_id,
        quality,
        warnings,
        fetched_at_ms: now_ms(),
    };
    state
        .metrics
        .source_pull_success_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .source_pull_bytes_total
        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
    state
        .metrics
        .source_pull_stored_points_total
        .fetch_add(stored_points as u64, Ordering::Relaxed);
    state
        .metrics
        .source_pull_last_success_unix_seconds
        .store(now_unix_seconds(), Ordering::Relaxed);
    emit_log(
        "INFO",
        "economics.source_pull.ok",
        "economics source pull completed",
        json!({
            "requestId": &response.request_id,
            "sourceId": &response.source_id,
            "source": &response.source,
            "urlHost": &response.url_host,
            "httpStatus": response.http_status,
            "bytes": response.bytes,
            "storedPoints": response.stored_points,
            "instrumentId": &response.instrument_id,
            "parser": &response.parser
        }),
    );
    publish_market_event(
        state,
        json!({
            "type": "economics.source_pull",
            "source": SERVICE_NAME,
            "requestId": response.request_id,
            "urlHost": response.url_host,
            "storedPoints": response.stored_points,
            "instrumentId": response.instrument_id,
            "atMs": response.fetched_at_ms
        }),
    )
    .await;
    Ok(response)
}

pub(crate) fn apply_public_source_template(
    request: &mut ApiPullRequest,
) -> Result<Option<PublicSourceTemplate>, String> {
    let Some(source_id) = request.source_id.as_deref() else {
        return Ok(None);
    };
    let source_id = clean_token(source_id, "sourceId")?;
    let template = public_source_template(&source_id).ok_or_else(|| {
        format!(
            "unknown sourceId {source_id}; use GET /sources/public for supported public templates"
        )
    })?;
    if request
        .url
        .as_deref()
        .map(|url| !url.trim().is_empty())
        .unwrap_or(false)
    {
        return Err("sourceId templates do not allow url overrides".to_string());
    }
    if request.auth_header_env.is_some() || request.auth_header_name.is_some() {
        return Err(
            "sourceId templates are public and do not accept auth header overrides".to_string(),
        );
    }

    request.source_id = Some(source_id);
    request.url = Some(template.url.to_string());
    request.parser.get_or_insert(template.parser);
    request
        .instrument_id
        .get_or_insert_with(|| template.instrument_id.to_string());
    request
        .display_name
        .get_or_insert_with(|| template.display_name.to_string());
    request
        .asset_class
        .get_or_insert_with(|| template.asset_class.to_string());
    request
        .currency
        .get_or_insert_with(|| template.currency.to_string());
    request
        .source
        .get_or_insert_with(|| template.source.to_string());
    if request.root_pointer.is_none() {
        request.root_pointer = template.root_pointer.map(str::to_string);
    }
    if request.date_field.is_none() {
        request.date_field = template.date_field.map(str::to_string);
    }
    if request.price_field.is_none() {
        request.price_field = template.price_field.map(str::to_string);
    }
    if request.volume_field.is_none() {
        request.volume_field = template.volume_field.map(str::to_string);
    }
    request.date_index = request.date_index.or(template.date_index);
    request.price_index = request.price_index.or(template.price_index);
    request.volume_index = request.volume_index.or(template.volume_index);
    Ok(Some(template))
}

pub(crate) fn validate_api_pull_request(
    request: &ApiPullRequest,
    source_template: Option<&PublicSourceTemplate>,
) -> Result<(), String> {
    clean_optional_token(&request.source_id, "sourceId")?;
    clean_optional_token(&request.instrument_id, "instrumentId")?;
    clean_optional_token(&request.display_name, "displayName")?;
    clean_optional_token(&request.asset_class, "assetClass")?;
    clean_optional_token(&request.currency, "currency")?;
    clean_optional_token(&request.source, "source")?;
    clean_optional_token(&request.date_field, "dateField")?;
    clean_optional_token(&request.price_field, "priceField")?;
    clean_optional_token(&request.volume_field, "volumeField")?;
    clean_optional_token(&request.auth_header_env, "authHeaderEnv")?;
    clean_optional_token(&request.auth_header_name, "authHeaderName")?;
    if let Some(url) = request.url.as_deref() {
        if url.trim().is_empty() || url.len() > MAX_URL_LEN || url.chars().any(char::is_control) {
            return Err(format!(
                "url must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
            ));
        }
    }
    if let Some(pointer) = request.root_pointer.as_deref() {
        validate_json_pointer(pointer, "rootPointer")?;
    }
    for (label, index) in [
        ("dateIndex", request.date_index),
        ("priceIndex", request.price_index),
        ("volumeIndex", request.volume_index),
    ] {
        if let Some(index) = index {
            if index > 16 {
                return Err(format!("{label} must be between 0 and 16"));
            }
        }
    }
    if source_template.is_none() && request.source_id.is_some() {
        return Err("sourceId did not resolve to a public source template".to_string());
    }
    Ok(())
}

pub(crate) fn validate_json_pointer(pointer: &str, label: &str) -> Result<(), String> {
    let trimmed = pointer.trim();
    if trimmed.len() > MAX_JSON_POINTER_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "{label} must contain no control characters and be at most {MAX_JSON_POINTER_LEN} bytes"
        ));
    }
    if !trimmed.is_empty() && !trimmed.starts_with('/') {
        return Err(format!("{label} must be a JSON pointer starting with /"));
    }
    Ok(())
}

pub(crate) fn validate_public_source_url(
    url: &reqwest::Url,
    template: &PublicSourceTemplate,
) -> Result<(), String> {
    validate_source_url(url, false)?;
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if host != template.host {
        return Err(format!(
            "sourceId {} must resolve to host {}",
            template.id, template.host
        ));
    }
    Ok(())
}

pub(crate) fn validate_source_url_for_config(url: &reqwest::Url, config: &Config) -> Result<(), String> {
    validate_source_url(url, config.allow_private_source_urls)?;
    validate_source_host_allowlist(url, &config.allowed_source_hosts)
}

pub(crate) fn validate_source_url(url: &reqwest::Url, allow_private: bool) -> Result<(), String> {
    if url.as_str().len() > MAX_URL_LEN || url.as_str().chars().any(char::is_control) {
        return Err(format!(
            "source URL must contain no control characters and be at most {MAX_URL_LEN} bytes"
        ));
    }
    if url.fragment().is_some() {
        return Err("source URL fragments are not allowed".to_string());
    }
    match url.scheme() {
        "https" => {}
        "http" if allow_private => {}
        "http" => {
            return Err(
                "http source URLs require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
            );
        }
        other => return Err(format!("unsupported source URL scheme {other}")),
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if source_host_is_private(&host) && !allow_private {
        return Err(
            "private source hosts require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
        );
    }
    if url.port().is_some() && !allow_private {
        return Err(
            "custom source URL ports require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
        );
    }
    if url.username() != "" || url.password().is_some() {
        return Err("source URL credentials are not allowed".to_string());
    }
    Ok(())
}

pub(crate) fn source_host_is_private(host: &str) -> bool {
    if matches!(
        host,
        "localhost" | "host.docker.internal" | "metadata.google.internal"
    ) || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, _, _] = ip.octets();
            a == 0
                || a == 10
                || a == 127
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || a >= 224
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

pub(crate) fn validate_source_host_allowlist(
    url: &reqwest::Url,
    allowed_hosts: &[String],
) -> Result<(), String> {
    if allowed_hosts.is_empty() {
        return Ok(());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if allowed_hosts
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Ok(());
    }
    Err(format!(
        "source host {host} is not in ECONOMICS_ALLOWED_SOURCE_HOSTS"
    ))
}

pub(crate) fn series_from_bytes(
    request: &ApiPullRequest,
    bytes: &[u8],
) -> Result<(MarketSeries, SourceQualityReport), String> {
    match request.parser.unwrap_or(SourceParser::JsonRecords) {
        SourceParser::JsonRecords => {
            let json_value = serde_json::from_slice::<Value>(bytes)
                .map_err(|error| format!("source response is not JSON: {error}"))?;
            series_from_json_records_with_quality(request, &json_value)
        }
        SourceParser::JsonTupleArray => {
            let json_value = serde_json::from_slice::<Value>(bytes)
                .map_err(|error| format!("source response is not JSON: {error}"))?;
            series_from_json_tuple_array(request, &json_value)
        }
        SourceParser::CsvRecords => {
            let text = std::str::from_utf8(bytes)
                .map_err(|error| format!("source response is not UTF-8 CSV: {error}"))?;
            series_from_csv_records(request, text)
        }
    }
}

#[cfg(test)]
pub(crate) fn series_from_json(request: &ApiPullRequest, value: &Value) -> Result<MarketSeries, String> {
    series_from_json_records_with_quality(request, value).map(|(series, _)| series)
}

pub(crate) fn series_from_json_records_with_quality(
    request: &ApiPullRequest,
    value: &Value,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let root = match request.root_pointer.as_deref() {
        Some(pointer) if !pointer.trim().is_empty() => value
            .pointer(pointer)
            .ok_or_else(|| format!("rootPointer {pointer} did not match JSON response"))?,
        _ => value,
    };
    let items = root
        .as_array()
        .ok_or_else(|| "selected JSON value must be an array".to_string())?;
    let date_field = request.date_field.as_deref().unwrap_or("date");
    let price_field = request.price_field.as_deref().unwrap_or("price");
    let volume_field = request.volume_field.as_deref();
    let mut observations = Vec::with_capacity(items.len().min(MAX_OBSERVATIONS_PER_SERIES));
    let mut dropped_points = 0usize;
    for item in items.iter().take(MAX_OBSERVATIONS_PER_SERIES) {
        let Some(date) = field_value(item, date_field).and_then(date_from_value) else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = field_value(item, price_field).and_then(number_from_value) else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_field
            .and_then(|field| field_value(item, field))
            .and_then(number_from_value);
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::JsonRecords,
        observations,
        dropped_points,
    )
}

pub(crate) fn series_from_json_tuple_array(
    request: &ApiPullRequest,
    value: &Value,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let root = match request.root_pointer.as_deref() {
        Some(pointer) if !pointer.trim().is_empty() => value
            .pointer(pointer)
            .ok_or_else(|| format!("rootPointer {pointer} did not match JSON response"))?,
        _ => value,
    };
    let items = root
        .as_array()
        .ok_or_else(|| "selected JSON value must be an array".to_string())?;
    let date_index = request.date_index.unwrap_or(0);
    let price_index = request.price_index.unwrap_or(1);
    let volume_index = request.volume_index;
    let mut observations = Vec::with_capacity(items.len().min(MAX_OBSERVATIONS_PER_SERIES));
    let mut dropped_points = 0usize;
    for item in items.iter().take(MAX_OBSERVATIONS_PER_SERIES) {
        let Some(tuple) = item.as_array() else {
            dropped_points += 1;
            continue;
        };
        let Some(date) = tuple.get(date_index).and_then(date_from_value) else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = tuple.get(price_index).and_then(number_from_value) else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_index
            .and_then(|index| tuple.get(index))
            .and_then(number_from_value);
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::JsonTupleArray,
        observations,
        dropped_points,
    )
}

pub(crate) fn series_from_csv_records(
    request: &ApiPullRequest,
    text: &str,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| "CSV response must include a header row".to_string())?;
    let headers = parse_csv_line(header_line)?;
    let date_field = request.date_field.as_deref().unwrap_or("date");
    let price_field = request.price_field.as_deref().unwrap_or("price");
    let volume_field = request.volume_field.as_deref();
    let date_index = csv_header_index(&headers, date_field)?;
    let price_index = csv_header_index(&headers, price_field)?;
    let volume_index = volume_field
        .map(|field| csv_header_index(&headers, field))
        .transpose()?;
    let mut observations = Vec::with_capacity(MAX_OBSERVATIONS_PER_SERIES.min(1024));
    let mut dropped_points = 0usize;
    for line in lines.take(MAX_OBSERVATIONS_PER_SERIES) {
        let fields = parse_csv_line(line)?;
        let Some(date) = fields
            .get(date_index)
            .and_then(|value| date_from_text(value))
        else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = fields
            .get(price_index)
            .and_then(|value| number_from_text(value))
        else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_index
            .and_then(|index| fields.get(index))
            .and_then(|value| number_from_text(value));
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::CsvRecords,
        observations,
        dropped_points,
    )
}

pub(crate) fn build_series_with_quality(
    request: &ApiPullRequest,
    parser: SourceParser,
    mut observations: Vec<MarketObservation>,
    dropped_points: usize,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    observations.sort_by(|left, right| left.date.cmp(&right.date));
    let before_dedupe = observations.len();
    observations.dedup_by(|left, right| left.date == right.date);
    let dropped_points = dropped_points + before_dedupe.saturating_sub(observations.len());
    let quality = source_quality_report(parser, &observations, dropped_points);
    let series = MarketSeries {
        instrument_id: request
            .instrument_id
            .clone()
            .ok_or_else(|| "instrumentId is required to store parsed source data".to_string())?,
        display_name: request.display_name.clone(),
        asset_class: request
            .asset_class
            .clone()
            .ok_or_else(|| "assetClass is required to store parsed source data".to_string())?,
        currency: request.currency.clone(),
        source: request
            .source
            .clone()
            .or_else(|| Some("api-pull".to_string())),
        observations,
        features: None,
    };
    Ok((series, quality))
}

pub(crate) fn source_quality_report(
    parser: SourceParser,
    observations: &[MarketObservation],
    dropped_points: usize,
) -> SourceQualityReport {
    let min_price = observations
        .iter()
        .map(|point| point.price)
        .reduce(f64::min)
        .map(round6);
    let max_price = observations
        .iter()
        .map(|point| point.price)
        .reduce(f64::max)
        .map(round6);
    SourceQualityReport {
        parser,
        observed_points: observations.len(),
        dropped_points,
        first_date: observations.first().map(|point| point.date.clone()),
        last_date: observations.last().map(|point| point.date.clone()),
        min_price,
        max_price,
    }
}

pub(crate) fn parse_csv_line(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(field.trim().to_string());
                field.clear();
            }
            _ => field.push(ch),
        }
    }
    if in_quotes {
        return Err("CSV row has an unterminated quoted field".to_string());
    }
    fields.push(field.trim().to_string());
    Ok(fields)
}

pub(crate) fn csv_header_index(headers: &[String], field: &str) -> Result<usize, String> {
    headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case(field))
        .ok_or_else(|| format!("CSV field {field} was not found in header row"))
}

pub(crate) fn field_value<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    if field.starts_with('/') {
        value.pointer(field)
    } else {
        value.get(field)
    }
}

pub(crate) fn date_from_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(date_from_text)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
        .or_else(|| value.as_u64().map(|number| number.to_string()))
        .or_else(|| {
            value.as_f64().and_then(|number| {
                if number.is_finite() {
                    Some(format!("{number:.0}"))
                } else {
                    None
                }
            })
        })
}

pub(crate) fn date_from_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.chars().take(MAX_TOKEN_LEN).collect())
    }
}

pub(crate) fn number_from_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(number_from_text))
        .filter(|number| number.is_finite())
}

pub(crate) fn number_from_text(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed.eq_ignore_ascii_case("null")
        || trimmed.eq_ignore_ascii_case("nan")
    {
        return None;
    }
    let normalized = trimmed
        .trim_start_matches('$')
        .chars()
        .filter(|ch| *ch != ',')
        .collect::<String>();
    normalized
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}
