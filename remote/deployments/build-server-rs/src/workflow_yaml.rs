//! A bounded YAML reader for GitHub Actions workflow files.
//!
//! The build server intentionally keeps its locked dependency graph stable. This
//! module implements the workflow-shaped YAML subset needed by the planner:
//! indentation-based mappings and sequences, flow collections, quoted/plain
//! scalars, comments, and literal/folded block strings. It rejects aliases,
//! anchors, tags, duplicate keys, tabs, multiple documents, and excessive input
//! rather than silently accepting syntax it cannot interpret safely.

use std::error::Error;
use std::fmt::{Display, Formatter};

use serde_json::{Map, Number, Value};

const MAX_YAML_BYTES: usize = 1024 * 1024;
const MAX_YAML_LINES: usize = 20_000;
const MAX_YAML_DEPTH: usize = 64;
const MAX_COLLECTION_ITEMS: usize = 20_000;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct YamlError {
    line: usize,
    message: String,
}

impl YamlError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl Display for YamlError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            formatter.write_str(&self.message)
        } else {
            write!(formatter, "line {}: {}", self.line, self.message)
        }
    }
}

impl Error for YamlError {}

pub(crate) fn parse_yaml(input: &str) -> Result<Value, YamlError> {
    if input.len() > MAX_YAML_BYTES {
        return Err(YamlError::new(
            0,
            format!("workflow exceeds the {MAX_YAML_BYTES}-byte parser limit"),
        ));
    }
    if input.contains('\0') {
        return Err(YamlError::new(0, "workflow contains a NUL byte"));
    }
    if input.contains('\t') {
        return Err(YamlError::new(
            0,
            "tabs are not accepted in workflow YAML; use spaces for indentation",
        ));
    }

    let normalized = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .replace("\r\n", "\n");
    let lines = normalized
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > MAX_YAML_LINES {
        return Err(YamlError::new(
            0,
            format!("workflow exceeds the {MAX_YAML_LINES}-line parser limit"),
        ));
    }

    let mut parser = Parser { lines, index: 0 };
    parser.skip_ignorable();
    let Some(first) = parser.peek_line()? else {
        return Err(YamlError::new(0, "workflow YAML is empty"));
    };
    if first.indent != 0 {
        return Err(YamlError::new(
            first.number,
            "the root workflow mapping must start at indentation zero",
        ));
    }
    let value = parser.parse_node(0, 0)?;
    parser.skip_ignorable();
    if let Some(line) = parser.peek_line()? {
        return Err(YamlError::new(
            line.number,
            "unexpected content after the root workflow document",
        ));
    }
    Ok(value)
}

struct Parser {
    lines: Vec<String>,
    index: usize,
}

#[derive(Debug, Clone)]
struct ParsedLine {
    number: usize,
    indent: usize,
    content: String,
}

