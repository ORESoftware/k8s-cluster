use serde_json::Value;
use sqlparser::{
    ast::{
        BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr,
        LimitClause, Select, SelectItem, SetExpr, Statement, TableFactor, Value as SqlValue,
    },
    dialect::GenericDialect,
    parser::Parser,
};

use super::{
    util::{clean_field, clean_identifier},
    AggregationExpr, AggregationOp, ApiError, FilterExpr, LogicalPlan, QueryRequest,
    MAX_QUERY_ROWS, SCHEMA_VERSION,
};

pub(crate) fn parse_select(
    request: &QueryRequest,
    default_limit: usize,
) -> Result<LogicalPlan, ApiError> {
    let dialect = GenericDialect {};
    let statements = parse_sql_with_dataset_retry(&dialect, request)?;
    if statements.len() != 1 {
        return Err(ApiError::bad_request(
            "SQL endpoint accepts exactly one SELECT statement",
        ));
    }

    let Statement::Query(query) = &statements[0] else {
        return Err(ApiError::bad_request(
            "SQL endpoint only supports SELECT queries",
        ));
    };
    if query.with.is_some() {
        return Err(ApiError::bad_request(
            "SQL CTEs are not supported by the in-memory frontend yet",
        ));
    }
    if query.order_by.is_some() {
        return Err(ApiError::bad_request(
            "SQL ORDER BY is not supported by the in-memory frontend yet",
        ));
    }

    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(ApiError::bad_request(
            "SQL set operations and nested query bodies are not supported yet",
        ));
    };
    let source = source_from_select(select)?
        .or_else(|| {
            request
                .dataset_id
                .as_ref()
                .and_then(|value| clean_identifier(value))
        })
        .ok_or_else(|| ApiError::bad_request("SQL query must include FROM or datasetId"))?;
    let limit = limit_from_query(query.limit_clause.as_ref(), default_limit)?;
    let group_by = group_by_from_select(select)?;
    let filter = match &select.selection {
        Some(expr) => Some(filter_from_expr(expr)?),
        None => None,
    };
    if select.having.is_some() {
        return Err(ApiError::bad_request(
            "SQL HAVING is not supported by the in-memory frontend yet",
        ));
    }
    if !select.cluster_by.is_empty()
        || !select.distribute_by.is_empty()
        || !select.sort_by.is_empty()
        || !select.lateral_views.is_empty()
    {
        return Err(ApiError::bad_request(
            "SQL cluster/distribute/sort/lateral clauses are not supported yet",
        ));
    }

    let mut projections = Vec::new();
    let mut aggregations = Vec::new();
    let mut group_by = group_by;
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => {}
            SelectItem::UnnamedExpr(expr) => {
                if let Some(aggregation) = aggregation_from_expr(expr, None)? {
                    aggregations.push(aggregation);
                } else if let Some(field) = field_from_expr(expr) {
                    if !aggregations.is_empty() && !group_by.contains(&field) {
                        group_by.push(field.clone());
                    }
                    projections.push(field);
                } else {
                    return Err(ApiError::bad_request(format!(
                        "unsupported SQL projection `{expr}`"
                    )));
                }
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                if let Some(aggregation) = aggregation_from_expr(expr, Some(alias.value.clone()))? {
                    aggregations.push(aggregation);
                } else if let Some(field) = field_from_expr(expr) {
                    projections.push(field);
                } else {
                    return Err(ApiError::bad_request(format!(
                        "unsupported SQL aliased projection `{expr}`"
                    )));
                }
            }
            SelectItem::ExprWithAliases { .. } | SelectItem::QualifiedWildcard(_, _) => {
                return Err(ApiError::bad_request(
                    "SQL qualified wildcards and multi-alias projections are not supported yet",
                ));
            }
        }
    }

    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: request.dialect,
        source,
        projections,
        filter,
        group_by,
        aggregations,
        limit,
    })
}

fn parse_sql_with_dataset_retry(
    dialect: &GenericDialect,
    request: &QueryRequest,
) -> Result<Vec<Statement>, ApiError> {
    match Parser::parse_sql(dialect, &request.query) {
        Ok(statements) => Ok(statements),
        Err(first_error) => {
            let Some(dataset_id) = request.dataset_id.as_deref() else {
                return Err(ApiError::bad_request(format!(
                    "SQL parse error: {first_error}"
                )));
            };
            if !dataset_id.contains('-') {
                return Err(ApiError::bad_request(format!(
                    "SQL parse error: {first_error}"
                )));
            }
            let quoted = quote_sql_identifier(dataset_id);
            let rewritten = replace_from_dataset_id(&request.query, dataset_id, &quoted);
            if rewritten == request.query {
                return Err(ApiError::bad_request(format!(
                    "SQL parse error: {first_error}"
                )));
            }
            Parser::parse_sql(dialect, &rewritten).map_err(|retry_error| {
                ApiError::bad_request(format!(
                    "SQL parse error: {first_error}; retry after quoting datasetId also failed: {retry_error}"
                ))
            })
        }
    }
}

