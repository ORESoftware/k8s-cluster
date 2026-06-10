//! Dependency-free server-side SVG rendering for the common 2D marks.
//!
//! Produces a self-contained `<svg>` document from a spec plus inline rows so
//! the service can emit thumbnails, embeddable images, and PDF-ready vectors
//! without a browser or headless renderer. SVG is the only output format:
//! raster (PNG) would require pulling in a rasterizer, which this crate
//! deliberately avoids. SVG embeds directly in HTML and PDF and can be
//! rasterized downstream if needed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 480;
const MAX_WIDTH: u32 = 4000;
const MAX_HEIGHT: u32 = 4000;
const MAX_ROWS: usize = 20_000;
const HIST_BINS: usize = 12;
const MARGIN_LEFT: f64 = 64.0;
const MARGIN_RIGHT: f64 = 24.0;
const MARGIN_TOP: f64 = 48.0;
const MARGIN_BOTTOM: f64 = 56.0;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub mark: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Field bound to the x axis (categorical for bar, numeric otherwise).
    #[serde(default)]
    pub x: Option<String>,
    /// Field bound to the y axis (numeric measure).
    #[serde(default)]
    pub y: Option<String>,
    #[serde(default)]
    pub rows: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResponse {
    pub ok: bool,
    pub schema_version: String,
    pub mark: String,
    pub format: String,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub svg: String,
}

/// Pixel bounds of the inner plot area (y0 = top, y1 = bottom).
struct Area {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Area {
    fn sx(&self, value: f64, min: f64, max: f64) -> f64 {
        if (max - min).abs() < f64::EPSILON {
            (self.x0 + self.x1) / 2.0
        } else {
            self.x0 + (value - min) / (max - min) * (self.x1 - self.x0)
        }
    }

    fn sy(&self, value: f64, min: f64, max: f64) -> f64 {
        if (max - min).abs() < f64::EPSILON {
            (self.y0 + self.y1) / 2.0
        } else {
            self.y1 - (value - min) / (max - min) * (self.y1 - self.y0)
        }
    }
}

pub fn render(request: RenderRequest) -> Result<RenderResponse, String> {
    if let Some(format) = request.format.as_deref() {
        let normalized = format.trim().to_ascii_lowercase();
        if !normalized.is_empty() && normalized != "svg" {
            return Err(format!(
                "unsupported render format `{format}`; only `svg` is available"
            ));
        }
    }
    if request.rows.is_empty() {
        return Err("render requires at least one row".to_string());
    }
    if request.rows.len() > MAX_ROWS {
        return Err(format!("rows exceeds max {MAX_ROWS}"));
    }

    let width = request.width.unwrap_or(DEFAULT_WIDTH).clamp(160, MAX_WIDTH);
    let height = request.height.unwrap_or(DEFAULT_HEIGHT).clamp(120, MAX_HEIGHT);
    let title = request
        .title
        .clone()
        .unwrap_or_else(|| "dd-data-viz".to_string());
    let mark = request.mark.trim().to_string();

    let area = Area {
        x0: MARGIN_LEFT,
        y0: MARGIN_TOP,
        x1: width as f64 - MARGIN_RIGHT,
        y1: height as f64 - MARGIN_BOTTOM,
    };

    let body = match mark.as_str() {
        "bar" => render_bar(&request, &area)?,
        "line" | "scatter" | "stem" => render_xy(&request, &area, &mark)?,
        "histogram" => render_histogram(&request, &area)?,
        other => {
            return Err(format!(
                "server-side render does not support mark `{other}` (supported: bar, line, scatter, stem, histogram)"
            ))
        }
    };

    let svg = wrap_svg(width, height, &title, &area, &body);
    Ok(RenderResponse {
        ok: true,
        schema_version: "data-viz.render.v1".to_string(),
        mark,
        format: "svg".to_string(),
        content_type: "image/svg+xml".to_string(),
        width,
        height,
        svg,
    })
}

fn render_bar(request: &RenderRequest, area: &Area) -> Result<String, String> {
    let labels = category_column(request, area_len(request));
    let values = numeric_column(request, request.y.as_deref())
        .ok_or("bar render requires a numeric `y` field")?;
    let max = values.iter().copied().fold(0.0_f64, f64::max).max(1.0);

    let count = values.len().max(1);
    let slot = (area.x1 - area.x0) / count as f64;
    let bar_width = slot * 0.7;
    let mut body = axes(area, 0.0, max);
    for (index, value) in values.iter().enumerate() {
        let cx = area.x0 + slot * (index as f64 + 0.5);
        let top = area.sy(*value, 0.0, max);
        let bar_height = area.y1 - top;
        let _ = write!(
            body,
            r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="#4f72d8" />"##,
            cx - bar_width / 2.0,
            top,
            bar_width,
            bar_height.max(0.0)
        );
        if let Some(label) = labels.get(index) {
            let _ = write!(
                body,
                r##"<text x="{:.1}" y="{:.1}" font-size="10" text-anchor="middle" fill="#444">{}</text>"##,
                cx,
                area.y1 + 14.0,
                escape_xml(label)
            );
        }
    }
    Ok(body)
}

fn render_xy(request: &RenderRequest, area: &Area, mark: &str) -> Result<String, String> {
    let ys = numeric_column(request, request.y.as_deref())
        .ok_or("line/scatter/stem render requires a numeric `y` field")?;
    // x is numeric when available and parseable, otherwise the row index.
    let xs = numeric_column(request, request.x.as_deref())
        .filter(|values| values.len() == ys.len())
        .unwrap_or_else(|| (0..ys.len()).map(|index| index as f64).collect());

    let (x_min, x_max) = bounds(&xs, false);
    let (y_min, y_max) = bounds(&ys, true);

    let mut body = axes(area, y_min, y_max);
    let baseline = area.sy(0.0_f64.clamp(y_min, y_max), y_min, y_max);

    if mark == "line" {
        let points = xs
            .iter()
            .zip(&ys)
            .map(|(x, y)| format!("{:.1},{:.1}", area.sx(*x, x_min, x_max), area.sy(*y, y_min, y_max)))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            body,
            r##"<polyline points="{points}" fill="none" stroke="#4f72d8" stroke-width="2" />"##
        );
    }

