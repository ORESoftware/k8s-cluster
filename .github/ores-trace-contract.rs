//! ORES static trace-marker contract checker.
//!
//! Compatibility contract:
//!   * static trace/routine identifiers use the `ores-trace-` / `ores-routine-`
//!     prefixes and a 12..=64 character `[A-Za-z0-9_-]` suffix;
//!   * new identifiers minted by ORES tooling SHOULD use the canonical
//!     21-character nanoid width, but legacy-compatible widths remain valid;
//!   * trace identifiers stay inline at the call site;
//!   * routine identifiers are declared once per function and passed through
//!     `addRoutineId` / language equivalents;
//!   * the retired `dd-trace-` prefix and `addRoutine` spelling are rejected.
//!
//! The public wire/runtime contract admits 12..=64. Keeping admission wider
//! than the generator width avoids making a CI guard silently redefine the wire
//! contract. The self-tests pin both facts: compatibility bounds and the
//! canonical 21-character generator subset.
//!
//! Single file, no external crates: build with `rustc -O`.
//!
//! ores-trace-contract:ignore-file

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MIN_ID_WIDTH: usize = 12;
const CANONICAL_GENERATOR_WIDTH: usize = 21;
const MAX_ID_WIDTH: usize = 64;

const EXTS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "dart", "go", "java", "ex", "exs",
];

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", "coverage", "vendor", "generated",
    "tests", "test", "testdata", "fixtures", "__tests__", "__mocks__", ".r2g",
];

const TRACE_METHODS: &[&str] = &[
    "addTraceId", "addTrace", "add_trace_id", "add_trace", "AddTraceID", "AddTrace",
];
const ROUTINE_METHODS: &[&str] = &["addRoutineId", "add_routine_id", "AddRoutineID"];

struct Finding {
    file: String,
    line: usize,
    msg: String,
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn id_ok(id: &str, kind: &str) -> bool {
    let prefix = format!("ores-{kind}-");
    match id.strip_prefix(&prefix) {
        Some(rest) => {
            let width = rest.chars().count();
            (MIN_ID_WIDTH..=MAX_ID_WIDTH).contains(&width) && rest.chars().all(is_id_char)
        }
        None => false,
    }
}

fn canonical_generated_id(id: &str, kind: &str) -> bool {
    let prefix = format!("ores-{kind}-");
    match id.strip_prefix(&prefix) {
        Some(rest) => rest.chars().count() == CANONICAL_GENERATOR_WIDTH && rest.chars().all(is_id_char),
        None => false,
    }
}

fn skip_path(p: &Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        SKIP_DIRS.iter().any(|d| s == *d)
    })
}

fn is_test_file(p: &Path) -> bool {
    let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    n.contains(".test.")
        || n.contains(".spec.")
        || n.ends_with("_test.go")
        || n.ends_with("_test.rs")
        || n.ends_with("_test.exs")
        || n.starts_with("test_")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_symlink() {
            continue;
        }
        if p.is_dir() {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn literal_arg(line: &str, at: usize, method: &str) -> Option<Option<String>> {
    let after = line.get(at + 1 + method.len()..)?;
    let rest = after.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let rest = rest.strip_prefix('(')?.trim_start();
    let q = rest.chars().next()?;
    if q == '\'' || q == '"' || q == '`' {
        let inner: String = rest[q.len_utf8()..].chars().take_while(|&c| c != q).collect();
        Some(Some(inner))
    } else {
        Some(None)
    }
}

fn scan_calls(line: &str, methods: &[&str]) -> Vec<(String, Option<String>)> {
    let mut sorted = methods.to_vec();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.len()));
    let mut out = Vec::new();
    for (i, ch) in line.char_indices() {
        if ch != '.' {
            continue;
        }
        if matches!(line[..i].chars().next_back(), Some('\'') | Some('"') | Some('`')) {
            continue;
        }
        for m in &sorted {
            let Some(seg) = line.get(i + 1..i + 1 + m.len()) else { continue };
            if seg != *m {
                continue;
            }
            let next = line[i + 1 + m.len()..].chars().next();
            if matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            if let Some(arg) = literal_arg(line, i, m) {
                out.push((m.to_string(), arg));
            }
            break;
        }
    }
    out
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with('*')
        || t.starts_with("<!--")
        || t.starts_with("--")
        || t.starts_with("# ")
        || t == "#"
}

