//! Structured-filter DSL → **parameterized** SQL predicates over the
//! `attributes` JSONB column. The cardinal rule here: field names are
//! charset-validated and every value becomes a bound parameter (`$n`) — no
//! caller-supplied value is ever string-interpolated into SQL.
//!
//! DSL shape (a JSON object):
//! ```json
//! {
//!   "price":      { "lt": 150 },
//!   "waterproof": true,                 // shorthand for { "eq": true }
//!   "type":       { "in": ["laptop"] },
//!   "tags":       { "contains": ["sale"] },
//!   "sku":        { "exists": true }
//! }
//! ```

use serde_json::Value;

use crate::error::ApiError;

/// A heterogeneous bound parameter. Bound into a `PgArguments` in order; the
/// SQL only ever references these by `$n` placeholder.
#[derive(Debug, Clone)]
pub enum Bound {
    Text(String),
    Float(f64),
    Int(i64),
    Json(Value),
    /// A `uuid[]` (graph seeds, id batches). Values are service-controlled.
    Uuids(Vec<uuid::Uuid>),
}

/// Append a bind and return its 1-based placeholder index.
pub fn push(binds: &mut Vec<Bound>, b: Bound) -> usize {
    binds.push(b);
    binds.len()
}

/// Build a `PgArguments` from the ordered bind list.
pub fn to_args(binds: &[Bound]) -> Result<sqlx::postgres::PgArguments, ApiError> {
    use sqlx::Arguments;
    let mut args = sqlx::postgres::PgArguments::default();
    for b in binds {
        let r = match b {
            Bound::Text(s) => args.add(s.clone()),
            Bound::Float(f) => args.add(*f),
            Bound::Int(i) => args.add(*i),
            Bound::Json(v) => args.add(sqlx::types::Json(v.clone())),
            Bound::Uuids(v) => args.add(v.clone()),
        };
        r.map_err(|e| ApiError::Invalid(format!("could not bind filter value: {e}")))?;
    }
    Ok(args)
}

fn validate_field(field: &str) -> Result<(), ApiError> {
    if field.is_empty() || field.len() > 128 {
        return Err(ApiError::Invalid("filter field name must be 1..=128 chars".into()));
    }
    if !field.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err(ApiError::Invalid(format!(
            "filter field `{field}` may contain only [A-Za-z0-9_.-]"
        )));
    }
    Ok(())
}

/// Render the filter object into a SQL boolean expression (predicates ANDed
/// together), appending binds. Returns an empty string for no filters.
pub fn render(filters: &Value, binds: &mut Vec<Bound>) -> Result<String, ApiError> {
    let obj = match filters {
        Value::Null => return Ok(String::new()),
        Value::Object(m) => m,
        _ => return Err(ApiError::Invalid("filters must be a JSON object".into())),
    };

    let mut preds: Vec<String> = Vec::new();
    for (field, cond) in obj {
        validate_field(field)?;
        match cond {
            Value::Object(ops) => {
                for (op, val) in ops {
                    preds.push(render_op(field, op, val, binds)?);
                }
            }
            // Shorthand `field: scalar` ⇒ equality.
            scalar => preds.push(eq_pred(field, scalar, binds)?),
        }
    }
    Ok(preds.join(" and "))
}

/// Equality via JSONB containment — type-accurate for any scalar (avoids the
/// `"5" != "5.0"` pitfall of text comparison).
fn eq_pred(field: &str, val: &Value, binds: &mut Vec<Bound>) -> Result<String, ApiError> {
    let obj = Value::Object(serde_json::Map::from_iter([(field.to_string(), val.clone())]));
    let n = push(binds, Bound::Json(obj));
    Ok(format!("attributes @> ${n}::jsonb"))
}

fn num(field: &str, op: &str, val: &Value, binds: &mut Vec<Bound>) -> Result<String, ApiError> {
    let f = val
        .as_f64()
        .ok_or_else(|| ApiError::Invalid(format!("`{op}` on `{field}` requires a number")))?;
    let n = push(binds, Bound::Float(f));
    Ok(format!("(attributes->>'{field}')::numeric {op} ${n}"))
}

fn render_op(field: &str, op: &str, val: &Value, binds: &mut Vec<Bound>) -> Result<String, ApiError> {
    match op {
        "eq" => eq_pred(field, val, binds),
        "ne" => Ok(format!("not ({})", eq_pred(field, val, binds)?)),
        "gt" => num(field, ">", val, binds),
        "gte" => num(field, ">=", val, binds),
        "lt" => num(field, "<", val, binds),
        "lte" => num(field, "<=", val, binds),
        "in" => {
            let arr = val
                .as_array()
                .ok_or_else(|| ApiError::Invalid(format!("`in` on `{field}` requires an array")))?;
            if arr.is_empty() {
                // `x in ()` ⇒ always false.
                return Ok("false".into());
            }
            let ors: Vec<String> = arr.iter().map(|v| eq_pred(field, v, binds)).collect::<Result<_, _>>()?;
            Ok(format!("({})", ors.join(" or ")))
        }
        "contains" => {
            // Field-scoped JSONB containment (array membership or sub-object).
            let n = push(binds, Bound::Json(val.clone()));
            Ok(format!("(attributes->'{field}') @> ${n}::jsonb"))
        }
        "exists" => {
            let want = val.as_bool().ok_or_else(|| {
                ApiError::Invalid(format!("`exists` on `{field}` requires a boolean"))
            })?;
            let n = push(binds, Bound::Text(field.to_string()));
            Ok(if want {
                format!("attributes ? ${n}")
            } else {
                format!("not (attributes ? ${n})")
            })
        }
        other => Err(ApiError::Invalid(format!("unknown filter operator `{other}`"))),
    }
}