impl Parser {
    fn skip_ignorable(&mut self) {
        while self.index < self.lines.len() {
            let raw = &self.lines[self.index];
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn peek_line(&self) -> Result<Option<ParsedLine>, YamlError> {
        let Some(raw) = self.lines.get(self.index) else {
            return Ok(None);
        };
        let indent = leading_spaces(raw);
        let content = strip_comment(&raw[indent..]).trim_end().to_owned();
        let trimmed = content.trim();
        if matches!(trimmed, "---" | "...") || trimmed.starts_with("%YAML") {
            return Err(YamlError::new(
                self.index + 1,
                "multiple YAML documents and directives are not supported",
            ));
        }
        Ok(Some(ParsedLine {
            number: self.index + 1,
            indent,
            content,
        }))
    }

    fn next_significant(&self) -> Result<Option<ParsedLine>, YamlError> {
        let mut index = self.index;
        while let Some(raw) = self.lines.get(index) {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                index += 1;
                continue;
            }
            let indent = leading_spaces(raw);
            let content = strip_comment(&raw[indent..]).trim_end().to_owned();
            let visible = content.trim();
            if matches!(visible, "---" | "...") || visible.starts_with("%YAML") {
                return Err(YamlError::new(
                    index + 1,
                    "multiple YAML documents and directives are not supported",
                ));
            }
            return Ok(Some(ParsedLine {
                number: index + 1,
                indent,
                content,
            }));
        }
        Ok(None)
    }

    fn parse_node(&mut self, indent: usize, depth: usize) -> Result<Value, YamlError> {
        if depth > MAX_YAML_DEPTH {
            return Err(YamlError::new(
                self.index + 1,
                format!("workflow nesting exceeds {MAX_YAML_DEPTH} levels"),
            ));
        }
        self.skip_ignorable();
        let line = self
            .peek_line()?
            .ok_or_else(|| YamlError::new(self.index + 1, "expected a YAML value"))?;
        if line.indent != indent {
            return Err(YamlError::new(
                line.number,
                format!("unexpected indentation {}; expected {indent}", line.indent),
            ));
        }
        if sequence_item_text(&line.content).is_some() {
            self.parse_sequence(indent, depth)
        } else {
            self.parse_mapping(indent, depth)
        }
    }

    fn parse_mapping(&mut self, indent: usize, depth: usize) -> Result<Value, YamlError> {
        let mut object = Map::new();
        while object.len() < MAX_COLLECTION_ITEMS {
            self.skip_ignorable();
            let Some(line) = self.peek_line()? else {
                break;
            };
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return Err(YamlError::new(
                    line.number,
                    format!(
                        "unexpected indentation {}; expected mapping key at {indent}",
                        line.indent
                    ),
                ));
            }
            if sequence_item_text(&line.content).is_some() {
                break;
            }

            let (key, remainder) = split_mapping_entry(&line.content).ok_or_else(|| {
                YamlError::new(line.number, "expected a mapping entry in `key: value` form")
            })?;
            let key = parse_key(key, line.number)?;
            if key == "<<" {
                return Err(YamlError::new(
                    line.number,
                    "YAML merge keys are not supported",
                ));
            }
            if object.contains_key(&key) {
                return Err(YamlError::new(
                    line.number,
                    format!("duplicate mapping key {key:?}"),
                ));
            }

            self.index += 1;
            let value = self.parse_entry_value(indent, remainder, line.number, depth + 1)?;
            object.insert(key, value);
        }
        if object.len() >= MAX_COLLECTION_ITEMS {
            return Err(YamlError::new(
                self.index + 1,
                format!("mapping exceeds {MAX_COLLECTION_ITEMS} entries"),
            ));
        }
        if object.is_empty() {
            return Err(YamlError::new(
                self.index + 1,
                "expected a workflow mapping",
            ));
        }
        Ok(Value::Object(object))
    }

