use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_field, clean_identifier};

const MAX_DAX_BYTES: usize = 8 * 1024;
const MAX_DAX_TOKENS: usize = 512;
const MAX_DAX_DEPTH: usize = 32;
const MAX_DAX_ARGS: usize = 32;

const SUPPORTED_FUNCTIONS: &[&str] = &[
    "SUM",
    "AVERAGE",
    "MIN",
    "MAX",
    "COUNT",
    "COUNTROWS",
    "DISTINCTCOUNT",
    "DIVIDE",
    "IF",
    "CALCULATE",
    "COALESCE",
    "ABS",
    "ROUND",
    "BLANK",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileDaxRequest {
    pub dataset_id: String,
    pub expression: String,
    pub expression_kind: Option<DaxExpressionKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DaxExpressionKind {
    Measure,
    CalculatedColumn,
    Filter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileDaxResponse {
    ok: bool,
    compiled: CompiledDaxExpression,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompiledDaxExpression {
    dataset_id: String,
    expression_kind: DaxExpressionKind,
    normalized_expression: String,
    ast: DaxNode,
    dependencies: Vec<String>,
    sql_expression: String,
    logical_hint: DaxLogicalHint,
    supported_functions: Vec<&'static str>,
    posture: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaxLogicalHint {
    aggregation: Option<String>,
    field: Option<String>,
    calculation: String,
    can_push_down: bool,
    requires_row_context: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum DaxNode {
    Number {
        value: f64,
    },
    String {
        value: String,
    },
    Boolean {
        value: bool,
    },
    Blank,
    FieldRef {
        table: Option<String>,
        field: String,
    },
    TableRef {
        table: String,
    },
    Function {
        name: String,
        args: Vec<DaxNode>,
    },
    Binary {
        op: String,
        left: Box<DaxNode>,
        right: Box<DaxNode>,
    },
    Unary {
        op: String,
        expr: Box<DaxNode>,
    },
}

pub(crate) fn compile(
    request: CompileDaxRequest,
    available_fields: &BTreeSet<String>,
) -> Result<CompileDaxResponse, String> {
    let dataset_id = clean_identifier(&request.dataset_id).ok_or_else(|| {
        "datasetId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
    })?;
    let expression = request.expression.trim();
    if expression.is_empty() {
        return Err("DAX expression cannot be empty".to_string());
    }
    if expression.len() > MAX_DAX_BYTES {
        return Err(format!("DAX expression exceeds max {MAX_DAX_BYTES} bytes"));
    }
    if looks_secret_bearing(expression) {
        return Err("DAX expression appears to contain secret-bearing text".to_string());
    }
    if expression.contains(';') || expression.contains("--") || expression.contains("/*") {
        return Err("DAX expression cannot contain statement separators or comments".to_string());
    }

    let tokens = tokenize(expression)?;
    if tokens.len() > MAX_DAX_TOKENS {
        return Err(format!(
            "DAX expression exceeds max {MAX_DAX_TOKENS} tokens"
        ));
    }
    let expression_kind = request
        .expression_kind
        .unwrap_or(DaxExpressionKind::Measure);
    let mut parser = Parser::new(tokens, available_fields);
    let ast = parser.parse_expression(0)?;
    parser.expect_end()?;
    let dependencies = parser.dependencies.into_iter().collect::<Vec<_>>();
    let normalized_expression = render_dax(&ast);
    let sql_expression = sql_for_node(&ast)?;
    let mut warnings = Vec::new();
    let table_refs = table_refs(&ast);
    if table_refs.len() > 1 {
        warnings.push(format!(
            "DAX expression references {} tables; this compiler validates fields against dataset `{dataset_id}` only",
            table_refs.len()
        ));
    }

    Ok(CompileDaxResponse {
        ok: true,
        compiled: CompiledDaxExpression {
            dataset_id,
            expression_kind,
            normalized_expression,
            ast: ast.clone(),
            dependencies,
            sql_expression,
            logical_hint: logical_hint(&ast, expression_kind),
            supported_functions: supported_functions().to_vec(),
            posture: "bounded DAX subset parser; validates field references and returns an execution preview without evaluating user expressions",
        },
        warnings,
    })
}

pub(crate) fn supported_functions() -> &'static [&'static str] {
    SUPPORTED_FUNCTIONS
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxExpressionBytes": MAX_DAX_BYTES,
        "maxTokens": MAX_DAX_TOKENS,
        "maxDepth": MAX_DAX_DEPTH,
        "maxFunctionArgs": MAX_DAX_ARGS,
        "supportedFunctions": supported_functions()
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Identifier(String),
    QuotedIdentifier(String),
    BracketIdentifier(String),
    Number(String),
    String(String),
    Plus,
    Minus,
    Star,
    Slash,
    Comma,
    LParen,
    RParen,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    End,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars = input.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        match ch {
            '\'' => {
                let (value, next) = read_until(&chars, index + 1, '\'')?;
                tokens.push(Token::QuotedIdentifier(value));
                index = next;
            }
            '[' => {
                let (value, next) = read_until(&chars, index + 1, ']')?;
                tokens.push(Token::BracketIdentifier(value));
                index = next;
            }
            '"' => {
                let (value, next) = read_until(&chars, index + 1, '"')?;
                tokens.push(Token::String(value));
                index = next;
            }
            '+' => {
                tokens.push(Token::Plus);
                index += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                index += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                index += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                index += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                index += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                index += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                index += 1;
            }
            '=' => {
                tokens.push(Token::Eq);
                index += 1;
            }
            '<' => {
                if chars.get(index + 1) == Some(&'=') {
                    tokens.push(Token::Le);
                    index += 2;
                } else if chars.get(index + 1) == Some(&'>') {
                    tokens.push(Token::Ne);
                    index += 2;
                } else {
                    tokens.push(Token::Lt);
                    index += 1;
                }
            }
            '>' => {
                if chars.get(index + 1) == Some(&'=') {
                    tokens.push(Token::Ge);
                    index += 2;
                } else {
                    tokens.push(Token::Gt);
                    index += 1;
                }
            }
            _ if ch.is_ascii_digit() || ch == '.' => {
                let start = index;
                index += 1;
                while chars
                    .get(index)
                    .map(|ch| ch.is_ascii_digit() || *ch == '.')
                    .unwrap_or(false)
                {
                    index += 1;
                }
                tokens.push(Token::Number(chars[start..index].iter().collect()));
            }
            _ if ch.is_ascii_alphabetic() || ch == '_' => {
                let start = index;
                index += 1;
                while chars
                    .get(index)
                    .map(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .unwrap_or(false)
                {
                    index += 1;
                }
                tokens.push(Token::Identifier(chars[start..index].iter().collect()));
            }
            _ => {
                return Err(format!("unsupported DAX character `{ch}`"));
            }
        }
    }
    tokens.push(Token::End);
    Ok(tokens)
}

fn read_until(chars: &[char], start: usize, target: char) -> Result<(String, usize), String> {
    let mut value = String::new();
    let mut index = start;
    while index < chars.len() {
        if chars[index] == target {
            return Ok((value, index + 1));
        }
        value.push(chars[index]);
        index += 1;
    }
    Err(format!("unterminated DAX segment ending with `{target}`"))
}

struct Parser<'a> {
    tokens: Vec<Token>,
    index: usize,
    available_fields: &'a BTreeSet<String>,
    dependencies: BTreeSet<String>,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<Token>, available_fields: &'a BTreeSet<String>) -> Self {
        Self {
            tokens,
            index: 0,
            available_fields,
            dependencies: BTreeSet::new(),
        }
    }

    fn parse_expression(&mut self, depth: usize) -> Result<DaxNode, String> {
        if depth > MAX_DAX_DEPTH {
            return Err(format!("DAX expression exceeds max depth {MAX_DAX_DEPTH}"));
        }
        self.parse_comparison(depth)
    }

    fn parse_comparison(&mut self, depth: usize) -> Result<DaxNode, String> {
        let mut node = self.parse_additive(depth + 1)?;
        while let Some(op) = self.comparison_op() {
            self.advance();
            let right = self.parse_additive(depth + 1)?;
            node = DaxNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_additive(&mut self, depth: usize) -> Result<DaxNode, String> {
        let mut node = self.parse_multiplicative(depth + 1)?;
        loop {
            let op = match self.peek() {
                Token::Plus => "+",
                Token::Minus => "-",
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative(depth + 1)?;
            node = DaxNode::Binary {
                op: op.to_string(),
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_multiplicative(&mut self, depth: usize) -> Result<DaxNode, String> {
        let mut node = self.parse_unary(depth + 1)?;
        loop {
            let op = match self.peek() {
                Token::Star => "*",
                Token::Slash => "/",
                _ => break,
            };
            self.advance();
            let right = self.parse_unary(depth + 1)?;
            node = DaxNode::Binary {
                op: op.to_string(),
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<DaxNode, String> {
        match self.peek() {
            Token::Minus => {
                self.advance();
                Ok(DaxNode::Unary {
                    op: "-".to_string(),
                    expr: Box::new(self.parse_unary(depth + 1)?),
                })
            }
            _ => self.parse_primary(depth + 1),
        }
    }

    fn parse_primary(&mut self, depth: usize) -> Result<DaxNode, String> {
        if depth > MAX_DAX_DEPTH {
            return Err(format!("DAX expression exceeds max depth {MAX_DAX_DEPTH}"));
        }
        match self.advance() {
            Token::Number(value) => {
                let parsed = value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid DAX number `{value}`"))?;
                Ok(DaxNode::Number { value: parsed })
            }
            Token::String(value) => Ok(DaxNode::String { value }),
            Token::BracketIdentifier(field) => self.field_ref(None, &field),
            Token::Identifier(value) | Token::QuotedIdentifier(value) => {
                self.identifier_or_function(value, depth)
            }
            Token::LParen => {
                let node = self.parse_expression(depth + 1)?;
                self.expect_rparen()?;
                Ok(node)
            }
            other => Err(format!("unexpected DAX token `{}`", token_label(&other))),
        }
    }

    fn identifier_or_function(&mut self, value: String, depth: usize) -> Result<DaxNode, String> {
        if matches!(self.peek(), Token::LParen) {
            self.advance();
            let name = value.to_ascii_uppercase();
            if name == "TRUE" || name == "FALSE" {
                self.expect_rparen()?;
                return Ok(DaxNode::Boolean {
                    value: name == "TRUE",
                });
            }
            let mut args = Vec::new();
            if !matches!(self.peek(), Token::RParen) {
                loop {
                    if args.len() >= MAX_DAX_ARGS {
                        return Err(format!("DAX function exceeds max {MAX_DAX_ARGS} args"));
                    }
                    args.push(self.parse_expression(depth + 1)?);
                    if matches!(self.peek(), Token::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
            }
            self.expect_rparen()?;
            validate_function(&name, &args)?;
            if name == "BLANK" {
                return Ok(DaxNode::Blank);
            }
            return Ok(DaxNode::Function { name, args });
        }
        if let Token::BracketIdentifier(field) = self.peek().clone() {
            self.advance();
            let table = clean_identifier(&value)
                .or_else(|| clean_field(&value))
                .unwrap_or_else(|| "table".to_string());
            return self.field_ref(Some(table), &field);
        }
        match value.to_ascii_uppercase().as_str() {
            "TRUE" => Ok(DaxNode::Boolean { value: true }),
            "FALSE" => Ok(DaxNode::Boolean { value: false }),
            _ => {
                let table = clean_identifier(&value)
                    .or_else(|| clean_field(&value))
                    .ok_or_else(|| format!("invalid DAX table or identifier `{value}`"))?;
                Ok(DaxNode::TableRef { table })
            }
        }
    }

    fn field_ref(&mut self, table: Option<String>, field: &str) -> Result<DaxNode, String> {
        let field = clean_field(field).ok_or_else(|| {
            "DAX field references must contain letters, numbers, dash, underscore, dot, or colon"
                .to_string()
        })?;
        if !self.available_fields.contains(&field) {
            return Err(format!("DAX field `{field}` does not exist in dataset"));
        }
        self.dependencies.insert(field.clone());
        Ok(DaxNode::FieldRef { table, field })
    }

    fn comparison_op(&self) -> Option<String> {
        match self.peek() {
            Token::Eq => Some("=".to_string()),
            Token::Ne => Some("<>".to_string()),
            Token::Lt => Some("<".to_string()),
            Token::Le => Some("<=".to_string()),
            Token::Gt => Some(">".to_string()),
            Token::Ge => Some(">=".to_string()),
            _ => None,
        }
    }

    fn expect_rparen(&mut self) -> Result<(), String> {
        match self.advance() {
            Token::RParen => Ok(()),
            other => Err(format!("expected `)`, found `{}`", token_label(&other))),
        }
    }

    fn expect_end(&self) -> Result<(), String> {
        if matches!(self.peek(), Token::End) {
            Ok(())
        } else {
            Err(format!(
                "unexpected trailing DAX token `{}`",
                token_label(self.peek())
            ))
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.index).unwrap_or(&Token::End)
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        self.index += 1;
        token
    }
}

fn validate_function(name: &str, args: &[DaxNode]) -> Result<(), String> {
    if !SUPPORTED_FUNCTIONS.contains(&name) {
        return Err(format!("unsupported DAX function `{name}`"));
    }
    match name {
        "SUM" | "AVERAGE" | "MIN" | "MAX" | "COUNT" | "DISTINCTCOUNT" => {
            if args.len() != 1 {
                return Err(format!(
                    "DAX function `{name}` expects exactly one argument"
                ));
            }
            if !matches!(args.first(), Some(DaxNode::FieldRef { .. })) {
                return Err(format!("DAX function `{name}` expects a field reference"));
            }
        }
        "COUNTROWS" | "ABS" => {
            if args.len() != 1 {
                return Err(format!(
                    "DAX function `{name}` expects exactly one argument"
                ));
            }
        }
        "ROUND" => {
            if args.len() != 2 {
                return Err("DAX function `ROUND` expects exactly two arguments".to_string());
            }
        }
        "DIVIDE" | "IF" => {
            if !(2..=3).contains(&args.len()) {
                return Err(format!(
                    "DAX function `{name}` expects two or three arguments"
                ));
            }
        }
        "CALCULATE" | "COALESCE" => {
            if args.is_empty() {
                return Err(format!(
                    "DAX function `{name}` expects at least one argument"
                ));
            }
        }
        "BLANK" => {
            if !args.is_empty() {
                return Err("DAX function `BLANK` expects no arguments".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn logical_hint(ast: &DaxNode, expression_kind: DaxExpressionKind) -> DaxLogicalHint {
    let (aggregation, field) = root_aggregation(ast).unwrap_or((None, None));
    DaxLogicalHint {
        aggregation,
        field,
        calculation: calculation_label(ast).to_string(),
        can_push_down: !contains_function(ast, "CALCULATE"),
        requires_row_context: expression_kind == DaxExpressionKind::CalculatedColumn
            && !contains_aggregate(ast),
    }
}

fn root_aggregation(ast: &DaxNode) -> Option<(Option<String>, Option<String>)> {
    match ast {
        DaxNode::Function { name, args } => match name.as_str() {
            "SUM" | "MIN" | "MAX" | "COUNT" | "DISTINCTCOUNT" => args
                .first()
                .and_then(field_name)
                .map(|field| (Some(name.to_ascii_lowercase()), Some(field))),
            "AVERAGE" => args
                .first()
                .and_then(field_name)
                .map(|field| (Some("avg".to_string()), Some(field))),
            "COUNTROWS" => Some((Some("count".to_string()), None)),
            _ => None,
        },
        _ => None,
    }
}

fn field_name(node: &DaxNode) -> Option<String> {
    match node {
        DaxNode::FieldRef { field, .. } => Some(field.clone()),
        _ => None,
    }
}

fn calculation_label(ast: &DaxNode) -> &'static str {
    match ast {
        DaxNode::Function { name, .. } if name == "DIVIDE" => "ratio",
        DaxNode::Function { name, .. } if name == "IF" => "conditional",
        DaxNode::Function { name, .. } if name == "CALCULATE" => "filtered-measure",
        DaxNode::Function { name, .. } if aggregate_name(name).is_some() => "aggregate",
        DaxNode::Binary { .. } => "arithmetic-or-filter",
        DaxNode::FieldRef { .. } => "field",
        _ => "scalar",
    }
}

fn contains_aggregate(ast: &DaxNode) -> bool {
    match ast {
        DaxNode::Function { name, args } => {
            aggregate_name(name).is_some() || args.iter().any(contains_aggregate)
        }
        DaxNode::Binary { left, right, .. } => {
            contains_aggregate(left) || contains_aggregate(right)
        }
        DaxNode::Unary { expr, .. } => contains_aggregate(expr),
        _ => false,
    }
}

fn contains_function(ast: &DaxNode, target: &str) -> bool {
    match ast {
        DaxNode::Function { name, args } => {
            name == target || args.iter().any(|arg| contains_function(arg, target))
        }
        DaxNode::Binary { left, right, .. } => {
            contains_function(left, target) || contains_function(right, target)
        }
        DaxNode::Unary { expr, .. } => contains_function(expr, target),
        _ => false,
    }
}

fn aggregate_name(name: &str) -> Option<&'static str> {
    match name {
        "SUM" => Some("sum"),
        "AVERAGE" => Some("avg"),
        "MIN" => Some("min"),
        "MAX" => Some("max"),
        "COUNT" => Some("count"),
        "COUNTROWS" => Some("count"),
        "DISTINCTCOUNT" => Some("distinct-count"),
        _ => None,
    }
}

fn render_dax(ast: &DaxNode) -> String {
    match ast {
        DaxNode::Number { value } => value.to_string(),
        DaxNode::String { value } => format!("\"{}\"", value.replace('"', "\"\"")),
        DaxNode::Boolean { value } => value.to_string().to_ascii_uppercase(),
        DaxNode::Blank => "BLANK()".to_string(),
        DaxNode::FieldRef { table, field } => table
            .as_ref()
            .map(|table| format!("'{table}'[{field}]"))
            .unwrap_or_else(|| format!("[{field}]")),
        DaxNode::TableRef { table } => format!("'{table}'"),
        DaxNode::Function { name, args } => {
            let args = args.iter().map(render_dax).collect::<Vec<_>>().join(", ");
            format!("{name}({args})")
        }
        DaxNode::Binary { op, left, right } => {
            format!("({} {} {})", render_dax(left), op, render_dax(right))
        }
        DaxNode::Unary { op, expr } => format!("({}{})", op, render_dax(expr)),
    }
}

fn sql_for_node(ast: &DaxNode) -> Result<String, String> {
    match ast {
        DaxNode::Number { value } => Ok(value.to_string()),
        DaxNode::String { value } => Ok(format!("'{}'", value.replace('\'', "''"))),
        DaxNode::Boolean { value } => Ok(value.to_string().to_ascii_uppercase()),
        DaxNode::Blank => Ok("NULL".to_string()),
        DaxNode::FieldRef { field, .. } => Ok(quote_sql_identifier(field)),
        DaxNode::TableRef { table } => Ok(quote_sql_identifier(table)),
        DaxNode::Unary { op, expr } => Ok(format!("({op}{})", sql_for_node(expr)?)),
        DaxNode::Binary { op, left, right } => Ok(format!(
            "({} {op} {})",
            sql_for_node(left)?,
            sql_for_node(right)?
        )),
        DaxNode::Function { name, args } => sql_for_function(name, args),
    }
}

fn sql_for_function(name: &str, args: &[DaxNode]) -> Result<String, String> {
    match name {
        "SUM" | "MIN" | "MAX" | "COUNT" => Ok(format!("{name}({})", sql_for_node(&args[0])?)),
        "AVERAGE" => Ok(format!("AVG({})", sql_for_node(&args[0])?)),
        "DISTINCTCOUNT" => Ok(format!("COUNT(DISTINCT {})", sql_for_node(&args[0])?)),
        "COUNTROWS" => Ok("COUNT(*)".to_string()),
        "ABS" => Ok(format!("ABS({})", sql_for_node(&args[0])?)),
        "ROUND" => Ok(format!(
            "ROUND({}, {})",
            sql_for_node(&args[0])?,
            sql_for_node(&args[1])?
        )),
        "COALESCE" => Ok(format!(
            "COALESCE({})",
            args.iter()
                .map(sql_for_node)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
        "IF" => {
            let fallback = if args.len() == 3 {
                sql_for_node(&args[2])?
            } else {
                "NULL".to_string()
            };
            Ok(format!(
                "CASE WHEN {} THEN {} ELSE {} END",
                sql_for_node(&args[0])?,
                sql_for_node(&args[1])?,
                fallback
            ))
        }
        "DIVIDE" => {
            let numerator = sql_for_node(&args[0])?;
            let denominator = sql_for_node(&args[1])?;
            let fallback = if args.len() == 3 {
                sql_for_node(&args[2])?
            } else {
                "NULL".to_string()
            };
            Ok(format!(
                "CASE WHEN ({denominator}) = 0 OR ({denominator}) IS NULL THEN {fallback} ELSE ({numerator}) / ({denominator}) END"
            ))
        }
        "CALCULATE" => Ok(format!(
            "{} /* CALCULATE filters compiled as planner predicates: {} */",
            sql_for_node(&args[0])?,
            args.iter()
                .skip(1)
                .map(render_dax)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        other => Err(format!("unsupported DAX function `{other}`")),
    }
}

fn quote_sql_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn table_refs(ast: &DaxNode) -> HashSet<String> {
    let mut refs = HashSet::new();
    collect_table_refs(ast, &mut refs);
    refs
}

fn collect_table_refs(ast: &DaxNode, refs: &mut HashSet<String>) {
    match ast {
        DaxNode::FieldRef {
            table: Some(table), ..
        }
        | DaxNode::TableRef { table } => {
            refs.insert(table.clone());
        }
        DaxNode::Function { args, .. } => {
            for arg in args {
                collect_table_refs(arg, refs);
            }
        }
        DaxNode::Binary { left, right, .. } => {
            collect_table_refs(left, refs);
            collect_table_refs(right, refs);
        }
        DaxNode::Unary { expr, .. } => collect_table_refs(expr, refs),
        _ => {}
    }
}

fn token_label(token: &Token) -> String {
    match token {
        Token::Identifier(value)
        | Token::QuotedIdentifier(value)
        | Token::BracketIdentifier(value)
        | Token::Number(value)
        | Token::String(value) => value.clone(),
        Token::Plus => "+".to_string(),
        Token::Minus => "-".to_string(),
        Token::Star => "*".to_string(),
        Token::Slash => "/".to_string(),
        Token::Comma => ",".to_string(),
        Token::LParen => "(".to_string(),
        Token::RParen => ")".to_string(),
        Token::Eq => "=".to_string(),
        Token::Ne => "<>".to_string(),
        Token::Lt => "<".to_string(),
        Token::Le => "<=".to_string(),
        Token::Gt => ">".to_string(),
        Token::Ge => ">=".to_string(),
        Token::End => "end".to_string(),
    }
}

fn looks_secret_bearing(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "authorization",
        "bearer",
        "api_key",
        "private_key",
        "access_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> BTreeSet<String> {
        ["revenue", "cost", "region", "visitors"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn dax_compile_handles_ratio_measure() {
        let response = compile(
            CompileDaxRequest {
                dataset_id: "sales".to_string(),
                expression:
                    "DIVIDE(SUM('sales'[revenue]) - SUM('sales'[cost]), SUM('sales'[revenue]), 0)"
                        .to_string(),
                expression_kind: Some(DaxExpressionKind::Measure),
            },
            &fields(),
        )
        .expect("DAX compiles");

        assert_eq!(response.compiled.dependencies, vec!["cost", "revenue"]);
        assert_eq!(response.compiled.logical_hint.calculation, "ratio");
        assert!(response.compiled.sql_expression.contains("CASE WHEN"));
        assert!(response
            .compiled
            .normalized_expression
            .contains("'sales'[revenue]"));
    }

    #[test]
    fn dax_compile_rejects_missing_fields() {
        let error = compile(
            CompileDaxRequest {
                dataset_id: "sales".to_string(),
                expression: "SUM([missing])".to_string(),
                expression_kind: None,
            },
            &fields(),
        )
        .expect_err("missing field is rejected");

        assert!(error.contains("does not exist"));
    }

    #[test]
    fn dax_compile_rejects_secret_like_text() {
        let error = compile(
            CompileDaxRequest {
                dataset_id: "sales".to_string(),
                expression: "SUM([revenue]) + token".to_string(),
                expression_kind: None,
            },
            &fields(),
        )
        .expect_err("secret-like text is rejected");

        assert!(error.contains("secret-bearing"));
    }

    #[test]
    fn dax_compile_validates_function_arity() {
        let error = compile(
            CompileDaxRequest {
                dataset_id: "sales".to_string(),
                expression: "DIVIDE(SUM([revenue]))".to_string(),
                expression_kind: None,
            },
            &fields(),
        )
        .expect_err("bad arity is rejected");

        assert!(error.contains("expects two or three"));
    }
}