    for (x, y) in xs.iter().zip(&ys) {
        let px = area.sx(*x, x_min, x_max);
        let py = area.sy(*y, y_min, y_max);
        if mark == "stem" {
            let _ = write!(
                body,
                r##"<line x1="{px:.1}" y1="{baseline:.1}" x2="{px:.1}" y2="{py:.1}" stroke="#4f72d8" stroke-width="1.5" />"##
            );
        }
        // Point markers on every mark (line gets markers on top of the path).
        let _ = write!(
            body,
            r##"<circle cx="{px:.1}" cy="{py:.1}" r="3" fill="#4f72d8" />"##
        );
    }
    Ok(body)
}

fn render_histogram(request: &RenderRequest, area: &Area) -> Result<String, String> {
    let field = request.x.as_deref().or(request.y.as_deref());
    let values = numeric_column(request, field)
        .ok_or("histogram render requires a numeric `x` (or `y`) field")?;
    let (min, max) = bounds(&values, false);
    let span = (max - min).max(f64::EPSILON);
    let mut counts = [0_usize; HIST_BINS];
    for value in &values {
        let mut bin = ((value - min) / span * HIST_BINS as f64).floor() as isize;
        bin = bin.clamp(0, HIST_BINS as isize - 1);
        counts[bin as usize] += 1;
    }
    let max_count = counts.iter().copied().max().unwrap_or(1).max(1) as f64;

    let slot = (area.x1 - area.x0) / HIST_BINS as f64;
    let mut body = axes(area, 0.0, max_count);
    for (index, count) in counts.iter().enumerate() {
        let top = area.sy(*count as f64, 0.0, max_count);
        let x = area.x0 + slot * index as f64;
        let _ = write!(
            body,
            r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="#4f72d8" stroke="#fff" />"##,
            x,
            top,
            slot * 0.96,
            (area.y1 - top).max(0.0)
        );
    }
    Ok(body)
}

/// Draw the plot frame: axes and a handful of y gridlines with value labels.
fn axes(area: &Area, y_min: f64, y_max: f64) -> String {
    let mut svg = String::new();
    // Plot border.
    let _ = write!(
        svg,
        r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#999" stroke-width="1" />"##,
        area.x0, area.y1, area.x1, area.y1
    );
    let _ = write!(
        svg,
        r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#999" stroke-width="1" />"##,
        area.x0, area.y0, area.x0, area.y1
    );
    // Five horizontal gridlines + labels.
    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let value = y_min + (y_max - y_min) * fraction;
        let y = area.sy(value, y_min, y_max);
        let _ = write!(
            svg,
            r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#eee" stroke-width="1" />"##,
            area.x0, y, area.x1, y
        );
        let _ = write!(
            svg,
            r##"<text x="{:.1}" y="{:.1}" font-size="10" text-anchor="end" fill="#666">{}</text>"##,
            area.x0 - 6.0,
            y + 3.0,
            format_tick(value)
        );
    }
    svg
}

fn wrap_svg(width: u32, height: u32, title: &str, area: &Area, body: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{label}"><rect width="{width}" height="{height}" fill="#ffffff" /><text x="{tx:.1}" y="28" font-size="16" font-family="system-ui, sans-serif" fill="#222">{title_text}</text><g font-family="system-ui, sans-serif">{body}</g></svg>"##,
        label = escape_xml(title),
        tx = area.x0,
        title_text = escape_xml(title),
    )
}