fn quote_sql_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn replace_from_dataset_id(query: &str, dataset_id: &str, quoted: &str) -> String {
    query
        .replace(&format!("FROM {dataset_id}"), &format!("FROM {quoted}"))
        .replace(&format!("from {dataset_id}"), &format!("from {quoted}"))
        .replace(&format!("From {dataset_id}"), &format!("From {quoted}"))
}

fn source_from_select(select: &Select) -> Result<Option<String>, ApiError> {
    if select.from.is_empty() {
        return Ok(None);
    }
    if select.from.len() != 1 {
        return Err(ApiError::bad_request(
            "SQL frontend currently supports one FROM source",
        ));
    }
    let table = &select.from[0];
    if !table.joins.is_empty() {
        return Err(ApiError::bad_request(
            "SQL joins are not supported by the in-memory frontend yet",
        ));
    }
    match &table.relation {
        TableFactor::Table { name, .. } => Ok(clean_identifier(&name.to_string())),
        _ => Err(ApiError::bad_request(
            "SQL FROM source must be a named dataset table",
        )),
    }
}

fn limit_from_query(
    limit_clause: Option<&LimitClause>,
    default_limit: usize,
) -> Result<usize, ApiError> {
    let Some(limit_clause) = limit_clause else {
        return Ok(default_limit.clamp(1, MAX_QUERY_ROWS));
    };
    let limit = match limit_clause {
        LimitClause::LimitOffset { limit, .. } => match limit {
            Some(expr) => integer_literal(expr)?,
            None => default_limit,
        },
        LimitClause::OffsetCommaLimit { limit, .. } => integer_literal(limit)?,
    };
    Ok(limit.clamp(1, MAX_QUERY_ROWS))
}

fn group_by_from_select(select: &Select) -> Result<Vec<String>, ApiError> {
    match &select.group_by {
        GroupByExpr::All(_) => Err(ApiError::bad_request(
            "SQL GROUP BY ALL is not supported by the in-memory frontend yet",
        )),
        GroupByExpr::Expressions(expressions, modifiers) => {
            if !modifiers.is_empty() {
                return Err(ApiError::bad_request(
                    "SQL GROUP BY modifiers are not supported yet",
                ));
            }
            expressions
                .iter()
                .map(|expr| {
                    field_from_expr(expr).ok_or_else(|| {
                        ApiError::bad_request(format!("unsupported GROUP BY expression `{expr}`"))
                    })
                })
                .collect()
        }
    }
}

fn aggregation_from_expr(
    expr: &Expr,
    alias: Option<String>,
) -> Result<Option<AggregationExpr>, ApiError> {
    let Expr::Function(function) = expr else {
        return Ok(None);
    };
    let op = match function.name.to_string().to_ascii_lowercase().as_str() {
        "count" => AggregationOp::Count,
        "sum" => AggregationOp::Sum,
        "avg" | "mean" => AggregationOp::Avg,
        "min" => AggregationOp::Min,
        "max" => AggregationOp::Max,
        _ => {
            return Err(ApiError::bad_request(format!(
                "unsupported aggregate function `{}`",
                function.name
            )))
        }
    };
    if function.filter.is_some()
        || function.over.is_some()
        || !function.within_group.is_empty()
        || function.null_treatment.is_some()
    {
        return Err(ApiError::bad_request(
            "SQL aggregate FILTER/OVER/WITHIN GROUP/null treatment clauses are not supported yet",
        ));
    }
    let field = match &function.args {
        FunctionArguments::List(args) => {
            if args.args.len() > 1 {
                return Err(ApiError::bad_request(
                    "SQL aggregate functions support at most one argument",
                ));
            }
            match args.args.first() {
                None => None,
                Some(FunctionArg::Unnamed(FunctionArgExpr::Wildcard)) => None,
                Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))) => field_from_expr(expr),
                Some(_) => {
                    return Err(ApiError::bad_request(
                        "SQL aggregate named or qualified wildcard arguments are not supported yet",
                    ))
                }
            }
        }
        FunctionArguments::None => None,
        FunctionArguments::Subquery(_) => {
            return Err(ApiError::bad_request(
                "SQL aggregate subquery arguments are not supported",
            ))
        }
    };
    if op != AggregationOp::Count && field.is_none() {
        return Err(ApiError::bad_request(
            "SQL sum/avg/min/max aggregates require a field argument",
        ));
    }

    Ok(Some(AggregationExpr {
        alias: alias.unwrap_or_else(|| {
            field
                .as_ref()
                .map(|field| format!("{}_{}", op.as_str(), field))
                .unwrap_or_else(|| op.as_str().to_string())
        }),
        op,
        field,
    }))
}

