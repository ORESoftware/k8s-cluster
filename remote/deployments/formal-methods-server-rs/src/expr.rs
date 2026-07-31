use std::collections::HashSet;

const MAX_EXPRESSION_BYTES: usize = 4 * 1024;
const MAX_EXPRESSION_TOKENS: usize = 512;
const MAX_EXPRESSION_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// expression DSL: lexer + Pratt parser + SMT-LIB printer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Int(String),
    Real(String),
    True,
    False,
    LParen,
    RParen,
    Comma,
    OpOr,
    OpAnd,
    OpNot,
    OpEq,
    OpNeq,
    OpLt,
    OpLe,
    OpGt,
    OpGe,
    OpPlus,
    OpMinus,
    OpStar,
    OpSlash,
    OpPercent,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum SortHint {
    Int,
    Real,
    Bool,
    Unknown,
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Var(String),
    IntLit(String),
    RealLit(String),
    BoolLit(bool),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

fn lex(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let ident = &input[start..i];
            tokens.push(match ident {
                "true" => Token::True,
                "false" => Token::False,
                "and" => Token::OpAnd,
                "or" => Token::OpOr,
                "not" => Token::OpNot,
                _ => Token::Ident(ident.to_string()),
            });
            if tokens.len() > MAX_EXPRESSION_TOKENS {
                return Err(format!(
                    "expression exceeds the {MAX_EXPRESSION_TOKENS} token limit"
                ));
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            let mut saw_dot = false;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !saw_dot {
                    saw_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let lit = &input[start..i];
            tokens.push(if saw_dot {
                Token::Real(lit.to_string())
            } else {
                Token::Int(lit.to_string())
            });
            if tokens.len() > MAX_EXPRESSION_TOKENS {
                return Err(format!(
                    "expression exceeds the {MAX_EXPRESSION_TOKENS} token limit"
                ));
            }
            continue;
        }
        let next = bytes.get(i + 1).map(|b| *b as char);
        let pushed = match (c, next) {
            ('=', Some('=')) => {
                i += 2;
                Some(Token::OpEq)
            }
            ('!', Some('=')) => {
                i += 2;
                Some(Token::OpNeq)
            }
            ('<', Some('=')) => {
                i += 2;
                Some(Token::OpLe)
            }
            ('>', Some('=')) => {
                i += 2;
                Some(Token::OpGe)
            }
            ('&', Some('&')) => {
                i += 2;
                Some(Token::OpAnd)
            }
            ('|', Some('|')) => {
                i += 2;
                Some(Token::OpOr)
            }
            _ => None,
        };
        if let Some(tok) = pushed {
            tokens.push(tok);
            if tokens.len() > MAX_EXPRESSION_TOKENS {
                return Err(format!(
                    "expression exceeds the {MAX_EXPRESSION_TOKENS} token limit"
                ));
            }
            continue;
        }
        let tok = match c {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            '<' => Token::OpLt,
            '>' => Token::OpGt,
            '+' => Token::OpPlus,
            '-' => Token::OpMinus,
            '*' => Token::OpStar,
            '/' => Token::OpSlash,
            '%' => Token::OpPercent,
            '!' => Token::OpNot,
            other => return Err(format!("unexpected character {other:?}")),
        };
        i += 1;
        tokens.push(tok);
        if tokens.len() > MAX_EXPRESSION_TOKENS {
            return Err(format!(
                "expression exceeds the {MAX_EXPRESSION_TOKENS} token limit"
            ));
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_expr(&mut self, min_bp: u8, depth: usize) -> Result<Expr, String> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(format!(
                "expression exceeds the {MAX_EXPRESSION_DEPTH} level nesting limit"
            ));
        }
        let mut lhs = self.parse_unary(depth)?;
        loop {
            let (op, l_bp, r_bp) = match self.peek() {
                Some(Token::OpOr) => ("or", 10, 11),
                Some(Token::OpAnd) => ("and", 20, 21),
                Some(Token::OpEq) => ("=", 30, 31),
                Some(Token::OpNeq) => ("!=", 30, 31),
                Some(Token::OpLt) => ("<", 40, 41),
                Some(Token::OpLe) => ("<=", 40, 41),
                Some(Token::OpGt) => (">", 40, 41),
                Some(Token::OpGe) => (">=", 40, 41),
                Some(Token::OpPlus) => ("+", 50, 51),
                Some(Token::OpMinus) => ("-", 50, 51),
                Some(Token::OpStar) => ("*", 60, 61),
                Some(Token::OpSlash) => ("/", 60, 61),
                Some(Token::OpPercent) => ("mod", 60, 61),
                _ => break,
            };
            if l_bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_expr(r_bp, depth + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<Expr, String> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(format!(
                "expression exceeds the {MAX_EXPRESSION_DEPTH} level nesting limit"
            ));
        }
        match self.peek() {
            Some(Token::OpNot) => {
                self.bump();
                let inner = self.parse_unary(depth + 1)?;
                Ok(Expr::Unary("not", Box::new(inner)))
            }
            Some(Token::OpMinus) => {
                self.bump();
                let inner = self.parse_unary(depth + 1)?;
                Ok(Expr::Unary("-", Box::new(inner)))
            }
            _ => self.parse_atom(depth),
        }
    }

    fn parse_atom(&mut self, depth: usize) -> Result<Expr, String> {
        let tok = self
            .bump()
            .ok_or_else(|| "unexpected end of expression".to_string())?;
        match tok {
            Token::LParen => {
                let inner = self.parse_expr(0, depth + 1)?;
                match self.bump() {
                    Some(Token::RParen) => Ok(inner),
                    other => Err(format!("expected ')', got {other:?}")),
                }
            }
            Token::Int(value) => Ok(Expr::IntLit(value)),
            Token::Real(value) => Ok(Expr::RealLit(value)),
            Token::True => Ok(Expr::BoolLit(true)),
            Token::False => Ok(Expr::BoolLit(false)),
            Token::Ident(name) => {
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        loop {
                            args.push(self.parse_expr(0, depth + 1)?);
                            match self.peek() {
                                Some(Token::Comma) => {
                                    self.bump();
                                }
                                Some(Token::RParen) => break,
                                other => {
                                    return Err(format!(
                                        "expected ',' or ')' in argument list, got {other:?}"
                                    ));
                                }
                            }
                        }
                    }
                    match self.bump() {
                        Some(Token::RParen) => {}
                        other => return Err(format!("expected ')', got {other:?}")),
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

pub(crate) fn parse_expr(input: &str) -> Result<Expr, String> {
    if input.len() > MAX_EXPRESSION_BYTES {
        return Err(format!(
            "expression exceeds the {MAX_EXPRESSION_BYTES} byte limit"
        ));
    }
    let tokens = lex(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr(0, 0)?;
    if parser.pos != parser.tokens.len() {
        return Err(format!(
            "trailing tokens after expression near position {}",
            parser.pos
        ));
    }
    Ok(expr)
}

pub(crate) fn collect_vars(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Var(name) => {
            out.insert(name.clone());
        }
        Expr::Unary(_, inner) => collect_vars(inner, out),
        Expr::Binary(_, lhs, rhs) => {
            collect_vars(lhs, out);
            collect_vars(rhs, out);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_vars(arg, out);
            }
        }
        Expr::IntLit(_) | Expr::RealLit(_) | Expr::BoolLit(_) => {}
    }
}

pub(crate) fn expr_to_smt(expr: &Expr) -> Result<String, String> {
    match expr {
        Expr::Var(name) => Ok(name.clone()),
        Expr::IntLit(value) => Ok(value.clone()),
        Expr::RealLit(value) => Ok(value.clone()),
        Expr::BoolLit(value) => Ok(if *value {
            "true".into()
        } else {
            "false".into()
        }),
        Expr::Unary(op, inner) => Ok(format!("({op} {})", expr_to_smt(inner)?)),
        Expr::Binary(op, lhs, rhs) => {
            let lhs_s = expr_to_smt(lhs)?;
            let rhs_s = expr_to_smt(rhs)?;
            let smt_op = match *op {
                "!=" => {
                    return Ok(format!("(not (= {lhs_s} {rhs_s}))"));
                }
                "or" => "or",
                "and" => "and",
                "=" => "=",
                "<" => "<",
                "<=" => "<=",
                ">" => ">",
                ">=" => ">=",
                "+" => "+",
                "-" => "-",
                "*" => "*",
                "/" => "div",
                "mod" => "mod",
                other => return Err(format!("unsupported operator {other:?}")),
            };
            Ok(format!("({smt_op} {lhs_s} {rhs_s})"))
        }
        Expr::Call(name, args) => {
            let lname = name.to_ascii_lowercase();
            let n = args.len();
            match (lname.as_str(), n) {
                ("min", 2) => Ok(format!(
                    "(ite (<= {a} {b}) {a} {b})",
                    a = expr_to_smt(&args[0])?,
                    b = expr_to_smt(&args[1])?
                )),
                ("max", 2) => Ok(format!(
                    "(ite (>= {a} {b}) {a} {b})",
                    a = expr_to_smt(&args[0])?,
                    b = expr_to_smt(&args[1])?
                )),
                ("abs", 1) => {
                    let a = expr_to_smt(&args[0])?;
                    Ok(format!("(ite (>= {a} 0) {a} (- {a}))"))
                }
                _ => Err(format!(
                    "unsupported call {name}/{n}; supported: min(_,_), max(_,_), abs(_)"
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_handles_basic_tokens() {
        let toks = lex("x >= 0 && y != -3").unwrap();
        assert!(matches!(toks[0], Token::Ident(ref n) if n == "x"));
        assert!(matches!(toks[1], Token::OpGe));
        assert!(matches!(toks[2], Token::Int(ref n) if n == "0"));
        assert!(matches!(toks[3], Token::OpAnd));
    }

    #[test]
    fn parser_respects_precedence() {
        let expr = parse_expr("a + b * c >= d").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert_eq!(smt, "(>= (+ a (* b c)) d)");
    }

    #[test]
    fn parser_handles_neq_as_not_eq() {
        let expr = parse_expr("a != b").unwrap();
        assert_eq!(expr_to_smt(&expr).unwrap(), "(not (= a b))");
    }

    #[test]
    fn parser_handles_unary() {
        let expr = parse_expr("!(x > 0) || y == -1").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert_eq!(smt, "(or (not (> x 0)) (= y (- 1)))");
    }

    #[test]
    fn parser_handles_min_max_abs() {
        let expr = parse_expr("min(a, max(b, c)) > abs(d)").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert!(smt.contains("ite"));
    }

    #[test]
    fn collect_vars_picks_up_identifiers() {
        let expr = parse_expr("(x + y) * z > 0 && flag").unwrap();
        let mut vars = HashSet::new();
        collect_vars(&expr, &mut vars);
        assert!(vars.contains("x"));
        assert!(vars.contains("y"));
        assert!(vars.contains("z"));
        assert!(vars.contains("flag"));
    }

    #[test]
    fn parser_rejects_oversized_and_deep_expressions() {
        assert!(parse_expr(&"x".repeat(MAX_EXPRESSION_BYTES + 1)).is_err());
        let deeply_nested = format!(
            "{}x{}",
            "(".repeat(MAX_EXPRESSION_DEPTH + 1),
            ")".repeat(MAX_EXPRESSION_DEPTH + 1)
        );
        assert!(parse_expr(&deeply_nested).is_err());
        assert!(parse_expr(&vec!["x"; MAX_EXPRESSION_TOKENS + 1].join(" + ")).is_err());
    }
}