fn area_len(request: &RenderRequest) -> usize {
    request.rows.len()
}

fn category_column(request: &RenderRequest, len: usize) -> Vec<String> {
    match request.x.as_deref() {
        Some(field) => request
            .rows
            .iter()
            .map(|row| match row.get(field) {
                Some(Value::String(text)) => text.clone(),
                Some(value) => value.to_string(),
                None => String::new(),
            })
            .collect(),
        None => (0..len).map(|index| index.to_string()).collect(),
    }
}

fn numeric_column(request: &RenderRequest, field: Option<&str>) -> Option<Vec<f64>> {
    let field = field?;
    let values: Vec<f64> = request
        .rows
        .iter()
        .filter_map(|row| row.get(field).and_then(as_f64))
        .collect();
    if values.len() == request.rows.len() && !values.is_empty() {
        Some(values)
    } else {
        None
    }
}

/// Numeric bounds; when `include_zero` is set the baseline is pulled to 0 so
/// bar/stem marks rest on a sensible axis.
fn bounds(values: &[f64], include_zero: bool) -> (f64, f64) {
    let mut min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let mut max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    if include_zero {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    if (max - min).abs() < f64::EPSILON {
        max = min + 1.0;
    }
    (min, max)
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn format_tick(value: f64) -> String {
    if value.abs() >= 1000.0 || (value.fract().abs() < 0.001 && value.abs() < 1e6) {
        format!("{:.0}", value)
    } else {
        format!("{:.2}", value)
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn descriptor() -> Value {
    serde_json::json!({
        "ok": true,
        "schemaVersion": "data-viz.render.v1",
        "format": "svg",
        "contentType": "image/svg+xml",
        "marks": ["bar", "line", "scatter", "stem", "histogram"],
        "notes": "Server-side SVG rendering with no browser or headless dependency. SVG embeds directly in HTML/PDF; PNG rasterization is intentionally not bundled.",
        "limits": { "maxRows": MAX_ROWS, "maxWidth": MAX_WIDTH, "maxHeight": MAX_HEIGHT }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(field: &str, values: &[i64]) -> Vec<BTreeMap<String, Value>> {
        values
            .iter()
            .map(|value| BTreeMap::from([(field.to_string(), json!(value))]))
            .collect()
    }

    #[test]
    fn rejects_non_svg_format() {
        let request = RenderRequest {
            mark: "bar".to_string(),
            format: Some("png".to_string()),
            title: None,
            width: None,
            height: None,
            x: None,
            y: Some("v".to_string()),
            rows: rows("v", &[1, 2, 3]),
        };
        let error = render(request).expect_err("png is rejected");
        assert!(error.contains("only `svg`"));
    }

    #[test]
    fn renders_bar_chart_svg() {
        let request = RenderRequest {
            mark: "bar".to_string(),
            format: Some("svg".to_string()),
            title: Some("Revenue".to_string()),
            width: None,
            height: None,
            x: None,
            y: Some("v".to_string()),
            rows: rows("v", &[10, 30, 20]),
        };
        let response = render(request).expect("bar renders");
        assert_eq!(response.content_type, "image/svg+xml");
        assert!(response.svg.starts_with("<svg"));
        assert!(response.svg.contains("<rect"));
        assert!(response.svg.contains("Revenue"));
    }

    #[test]
    fn renders_line_and_scatter_and_stem() {
        for mark in ["line", "scatter", "stem"] {
            let request = RenderRequest {
                mark: mark.to_string(),
                format: None,
                title: None,
                width: Some(400),
                height: Some(300),
                x: None,
                y: Some("v".to_string()),
                rows: rows("v", &[1, 5, 2, 8]),
            };
            let response = render(request).unwrap_or_else(|err| panic!("{mark} renders: {err}"));
            assert!(response.svg.contains("<circle"), "{mark} has points");
            if mark == "line" {
                assert!(response.svg.contains("<polyline"));
            }
            if mark == "stem" {
                assert!(response.svg.contains("<line"));
            }
        }
    }

    #[test]
    fn renders_histogram_bins() {
        let request = RenderRequest {
            mark: "histogram".to_string(),
            format: None,
            title: None,
            width: None,
            height: None,
            x: Some("v".to_string()),
            y: None,
            rows: rows("v", &[1, 1, 2, 3, 3, 3, 9]),
        };
        let response = render(request).expect("histogram renders");
        assert!(response.svg.matches("<rect").count() > 1);
    }

    #[test]
    fn rejects_unsupported_mark() {
        let request = RenderRequest {
            mark: "choropleth".to_string(),
            format: None,
            title: None,
            width: None,
            height: None,
            x: None,
            y: Some("v".to_string()),
            rows: rows("v", &[1, 2]),
        };
        let error = render(request).expect_err("unsupported mark");
        assert!(error.contains("does not support mark"));
    }
}