    fn parse_sequence(&mut self, indent: usize, depth: usize) -> Result<Value, YamlError> {
        let mut values = Vec::new();
        while values.len() < MAX_COLLECTION_ITEMS {
            self.skip_ignorable();
            let Some(line) = self.peek_line()? else {
                break;
            };
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return Err(YamlError::new(
                    line.number,
                    format!(
                        "unexpected indentation {}; expected sequence item at {indent}",
                        line.indent
                    ),
                ));
            }
            let Some(item_text) = sequence_item_text(&line.content) else {
                break;
            };
            self.index += 1;
            let item_text = item_text.trim();
            let value = if item_text.is_empty() {
                self.parse_optional_child(indent, depth + 1)?
                    .unwrap_or(Value::Null)
            } else if let Some((key, remainder)) = split_mapping_entry(item_text) {
                self.parse_sequence_mapping_item(indent, key, remainder, line.number, depth + 1)?
            } else if let Some(indicator) = block_indicator(item_text) {
                Value::String(self.parse_block_scalar(indent, indicator, line.number)?)
            } else {
                parse_inline_value(item_text, line.number)?
            };
            values.push(value);
        }
        if values.len() >= MAX_COLLECTION_ITEMS {
            return Err(YamlError::new(
                self.index + 1,
                format!("sequence exceeds {MAX_COLLECTION_ITEMS} items"),
            ));
        }
        Ok(Value::Array(values))
    }

    fn parse_sequence_mapping_item(
        &mut self,
        sequence_indent: usize,
        first_key: &str,
        first_remainder: &str,
        line_number: usize,
        depth: usize,
    ) -> Result<Value, YamlError> {
        let mapping_indent = sequence_indent
            .checked_add(2)
            .ok_or_else(|| YamlError::new(line_number, "mapping indentation overflow"))?;
        let mut object = Map::new();
        let key = parse_key(first_key, line_number)?;
        if key == "<<" {
            return Err(YamlError::new(
                line_number,
                "YAML merge keys are not supported",
            ));
        }
        let value =
            self.parse_entry_value(mapping_indent, first_remainder, line_number, depth + 1)?;
        object.insert(key, value);

        while object.len() < MAX_COLLECTION_ITEMS {
            self.skip_ignorable();
            let Some(line) = self.peek_line()? else {
                break;
            };
            if line.indent <= sequence_indent {
                break;
            }
            if line.indent != mapping_indent {
                return Err(YamlError::new(
                    line.number,
                    format!("sequence mapping fields must be indented {mapping_indent} spaces"),
                ));
            }
            if sequence_item_text(&line.content).is_some() {
                return Err(YamlError::new(
                    line.number,
                    "unexpected nested sequence where a mapping field was expected",
                ));
            }
            let (raw_key, remainder) = split_mapping_entry(&line.content).ok_or_else(|| {
                YamlError::new(line.number, "expected a mapping entry in `key: value` form")
            })?;
            let key = parse_key(raw_key, line.number)?;
            if key == "<<" {
                return Err(YamlError::new(
                    line.number,
                    "YAML merge keys are not supported",
                ));
            }
            if object.contains_key(&key) {
                return Err(YamlError::new(
                    line.number,
                    format!("duplicate mapping key {key:?}"),
                ));
            }
            self.index += 1;
            let value =
                self.parse_entry_value(mapping_indent, remainder, line.number, depth + 1)?;
            object.insert(key, value);
        }
        if object.len() >= MAX_COLLECTION_ITEMS {
            return Err(YamlError::new(
                self.index + 1,
                format!("mapping exceeds {MAX_COLLECTION_ITEMS} entries"),
            ));
        }
        Ok(Value::Object(object))
    }

    fn parse_entry_value(
        &mut self,
        container_indent: usize,
        remainder: &str,
        line_number: usize,
        depth: usize,
    ) -> Result<Value, YamlError> {
        let remainder = remainder.trim();
        if remainder.is_empty() {
            return Ok(self
                .parse_optional_child(container_indent, depth)?
                .unwrap_or(Value::Null));
        }
        if let Some(indicator) = block_indicator(remainder) {
            return Ok(Value::String(self.parse_block_scalar(
                container_indent,
                indicator,
                line_number,
            )?));
        }
        parse_inline_value(remainder, line_number)
    }

    fn parse_optional_child(
        &mut self,
        parent_indent: usize,
        depth: usize,
    ) -> Result<Option<Value>, YamlError> {
        let Some(next) = self.next_significant()? else {
            return Ok(None);
        };
        if next.indent <= parent_indent {
            return Ok(None);
        }
        self.skip_ignorable();
        self.parse_node(next.indent, depth).map(Some)
    }

    fn parse_block_scalar(
        &mut self,
        parent_indent: usize,
        indicator: BlockIndicator,
        line_number: usize,
    ) -> Result<String, YamlError> {
        let start = self.index;
        let mut end = start;
        let mut minimum_indent: Option<usize> = None;

        while let Some(raw) = self.lines.get(end) {
            if raw.trim().is_empty() {
                end += 1;
                continue;
            }
            let indent = leading_spaces(raw);
            if indent <= parent_indent {
                break;
            }
            minimum_indent = Some(minimum_indent.map_or(indent, |current| current.min(indent)));
            end += 1;
        }

        let Some(block_indent) = minimum_indent else {
            self.index = end;
            return match indicator.chomp {
                Chomp::Keep => Ok("\n".to_owned()),
                Chomp::Clip | Chomp::Strip => Ok(String::new()),
            };
        };
        if block_indent <= parent_indent {
            return Err(YamlError::new(
                line_number,
                "block scalar content must be indented below its key",
            ));
        }

        let mut block_lines = Vec::with_capacity(end.saturating_sub(start));
        for raw in &self.lines[start..end] {
            if raw.trim().is_empty() {
                block_lines.push(String::new());
            } else {
                let content = raw.get(block_indent..).ok_or_else(|| {
                    YamlError::new(line_number, "invalid UTF-8 block scalar indentation")
                })?;
                block_lines.push(content.to_owned());
            }
        }
        self.index = end;

        let mut value = match indicator.style {
            BlockStyle::Literal => block_lines.join("\n"),
            BlockStyle::Folded => fold_block_lines(&block_lines),
        };
        match indicator.chomp {
            Chomp::Strip => {
                while value.ends_with('\n') {
                    value.pop();
                }
            }
            Chomp::Clip => {
                while value.ends_with('\n') {
                    value.pop();
                }
                if !value.is_empty() {
                    value.push('\n');
                }
            }
            Chomp::Keep => value.push('\n'),
        }
        Ok(value)
    }
}