fn is_pattern_decl(line: &str) -> bool {
    line.contains("@pattern")
        || line.contains("\"pattern\"")
        || line.contains("^ores-")
        || line.contains("[A-Za-z0-9_-]{12,64}")
        || line.contains("[A-Za-z0-9_-]{21}")
}

fn is_matcher(line: &str) -> bool {
    const NEEDLES: &[&str] = &[
        ".contains(", ".includes(", ".matches(", ".match(", ".starts_with(", ".startsWith(",
        ".strip_prefix(", ".test(", ".replace(", "assert!(", "assert_eq!(", "regex", "Regex",
        "RE =", "_RE",
    ];
    NEEDLES.iter().any(|n| line.contains(n))
}

fn marker_literals(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (kind, tag) in [("trace", "ores-trace-"), ("routine", "ores-routine-")] {
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(tag) {
            let start = from + rel;
            let prev = line[..start].chars().next_back();
            if matches!(prev, Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                from = start + tag.len();
                continue;
            }
            let id: String = line[start..].chars().take_while(|&c| is_id_char(c)).collect();
            if id.len() > tag.len() {
                out.push((kind.to_string(), id.clone()));
            }
            from = start + id.len().max(tag.len());
        }
    }
    out
}

fn hoisted_trace_decl(line: &str) -> Option<String> {
    let mut l = line.trim();
    loop {
        let lower = l.to_lowercase();
        let stripped = [
            "pub(crate) ", "pub(super) ", "pub ", "export ", "public ", "private ", "protected ", "declare ",
        ]
        .iter()
        .find(|k| lower.starts_with(*k))
        .map(|k| l[k.len()..].trim_start());
        match stripped {
            Some(next) => l = next,
            None => break,
        }
    }
    let lower = l.to_lowercase();
    if !["const ", "let ", "var ", "static ", "final ", "val "]
        .iter()
        .any(|k| lower.starts_with(k))
        || !lower.contains("trace")
    {
        return None;
    }
    let eq = l.find('=')?;
    let (name, val) = l.split_at(eq);
    if !name.to_lowercase().contains("trace") {
        return None;
    }
    let quoted = val.contains("\"ores-trace-") || val.contains("'ores-trace-") || val.contains("`ores-trace-");
    if quoted && marker_literals(val).iter().any(|(k, _)| k == "trace") {
        return Some(name.trim().to_string());
    }
    None
}

fn next_is_mod(lines: &[&str], idx: usize) -> bool {
    for l in lines.iter().skip(idx + 1) {
        let t = l.trim_start();
        if t.is_empty() || t.starts_with("#[") || t.starts_with("//") {
            continue;
        }
        return t.starts_with("mod ") || t.starts_with("pub mod ");
    }
    false
}

