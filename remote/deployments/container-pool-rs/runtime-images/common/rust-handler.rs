use std::{
    env,
    io::{self, Read},
};

fn main() {
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let runtime = env::var("DD_POOL_RUNTIME").unwrap_or_else(|_| "rust".to_string());
    print!(
        "{{\"ok\":true,\"runtime\":\"{}\",\"receivedBytes\":{}",
        escape_json(&runtime),
        input.len()
    );
    if let Some(expr) =
        json_string_value(&input, "expr").or_else(|| json_string_value(&input, "expression"))
    {
        print!(",\"expr\":\"{}\"", escape_json(&expr));
        match evaluate_expression(&expr) {
            Ok(answer) => print!(",\"answer\":{}", answer),
            Err(error) => print!(",\"error\":\"{}\"", escape_json(&error)),
        }
    }
    println!("}}");
}

fn json_string_value(input: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let bytes = input.as_bytes();
    let mut offset = 0;
    while let Some(relative) = input[offset..].find(&needle) {
        let mut index = offset + relative + needle.len();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b':' {
            offset = index.saturating_add(1);
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'"' {
            offset = index.saturating_add(1);
            continue;
        }
        index += 1;
        let mut value = String::new();
        while index < bytes.len() {
            match bytes[index] {
                b'"' => return Some(value),
                b'\\' if index + 1 < bytes.len() => {
                    index += 1;
                    value.push(match bytes[index] {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        other => other as char,
                    });
                }
                other => value.push(other as char),
            }
            index += 1;
        }
        offset = index;
    }
    None
}

fn evaluate_expression(expr: &str) -> Result<i64, String> {
    let bytes = expr.as_bytes();
    let (left, mut index) = parse_int(bytes, skip_ws(bytes, 0))?;
    index = skip_ws(bytes, index);
    if index >= bytes.len() {
        return Err("missing operator".to_string());
    }
    let operator = bytes[index] as char;
    if !matches!(operator, '+' | '-' | '*' | '/') {
        return Err("unsupported operator".to_string());
    }
    index += 1;
    let (right, mut index) = parse_int(bytes, skip_ws(bytes, index))?;
    index = skip_ws(bytes, index);
    if index != bytes.len() {
        return Err("unsupported expression".to_string());
    }
    match operator {
        '+' => Ok(left + right),
        '-' => Ok(left - right),
        '*' => Ok(left * right),
        '/' if right == 0 => Err("division by zero".to_string()),
        '/' => Ok(left / right),
        _ => Err("unsupported operator".to_string()),
    }
}

fn parse_int(bytes: &[u8], mut index: usize) -> Result<(i64, usize), String> {
    let mut sign = 1_i64;
    if index < bytes.len() && bytes[index] == b'-' {
        sign = -1;
        index += 1;
    }
    let start = index;
    let mut value = 0_i64;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        value = value
            .checked_mul(10)
            .and_then(|next| next.checked_add((bytes[index] - b'0') as i64))
            .ok_or_else(|| "integer overflow".to_string())?;
        index += 1;
    }
    if index == start {
        return Err("expected integer".to_string());
    }
    Ok((value * sign, index))
}

fn skip_ws(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push(' '),
            ch => escaped.push(ch),
        }
    }
    escaped
}