#[derive(Clone, Copy)]
struct BlockIndicator {
    style: BlockStyle,
    chomp: Chomp,
}

#[derive(Clone, Copy)]
enum BlockStyle {
    Literal,
    Folded,
}

#[derive(Clone, Copy)]
enum Chomp {
    Clip,
    Strip,
    Keep,
}

fn block_indicator(value: &str) -> Option<BlockIndicator> {
    let value = value.trim();
    let (style, suffix) = match value.as_bytes().first().copied() {
        Some(b'|') => (BlockStyle::Literal, &value[1..]),
        Some(b'>') => (BlockStyle::Folded, &value[1..]),
        _ => return None,
    };
    let chomp = match suffix.trim() {
        "" => Chomp::Clip,
        "-" => Chomp::Strip,
        "+" => Chomp::Keep,
        _ => return None,
    };
    Some(BlockIndicator { style, chomp })
}

fn sequence_item_text(content: &str) -> Option<&str> {
    let trimmed = content.trim_end();
    let rest = trimmed.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn split_mapping_entry(content: &str) -> Option<(&str, &str)> {
    let mut quote = None;
    let mut escaped = false;
    let mut square_depth = 0_usize;
    let mut curly_depth = 0_usize;

    for (index, character) in content.char_indices() {
        if let Some(active_quote) = quote {
            if active_quote == '"' && escaped {
                escaped = false;
                continue;
            }
            if active_quote == '"' && character == '\\' {
                escaped = true;
                continue;
            }
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' => square_depth = square_depth.saturating_add(1),
            ']' => square_depth = square_depth.saturating_sub(1),
            '{' => curly_depth = curly_depth.saturating_add(1),
            '}' => curly_depth = curly_depth.saturating_sub(1),
            ':' if square_depth == 0 && curly_depth == 0 => {
                let after = &content[index + 1..];
                if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let key = content[..index].trim();
                    if !key.is_empty() {
                        return Some((key, after));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_key(raw: &str, line: usize) -> Result<String, YamlError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('?') {
        return Err(YamlError::new(
            line,
            "complex or empty mapping keys are not supported",
        ));
    }
    let value = parse_inline_value(raw, line)?;
    match value {
        Value::String(key) if !key.is_empty() => Ok(key),
        Value::String(_) => Err(YamlError::new(line, "mapping key cannot be empty")),
        _ => Err(YamlError::new(
            line,
            "workflow mapping keys must be strings",
        )),
    }
}

fn parse_inline_value(raw: &str, line: usize) -> Result<Value, YamlError> {
    let raw = strip_comment(raw).trim();
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    if !matches!(raw.chars().next(), Some('[' | '{' | '"' | '\'')) {
        return parse_plain_scalar(raw, line);
    }
    let mut parser = FlowParser {
        input: raw,
        position: 0,
        line,
    };
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if !parser.is_end() {
        return Err(YamlError::new(
            line,
            format!(
                "unexpected trailing flow content near {:?}",
                parser.remaining()
            ),
        ));
    }
    Ok(value)
}

struct FlowParser<'a> {
    input: &'a str,
    position: usize,
    line: usize,
}

impl<'a> FlowParser<'a> {
    fn parse_value(&mut self) -> Result<Value, YamlError> {
        self.skip_whitespace();
        match self.peek_char() {
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some('"') => self.parse_double_quoted().map(Value::String),
            Some('\'') => self.parse_single_quoted().map(Value::String),
            Some(_) => self.parse_plain(),
            None => Err(YamlError::new(self.line, "expected a scalar value")),
        }
    }

    fn parse_array(&mut self) -> Result<Value, YamlError> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        loop {
            self.skip_whitespace();
            if self.consume_char(']') {
                break;
            }
            if values.len() >= MAX_COLLECTION_ITEMS {
                return Err(YamlError::new(
                    self.line,
                    format!("flow sequence exceeds {MAX_COLLECTION_ITEMS} items"),
                ));
            }
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume_char(']') {
                break;
            }
            self.expect_char(',')?;
        }
        Ok(Value::Array(values))
    }

    fn parse_object(&mut self) -> Result<Value, YamlError> {
        self.expect_char('{')?;
        let mut object = Map::new();
        loop {
            self.skip_whitespace();
            if self.consume_char('}') {
                break;
            }
            if object.len() >= MAX_COLLECTION_ITEMS {
                return Err(YamlError::new(
                    self.line,
                    format!("flow mapping exceeds {MAX_COLLECTION_ITEMS} entries"),
                ));
            }
            let key = self.parse_flow_key()?;
            self.skip_whitespace();
            self.expect_char(':')?;
            let value = self.parse_value()?;
            if object.insert(key.clone(), value).is_some() {
                return Err(YamlError::new(
                    self.line,
                    format!("duplicate flow mapping key {key:?}"),
                ));
            }
            self.skip_whitespace();
            if self.consume_char('}') {
                break;
            }
            self.expect_char(',')?;
        }
        Ok(Value::Object(object))
    }

    fn parse_flow_key(&mut self) -> Result<String, YamlError> {
        self.skip_whitespace();
        match self.peek_char() {
            Some('"') => self.parse_double_quoted(),
            Some('\'') => self.parse_single_quoted(),
            Some(_) => {
                let start = self.position;
                while let Some(character) = self.peek_char() {
                    if character == ':' {
                        break;
                    }
                    if matches!(character, ',' | '{' | '}' | '[' | ']') {
                        return Err(YamlError::new(
                            self.line,
                            "invalid character in flow mapping key",
                        ));
                    }
                    self.bump_char();
                }
                let key = self.input[start..self.position].trim();
                if key.is_empty() {
                    Err(YamlError::new(
                        self.line,
                        "flow mapping key cannot be empty",
                    ))
                } else {
                    Ok(key.to_owned())
                }
            }
            None => Err(YamlError::new(self.line, "unterminated flow mapping")),
        }
    }

    fn parse_double_quoted(&mut self) -> Result<String, YamlError> {
        let start = self.position;
        self.expect_char('"')?;
        let mut escaped = false;
        while let Some(character) = self.bump_char() {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == '"' {
                let encoded = &self.input[start..self.position];
                return serde_json::from_str::<String>(encoded).map_err(|error| {
                    YamlError::new(
                        self.line,
                        format!("unsupported double-quoted escape: {error}"),
                    )
                });
            }
        }
        Err(YamlError::new(
            self.line,
            "unterminated double-quoted scalar",
        ))
    }

    fn parse_single_quoted(&mut self) -> Result<String, YamlError> {
        self.expect_char('\'')?;
        let mut value = String::new();
        loop {
            let Some(character) = self.bump_char() else {
                return Err(YamlError::new(
                    self.line,
                    "unterminated single-quoted scalar",
                ));
            };
            if character == '\'' {
                if self.consume_char('\'') {
                    value.push('\'');
                    continue;
                }
                return Ok(value);
            }
            value.push(character);
        }
    }

    fn parse_plain(&mut self) -> Result<Value, YamlError> {
        let start = self.position;
        let mut expression_braces = 0_usize;
        while let Some(character) = self.peek_char() {
            match character {
                '{' if self.input[start..self.position].trim_end().ends_with('$')
                    || expression_braces > 0 =>
                {
                    expression_braces = expression_braces.saturating_add(1);
                    self.bump_char();
                }
                '}' if expression_braces > 0 => {
                    expression_braces = expression_braces.saturating_sub(1);
                    self.bump_char();
                }
                ',' | ']' | '}' if expression_braces == 0 => break,
                _ => {
                    self.bump_char();
                }
            }
        }
        let value = self.input[start..self.position].trim();
        parse_plain_scalar(value, self.line)
    }

    fn skip_whitespace(&mut self) {
        while self.peek_char().is_some_and(char::is_whitespace) {
            self.bump_char();
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), YamlError> {
        if self.consume_char(expected) {
            Ok(())
        } else {
            Err(YamlError::new(
                self.line,
                format!("expected {expected:?} near {:?}", self.remaining()),
            ))
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.bump_char();
            true
        } else {
            false
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let character = self.peek_char()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn remaining(&self) -> &str {
        &self.input[self.position..]
    }

    fn is_end(&self) -> bool {
        self.position == self.input.len()
    }
}

fn parse_plain_scalar(value: &str, line: usize) -> Result<Value, YamlError> {
    if value.is_empty() {
        return Err(YamlError::new(line, "plain scalar cannot be empty"));
    }
    if value.starts_with('&') || value.starts_with('*') || value.starts_with('!') {
        return Err(YamlError::new(
            line,
            "YAML anchors, aliases, and tags are not supported",
        ));
    }

    match value {
        "~" | "null" | "Null" | "NULL" => return Ok(Value::Null),
        "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        _ => {}
    }

    if looks_like_integer(value) {
        if let Ok(number) = value.parse::<i64>() {
            return Ok(Value::Number(Number::from(number)));
        }
        if let Ok(number) = value.parse::<u64>() {
            return Ok(Value::Number(Number::from(number)));
        }
    }
    if looks_like_float(value) {
        if let Ok(number) = value.parse::<f64>() {
            if let Some(number) = Number::from_f64(number) {
                return Ok(Value::Number(number));
            }
        }
    }
    Ok(Value::String(value.to_owned()))
}

fn looks_like_integer(value: &str) -> bool {
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    if unsigned.is_empty() || !unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    unsigned == "0" || !unsigned.starts_with('0')
}

fn looks_like_float(value: &str) -> bool {
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    !unsigned.is_empty()
        && (unsigned.contains('.') || unsigned.contains('e') || unsigned.contains('E'))
        && unsigned
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-'))
}

fn leading_spaces(value: &str) -> usize {
    value.bytes().take_while(|byte| *byte == b' ').count()
}

fn strip_comment(value: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if let Some(active_quote) = quote {
            if active_quote == '"' && escaped {
                escaped = false;
                continue;
            }
            if active_quote == '"' && character == '\\' {
                escaped = true;
                continue;
            }
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '#' if index == 0 || value[..index].ends_with(char::is_whitespace) => {
                return &value[..index]
            }
            _ => {}
        }
    }
    value
}

fn fold_block_lines(lines: &[String]) -> String {
    let mut value = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            let previous_blank = lines[index - 1].is_empty();
            if line.is_empty() || previous_blank {
                value.push('\n');
            } else {
                value.push(' ');
            }
        }
        value.push_str(line);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_workflow_shaped_mappings_sequences_and_flow_values() {
        let yaml = r#"
name: CI
on:
  push:
  pull_request:
jobs:
  build:
    runs-on: [self-hosted, linux, x64]
    strategy:
      fail-fast: false
      matrix:
        rust: [stable, beta]
        include:
          - rust: nightly
            experimental: true
    env: { RUST_BACKTRACE: 1, FEATURE: "voice" }
    steps:
      - uses: actions/checkout@v4
      - name: Test
        if: ${{ !cancelled() }}
        run: cargo test --locked
"#;
        let parsed = parse_yaml(yaml).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(parsed["name"], json!("CI"));
        assert_eq!(parsed["jobs"]["build"]["runs-on"][1], json!("linux"));
        assert_eq!(
            parsed["jobs"]["build"]["strategy"]["matrix"]["include"][0]["experimental"],
            json!(true)
        );
        assert_eq!(parsed["jobs"]["build"]["env"]["RUST_BACKTRACE"], json!(1));
    }

    #[test]
    fn parses_literal_and_folded_block_scalars() {
        let yaml = r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: Literal
        run: |
          cargo test --locked
          cargo clippy --locked
      - name: Folded
        run: >-
          echo one
          echo two
"#;
        let parsed = parse_yaml(yaml).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(
            parsed["jobs"]["test"]["steps"][0]["run"],
            json!("cargo test --locked\ncargo clippy --locked\n")
        );
        assert_eq!(
            parsed["jobs"]["test"]["steps"][1]["run"],
            json!("echo one echo two")
        );
    }

    #[test]
    fn rejects_duplicate_keys_aliases_tabs_and_bad_indentation() {
        assert!(parse_yaml("jobs: {}\njobs: {}\n").is_err());
        assert!(parse_yaml("jobs: *shared\n").is_err());
        assert!(parse_yaml("jobs:\n\tbuild: {}\n").is_err());
        assert!(parse_yaml("jobs:\n   build:\n      runs-on: linux\n    broken: true\n").is_err());
    }

    #[test]
    fn preserves_expression_and_url_plain_scalars() {
        let yaml = r#"
value: ${{ fromJSON(inputs.matrix) }}
url: https://github.com/actions/checkout
hash: abc#def
commented: value # ignored
components: rustfmt, clippy
"#;
        let parsed = parse_yaml(yaml).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(parsed["value"], json!("${{ fromJSON(inputs.matrix) }}"));
        assert_eq!(parsed["url"], json!("https://github.com/actions/checkout"));
        assert_eq!(parsed["hash"], json!("abc#def"));
        assert_eq!(parsed["commented"], json!("value"));
        assert_eq!(parsed["components"], json!("rustfmt, clippy"));
    }
}