fn check_line(file: &str, no: usize, line: &str, findings: &mut Vec<Finding>) {
    if is_comment(line) {
        return;
    }
    let matcher = is_matcher(line);
    let push = |findings: &mut Vec<Finding>, msg: String| {
        findings.push(Finding { file: file.to_string(), line: no, msg })
    };

    if !matcher && line.contains(concat!("dd", "-trace-")) {
        push(findings, "legacy dd-trace-* marker; use ores-trace-*".into());
    }
    if line.contains(".addRoutine(") || line.contains(".add_routine(") {
        push(findings, "use addRoutineId()/add_routine_id(), not addRoutine()".into());
    }

    for (m, arg) in scan_calls(line, TRACE_METHODS) {
        if let Some(id) = arg {
            if !id_ok(&id, "trace") {
                push(findings, format!(
                    ".{m}(\"{id}\") is not a compatible static marker; expected ^ores-trace-[A-Za-z0-9_-]{{12,64}}$"
                ));
            }
        }
    }
    for (m, arg) in scan_calls(line, ROUTINE_METHODS) {
        if let Some(id) = arg {
            if !id_ok(&id, "routine") {
                push(findings, format!(
                    ".{m}(\"{id}\") is not a compatible routine id; expected ^ores-routine-[A-Za-z0-9_-]{{12,64}}$"
                ));
            }
        }
    }

    if let Some(rest) = hoisted_trace_decl(line).filter(|_| !matcher) {
        push(findings, format!("static ores-trace-* id must stay inline at the call site, not in `{rest}`"));
    }

    if !matcher && !is_pattern_decl(line) {
        let already: Vec<String> = findings
            .iter()
            .filter(|x| x.line == no && x.file == file)
            .map(|x| x.msg.clone())
            .collect();
        for (kind, id) in marker_literals(line) {
            if already.iter().any(|m| m.contains(&format!("\"{id}\""))) {
                continue;
            }
            if !id_ok(&id, &kind) {
                push(findings, format!(
                    "\"{id}\" does not match ^ores-{kind}-[A-Za-z0-9_-]{{12,64}}$"
                ));
            }
        }
    }
}

fn vacuous_scan(root: &Path, checked: usize) -> Option<String> {
    if !root.is_dir() {
        return Some(format!("root {} is not a directory", root.display()));
    }
    if checked == 0 {
        return Some(format!(
            "no source files were checked under {}; refusing to report a scan that covered nothing",
            root.display()
        ));
    }
    None
}

