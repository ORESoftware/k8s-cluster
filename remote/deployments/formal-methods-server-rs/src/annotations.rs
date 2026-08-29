use crate::expr::SortHint;

// ---------------------------------------------------------------------------
// annotation extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct AnnotationBlock {
    pub(crate) file: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) decls: Vec<VarDecl>,
    pub(crate) assumes: Vec<AnnotatedExpr>,
    pub(crate) requires: Vec<AnnotatedExpr>,
    pub(crate) ensures: Vec<AnnotatedExpr>,
    pub(crate) invariants: Vec<AnnotatedExpr>,
    pub(crate) variants: Vec<AnnotatedExpr>,
    pub(crate) asserts: Vec<AnnotatedExpr>,
}

#[derive(Debug, Clone)]
pub(crate) struct VarDecl {
    pub(crate) name: String,
    pub(crate) sort: SortHint,
    #[allow(dead_code)]
    pub(crate) line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct AnnotatedExpr {
    pub(crate) raw: String,
    pub(crate) line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedSource {
    pub(crate) file: String,
    pub(crate) blocks: Vec<AnnotationBlock>,
    pub(crate) plain_lines: Vec<(usize, String)>,
}

fn strip_comment_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for prefix in ["//", "#", "--", ";;"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn extract_annotation_directive(comment_body: &str) -> Option<(&'static str, &str)> {
    let body = comment_body.trim_start_matches(['/', '*', '!', ' ', '\t']);
    if !body.starts_with('@') {
        return None;
    }
    let rest = &body[1..];
    let directives: &[(&'static str, &'static str)] = &[
        ("var", "var"),
        ("requires", "requires"),
        ("ensures", "ensures"),
        ("assume", "assume"),
        ("invariant", "invariant"),
        ("variant", "variant"),
        ("assert", "assert"),
    ];
    for (kind, kw) in directives {
        if let Some(after) = rest.strip_prefix(kw) {
            if after.is_empty() {
                return Some((kind, ""));
            }
            let first = after.chars().next().unwrap();
            if first.is_whitespace() || first == ':' {
                return Some((kind, after.trim_start_matches([' ', '\t']).trim()));
            }
        }
    }
    None
}

fn parse_var_decl(body: &str, line: usize) -> Result<VarDecl, String> {
    let (name, sort) = match body.split_once(':') {
        Some((name, sort)) => (name.trim(), sort.trim()),
        None => (body.trim(), "Int"),
    };
    if name.is_empty() {
        return Err("missing variable name in @var".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next().map_or(true, |c| c.is_ascii_digit())
    {
        return Err(format!("invalid @var name {name:?}"));
    }
    let sort = match sort.to_ascii_lowercase().as_str() {
        "int" | "integer" | "i64" | "i32" | "u64" | "u32" => SortHint::Int,
        "real" | "float" | "double" | "f64" | "f32" => SortHint::Real,
        "bool" | "boolean" => SortHint::Bool,
        other => {
            return Err(format!(
                "unsupported @var sort {other:?} (use Int|Real|Bool)"
            ))
        }
    };
    Ok(VarDecl {
        name: name.to_string(),
        sort,
        line,
    })
}

pub(crate) fn parse_annotations(file: &str, content: &str) -> ParsedSource {
    let mut blocks: Vec<AnnotationBlock> = Vec::new();
    let mut plain_lines: Vec<(usize, String)> = Vec::new();
    let mut current: Option<AnnotationBlock> = None;

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let comment = strip_comment_prefix(line);
        let directive = comment.and_then(extract_annotation_directive);

        if let Some((kind, body)) = directive {
            let block = current.get_or_insert_with(|| AnnotationBlock {
                file: file.to_string(),
                start_line: line_no,
                end_line: line_no,
                decls: Vec::new(),
                assumes: Vec::new(),
                requires: Vec::new(),
                ensures: Vec::new(),
                invariants: Vec::new(),
                variants: Vec::new(),
                asserts: Vec::new(),
            });
            block.end_line = line_no;
            match kind {
                "var" => match parse_var_decl(body, line_no) {
                    Ok(decl) => block.decls.push(decl),
                    Err(err) => {
                        plain_lines.push((line_no, format!("@var parse error: {err}")));
                    }
                },
                "requires" => block.requires.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "ensures" => block.ensures.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "assume" => block.assumes.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "invariant" => block.invariants.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "variant" => block.variants.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "assert" => block.asserts.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                _ => {}
            }
        } else if comment.is_some() {
            // A comment line with no @-directive (a blank `//`, a `// some prose`,
            // a `# ----`, etc.) does NOT close the current annotation block — it
            // is part of the same visual span. Only a non-comment line ends the
            // block.
            if let Some(block) = current.as_mut() {
                block.end_line = line_no;
            } else {
                plain_lines.push((line_no, line.to_string()));
            }
        } else {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            plain_lines.push((line_no, line.to_string()));
        }
    }
    if let Some(block) = current.take() {
        blocks.push(block);
    }
    ParsedSource {
        file: file.to_string(),
        blocks,
        plain_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_extractor_keeps_block_across_blank_comment_lines() {
        let src = "\
// @var x: Int
// @requires x > 0
// @assume x < 100
//
// @ensures x + 1 > 0
fn f(x: i64) -> i64 { x + 1 }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1, "blank `//` must not split a block");
        let block = &parsed.blocks[0];
        assert_eq!(block.decls.len(), 1);
        assert_eq!(block.requires.len(), 1);
        assert_eq!(block.assumes.len(), 1);
        assert_eq!(block.ensures.len(), 1);
    }

    #[test]
    fn annotation_extractor_keeps_block_across_prose_comment_lines() {
        let src = "\
// @var x: Int
// @requires x > 0
// Some explanatory prose between requires and ensures.
// @ensures x + 1 > 0
fn f(x: i64) -> i64 { x + 1 }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].ensures.len(), 1);
    }

    #[test]
    fn annotation_extractor_groups_block() {
        let src = "\
// @var x: Int
// @var y: Int
// @requires x > 0
// @requires y >= x
// @ensures x + y > 0
fn add(x: i64, y: i64) -> i64 { x + y }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1);
        let block = &parsed.blocks[0];
        assert_eq!(block.decls.len(), 2);
        assert_eq!(block.requires.len(), 2);
        assert_eq!(block.ensures.len(), 1);
    }
}