fn filter_from_expr(expr: &Expr) -> Result<FilterExpr, ApiError> {
    let Expr::BinaryOp { left, op, right } = expr else {
        return Err(ApiError::bad_request(
            "SQL WHERE currently supports one simple binary comparison",
        ));
    };
    let field = field_from_expr(left)
        .ok_or_else(|| ApiError::bad_request("SQL WHERE left side must be a field"))?;
    let op = match op {
        BinaryOperator::Eq => "=",
        BinaryOperator::NotEq => "!=",
        BinaryOperator::Gt => ">",
        BinaryOperator::GtEq => ">=",
        BinaryOperator::Lt => "<",
        BinaryOperator::LtEq => "<=",
        _ => {
            return Err(ApiError::bad_request(
                "SQL WHERE only supports =, !=, >, >=, <, and <= comparisons",
            ))
        }
    };
    let value = literal_value(right)
        .ok_or_else(|| ApiError::bad_request("SQL WHERE right side must be a literal"))?;
    Ok(FilterExpr {
        field,
        op: op.to_string(),
        value,
    })
}

fn field_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => clean_field(&ident.value),
        Expr::CompoundIdentifier(parts) => parts.last().and_then(|ident| clean_field(&ident.value)),
        _ => None,
    }
}

fn integer_literal(expr: &Expr) -> Result<usize, ApiError> {
    match expr {
        Expr::Value(value) => match &value.value {
            SqlValue::Number(raw, _) => raw
                .parse::<usize>()
                .map_err(|_| ApiError::bad_request("SQL LIMIT must be a positive integer")),
            _ => Err(ApiError::bad_request("SQL LIMIT must be numeric")),
        },
        _ => Err(ApiError::bad_request("SQL LIMIT must be a literal integer")),
    }
}

fn literal_value(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Value(value) => match &value.value {
            SqlValue::Number(raw, _) => raw.parse::<f64>().ok().map(Value::from),
            SqlValue::SingleQuotedString(value)
            | SqlValue::DoubleQuotedString(value)
            | SqlValue::TripleSingleQuotedString(value)
            | SqlValue::TripleDoubleQuotedString(value)
            | SqlValue::EscapedStringLiteral(value)
            | SqlValue::UnicodeStringLiteral(value)
            | SqlValue::NationalStringLiteral(value) => Some(Value::from(value.clone())),
            SqlValue::Boolean(value) => Some(Value::from(*value)),
            SqlValue::Null => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryDialect;

    #[test]
    fn parser_backed_sql_extracts_grouped_aggregates() {
        let plan = parse_select(
            &QueryRequest {
                dialect: QueryDialect::Sql,
                query: "SELECT region, SUM(revenue) AS totalRevenue FROM sales-lab WHERE margin >= 0.2 GROUP BY region LIMIT 20".to_string(),
                dataset_id: Some("sales-lab".to_string()),
                limit: None,
            },
            1_000,
        )
        .expect("SQL parses");

        assert_eq!(plan.source, "sales-lab");
        assert_eq!(plan.group_by, vec!["region"]);
        assert_eq!(plan.aggregations[0].alias, "totalRevenue");
        assert_eq!(plan.filter.unwrap().op, ">=");
        assert_eq!(plan.limit, 20);
    }

    #[test]
    fn parser_backed_sql_rejects_joins() {
        let error = parse_select(
            &QueryRequest {
                dialect: QueryDialect::Sql,
                query:
                    "SELECT a.region, SUM(a.revenue) FROM a JOIN b ON a.id = b.id GROUP BY a.region"
                        .to_string(),
                dataset_id: None,
                limit: None,
            },
            1_000,
        )
        .expect_err("joins rejected");

        assert!(error.message.contains("joins"));
    }
}