fn main() -> ExitCode {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".to_string()));
    let mut files = Vec::new();
    walk(&root, &mut files);

    let mut findings = Vec::new();
    let mut checked = 0usize;
    let mut markers = 0usize;

    for p in files {
        let Some(ext) = p.extension().map(|e| e.to_string_lossy().to_lowercase()) else { continue };
        if !EXTS.contains(&ext.as_str()) || skip_path(&p) || is_test_file(&p) {
            continue;
        }
        let rel = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
        let Ok(text) = fs::read_to_string(&p) else {
            findings.push(Finding {
                file: rel,
                line: 0,
                msg: "source file could not be read as UTF-8, so it was not checked".to_string(),
            });
            continue;
        };
        if text.contains("ores-trace-contract:ignore-file") {
            continue;
        }
        checked += 1;
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if ext == "rs" && line.trim_start().starts_with("#[cfg(test)]") && next_is_mod(&lines, i) {
                break;
            }
            if line.contains("ores-trace-") || line.contains("ores-routine-") {
                markers += 1;
            }
            check_line(&rel, i + 1, line, &mut findings);
        }
    }

    if let Some(reason) = vacuous_scan(&root, checked) {
        eprintln!("ores-trace-contract: REFUSED -- {reason}");
        return ExitCode::FAILURE;
    }
    if findings.is_empty() {
        println!("ores-trace-contract: OK -- {checked} source files checked, {markers} marker lines, 0 violations");
        return ExitCode::SUCCESS;
    }
    eprintln!("ores-trace-contract: {} violation(s)", findings.len());
    for f in &findings {
        eprintln!("  {}:{}: {}", f.file, f.line, f.msg);
    }
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(kind: &str, width: usize) -> String {
        format!("ores-{kind}-{}", "A".repeat(width))
    }

    fn msgs(line: &str) -> Vec<String> {
        let mut f = Vec::new();
        check_line("x.rs", 1, line, &mut f);
        f.into_iter().map(|x| x.msg).collect()
    }

    #[test]
    fn compatibility_boundary_matrix_is_pinned() {
        for width in [11usize, 65] {
            assert!(!id_ok(&id("trace", width), "trace"));
            assert!(!id_ok(&id("routine", width), "routine"));
        }
        for width in [12usize, 20, 21, 22, 41, 64] {
            assert!(id_ok(&id("trace", width), "trace"), "trace width {width}");
            assert!(id_ok(&id("routine", width), "routine"), "routine width {width}");
        }
        assert!(!id_ok("ores-trace-AAAAAAAAAAA!", "trace"));
        assert!(!id_ok(&id("routine", 21), "trace"));
    }

    #[test]
    fn canonical_generator_width_is_a_subset_of_compatibility() {
        let trace = id("trace", CANONICAL_GENERATOR_WIDTH);
        let routine = id("routine", CANONICAL_GENERATOR_WIDTH);
        assert!(canonical_generated_id(&trace, "trace") && id_ok(&trace, "trace"));
        assert!(canonical_generated_id(&routine, "routine") && id_ok(&routine, "routine"));
        assert!(!canonical_generated_id(&id("trace", 20), "trace"));
        assert!(!canonical_generated_id(&id("trace", 22), "trace"));
    }

    #[test]
    fn compatibility_widths_are_clean_at_call_sites() {
        for width in [12usize, 21, 41, 64] {
            assert!(msgs(&format!("l.info(\"x\").add_trace(\"{}\", false);", id("trace", width))).is_empty());
        }
        assert!(!msgs(&format!("l.info(\"x\").add_trace(\"{}\", false);", id("trace", 11))).is_empty());
        assert!(!msgs(&format!("l.info(\"x\").add_trace(\"{}\", false);", id("trace", 65))).is_empty());
    }

    #[test]
    fn matcher_needle_does_not_hide_real_call_site() {
        let line = format!(
            "if p.contains(\"/health\") {{ l.info(\"x\").add_trace(\"{}\", false); }}",
            id("trace", 11)
        );
        assert!(!msgs(&line).is_empty());
    }

    #[test]
    fn cfg_test_only_stops_at_test_module() {
        assert!(next_is_mod(&["#[cfg(test)]", "mod tests {"], 0));
        assert!(!next_is_mod(&["#[cfg(test)]", "use std::fmt;", "pub fn real() {}"], 0));
    }

    #[test]
    fn prefix_constants_headers_and_comments_are_not_violations() {
        assert!(msgs("const TracePrefix = \"ores-trace-\";").is_empty());
        assert!(msgs("const TRACE_HEADER: &str = \"x-ores-trace-id\";").is_empty());
        assert!(msgs("// historical ores-trace-abc").is_empty());
    }

    #[test]
    fn hoisted_trace_ids_are_rejected_but_routine_constants_are_allowed() {
        let good = id("trace", 21);
        assert!(hoisted_trace_decl(&format!("pub const SHARED_TRACE: &str = \"{good}\";")).is_some());
        assert!(msgs(&format!("const ROUTINE_ID: &str = \"{}\";", id("routine", 21))).is_empty());
    }

    #[test]
    fn retired_prefix_and_wrong_method_are_rejected() {
        let dd = format!("l.add_trace(\"{}\", false);", concat!("dd", "-trace-abcdefghijkl"));
        assert!(!msgs(&dd).is_empty());
        assert!(msgs("l.addRoutine(ROUTINE_ID);").iter().any(|m| m.contains("addRoutineId")));
    }

    #[test]
    fn longer_identifiers_are_not_call_sites() {
        assert!(scan_calls("x.addTraceIdFrom(ctx)", TRACE_METHODS).is_empty());
    }

    #[test]
    fn vacuous_scans_fail_closed() {
        assert!(vacuous_scan(Path::new("/nonexistent/ores-trace-contract-root"), 0).is_some());
        assert!(vacuous_scan(Path::new("."), 0).is_some());
        assert!(vacuous_scan(Path::new("."), 1).is_none());
    }
}
