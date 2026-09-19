//! ORES static trace-marker contract checker.
//!
//! Fleet rule (DEN-666 / DEN-3432):
//!   * trace ids are INLINE string literals at the call site,
//!     `ores-trace-` + a 21-character nanoid;
//!   * routine ids are `ores-routine-` + a 21-character nanoid, declared once
//!     per function and passed to every addRoutineId call in it;
//!   * the legacy `dd-trace-` prefix is retired;
//!   * the method is `addRoutineId`, never `addRoutine`.
//!
//! Differences from the first-generation guard this replaces:
//!   1. the id length is pinned to exactly 21 (the nanoid width) instead of the
//!      loose {12,64}, which silently accepted truncated ids;
//!   2. Rust / Dart / Go / Java / Elixir call sites are checked, not only
//!      JS/TS -- the overwhelming majority of this fleet's markers are in Rust;
//!   3. it runs on pushes to the default branch, so the branch is actually
//!      validated and not only pull requests;
//!   4. test and fixture trees are excluded, because conformance suites
//!      deliberately exercise arbitrary trace ids.
//!
//! Single file, no external crates: build with `rustc -O`.
//!
//! This file implements the rule rather than using it, so it carries the
//! opt-out marker `ores-trace-contract:ignore-file` and is not scanned --
//! the same reason the upstream ESLint rule excludes `src/eslint-plugin.ts`.
//! Any other auditor, code generator or rule implementation may opt out the
//! same way by putting that marker anywhere in the file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Source extensions that can carry a next-loggers call site.
const EXTS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "dart", "go", "java", "ex", "exs",
];

/// Directories that never contain implementation call sites.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    "vendor",
    "generated",
    // Conformance/fixture trees: these deliberately pass arbitrary trace ids to
    // prove the logger accepts dynamic runtime context, so the static-marker
    // rule does not apply to them.
    "tests",
    "test",
    "testdata",
    "fixtures",
    "__tests__",
    "__mocks__",
    ".r2g",
];

/// Methods that take a trace id, across the SDK's language bindings.
const TRACE_METHODS: &[&str] = &[
    "addTraceId",
    "addTrace",
    "add_trace_id",
    "add_trace",
    "AddTraceID",
    "AddTrace",
];

/// Methods that take a routine id.
const ROUTINE_METHODS: &[&str] = &["addRoutineId", "add_routine_id", "AddRoutineID"];

struct Finding {
    file: String,
    line: usize,
    msg: String,
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `^ores-<kind>-[A-Za-z0-9_-]{21}$`
fn id_ok(id: &str, kind: &str) -> bool {
    let prefix = format!("ores-{kind}-");
    match id.strip_prefix(&prefix) {
        Some(rest) => rest.chars().count() == 21 && rest.chars().all(is_id_char),
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
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Returns the literal argument of `.<method>(` when the first argument is a
/// string literal, or `None` when it is an expression (runtime trace context,
/// which is allowed alongside a static marker).
fn literal_arg(line: &str, at: usize, method: &str) -> Option<Option<String>> {
    let after = line.get(at + 1 + method.len()..)?;
    // Must be a call, not a longer identifier such as `addTraceIdFrom`.
    let rest = after.trim_start();
    if !after.starts_with('(') && !rest.starts_with('(') {
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

/// Finds `.method(` occurrences, longest name first so `addTraceId` wins over
/// `addTrace`.
fn scan_calls(line: &str, methods: &[&str]) -> Vec<(String, Option<String>)> {
    let mut sorted: Vec<&str> = methods.to_vec();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.len()));
    let mut out = Vec::new();
    for (i, ch) in line.char_indices() {
        if ch != '.' {
            continue;
        }
        // A `.addTraceId(` that is itself inside a string literal is the fleet's
        // own auditors and ESLint rules naming the method, not a call site.
        if matches!(line[..i].chars().next_back(), Some('\'') | Some('"') | Some('`')) {
            continue;
        }
        for m in &sorted {
            let Some(seg) = line.get(i + 1..i + 1 + m.len()) else { continue };
            if seg != *m {
                continue;
            }
            // reject longer identifiers: next char must not continue the name
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

fn check_line(file: &str, no: usize, line: &str, f: &mut Vec<Finding>) {
    // Documentation prose never contains a call site at all.
    if is_comment(line) {
        return;
    }
    // Matcher/auditor code names these literals without emitting them, so the
    // prefix, hoisting and bare-shape rules are suppressed on such a line --
    // but the call-site rules (3 and 4) still run, because a real
    // `.add_trace("..")` does not stop being a call site just because the same
    // line also happens to contain `.contains(` or `assert!(`.
    let matcher = is_matcher(line);
    let push = |f: &mut Vec<Finding>, msg: String| {
        f.push(Finding { file: file.to_string(), line: no, msg })
    };

    // 1. retired prefix. Split so this checker's own source never trips it.
    if !matcher && line.contains(concat!("dd", "-trace-")) {
        push(f, "legacy dd-trace-* marker; use an inline ores-trace-* id".into());
    }

    // 2. wrong method name.
    if line.contains(".addRoutine(") || line.contains(".add_routine(") {
        push(f, "use addRoutineId()/add_routine_id(), not addRoutine()".into());
    }

    // 3. trace call sites.
    for (m, arg) in scan_calls(line, TRACE_METHODS) {
        if let Some(id) = arg {
            // A runtime-context id is only legal as a non-literal; a literal
            // must be a conforming static marker.
            if !id_ok(&id, "trace") {
                push(
                    f,
                    format!(
                        ".{m}(\"{id}\") is not a static marker; expected ^ores-trace-[A-Za-z0-9_-]{{21}}$"
                    ),
                );
            }
        }
    }

    // 4. routine call sites with a literal argument.
    for (m, arg) in scan_calls(line, ROUTINE_METHODS) {
        if let Some(id) = arg {
            if !id_ok(&id, "routine") {
                push(
                    f,
                    format!(
                        ".{m}(\"{id}\") is not a routine id; expected ^ores-routine-[A-Za-z0-9_-]{{21}}$"
                    ),
                );
            }
        }
    }

    // 5. hoisted trace ids: a static ores-trace-* belongs inline at the call
    //    site, never parked in a variable that several call sites share.
    //    (Routine ids are the opposite: they are declared once per function.)
    if let Some(rest) = hoisted_trace_decl(line).filter(|_| !matcher) {
        push(f, format!("static ores-trace-* id must stay inline at the call site, not in `{rest}`"));
    }

    // 6. Shape check on every marker literal in the file, whatever syntax
    //    reaches it. A call split across lines, a nested/chained call site, or
    //    an id parked in a constant all still land here, so a truncated id
    //    cannot hide from the call-site matcher above.
    if !matcher && !is_pattern_decl(line) {
        // Ids the call-site rules already reported on this line, so a bad
        // marker is named once rather than twice.
        let already: Vec<String> = f
            .iter()
            .filter(|x| x.line == no && x.file == file)
            .map(|x| x.msg.clone())
            .collect();
        for (kind, id) in marker_literals(line) {
            if already.iter().any(|m| m.contains(&format!("\"{id}\""))) {
                continue;
            }
            if !id_ok(&id, &kind) {
                push(
                    f,
                    format!(
                        "\"{id}\" does not match ^ores-{kind}-[A-Za-z0-9_-]{{21}}$ \
                         (ores-interfaces contracts/rpc-operation/v1 Ores{}Id)",
                        if kind == "trace" { "Trace" } else { "Routine" }
                    ),
                );
            }
        }
    }
}

/// True for lines that state the rule rather than use it (regex literals,
/// TypeSpec `@pattern`, JSON Schema `"pattern"`), so the contract's own text
/// and this checker's docs never self-trip.
fn is_pattern_decl(line: &str) -> bool {
    line.contains("[A-Za-z0-9_-]{21}")
        || line.contains("@pattern")
        || line.contains("\"pattern\"")
        || line.contains("^ores-")
}

/// Comment / doc-comment lines. Module and field docs across this fleet
/// describe the convention in prose ("attaches its own static `ores-trace-`
/// literal"), which is documentation, not a call site.
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with('*')
        || t.starts_with("<!--")
        || t.starts_with("--")
        // Elixir/Ruby/shell line comments. A Rust attribute is `#[` or `#!`,
        // never `# `, so this cannot swallow an attribute line.
        || t.starts_with("# ")
        || t == "#"
}

/// Lines that *detect* a marker rather than emit one: the fleet's own auditors,
/// ESLint rules and self-check tests match on these literals, and flagging them
/// would make every checker fail its own rule.
fn is_matcher(line: &str) -> bool {
    const NEEDLES: &[&str] = &[
        ".contains(",
        ".includes(",
        ".matches(",
        ".match(",
        ".starts_with(",
        ".startsWith(",
        ".strip_prefix(",
        ".test(",
        ".replace(",
        "assert!(",
        "assert_eq!(",
        "regex",
        "Regex",
        "RE =",
        "_RE",
    ];
    NEEDLES.iter().any(|n| line.contains(n))
}

/// Every `ores-trace-…` / `ores-routine-…` token on the line, as
/// (kind, full-id). Scans the whole line, so several markers on one chained
/// expression are all returned.
fn marker_literals(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (kind, tag) in [("trace", "ores-trace-"), ("routine", "ores-routine-")] {
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(tag) {
            let start = from + rel;
            // Not a marker if it is part of a longer word (e.g. `x-ores-trace-`).
            let prev = line[..start].chars().next_back();
            if matches!(prev, Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                from = start + tag.len();
                continue;
            }
            let id: String = line[start..].chars().take_while(|&c| is_id_char(c)).collect();
            // A bare prefix with no suffix is a prefix constant used to build ids.
            if id.len() > tag.len() {
                out.push((kind.to_string(), id.clone()));
            }
            from = start + id.len().max(tag.len());
        }
    }
    out
}

/// Matches `const|let|var|static` ... `= "ores-trace-..."` where the binding
/// name mentions trace.
fn hoisted_trace_decl(line: &str) -> Option<String> {
    let mut l = line.trim();
    // Strip visibility/modifier keywords so `pub const`, `export const`,
    // `pub(crate) static`, `public static final` are still seen as declarations.
    loop {
        let lower = l.to_lowercase();
        let stripped = ["pub(crate) ", "pub(super) ", "pub ", "export ", "public ", "private ", "protected ", "declare "]
            .iter()
            .find(|k| lower.starts_with(*k))
            .map(|k| l[k.len()..].trim_start());
        match stripped {
            Some(next) => l = next,
            None => break,
        }
    }
    let lower = l.to_lowercase();
    let is_decl = ["const ", "let ", "var ", "static ", "final ", "val "]
        .iter()
        .any(|k| lower.starts_with(k));
    if !is_decl {
        return None;
    }
    if !lower.contains("trace") {
        return None;
    }
    let eq = l.find('=')?;
    let (name, val) = l.split_at(eq);
    if !name.to_lowercase().contains("trace") {
        return None;
    }
    // A bare `"ores-trace-"` with no id after it is a PREFIX CONSTANT used to
    // build or match ids, not a hoisted marker, so it must not be flagged.
    let quoted = val.contains("\"ores-trace-") || val.contains("'ores-trace-") || val.contains("`ores-trace-");
    if quoted && marker_literals(val).iter().any(|(k, _)| k == "trace") {
        return Some(name.trim().to_string());
    }
    None
}

/// True when the first non-blank, non-attribute line after `idx` declares a
/// module, i.e. the `#[cfg(test)]` at `idx` guards a test MODULE rather than a
/// single item. Only that shape is treated as end-of-production-code.
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

fn main() -> ExitCode {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = PathBuf::from(root);

    let mut files = Vec::new();
    walk(&root, &mut files);

    let mut findings = Vec::new();
    let mut checked = 0usize;
    let mut markers = 0usize;

    for p in files {
        let Some(ext) = p.extension().map(|e| e.to_string_lossy().to_lowercase()) else { continue };
        if !EXTS.contains(&ext.as_str()) {
            continue;
        }
        if skip_path(&p) || is_test_file(&p) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let rel = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
        // Opt-out for a file that implements the convention rather than using
        // it (an auditor, an ESLint rule, a code generator).
        if text.contains("ores-trace-contract:ignore-file") {
            continue;
        }
        checked += 1;
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            // Rust keeps its unit tests inline behind `#[cfg(test)] mod tests`,
            // and those fixtures deliberately carry malformed ids. Stop only at
            // that trailing TEST MODULE -- a `#[cfg(test)]` on any other item
            // (a `use`, a helper fn, a const) must NOT blind the checker to the
            // rest of the file.
            if ext == "rs" && line.trim_start().starts_with("#[cfg(test)]") && next_is_mod(&lines, i) {
                break;
            }
            let line = *line;
            if line.contains("ores-trace-") || line.contains("ores-routine-") {
                markers += 1;
            }
            check_line(&rel, i + 1, line, &mut findings);
        }
    }

    if findings.is_empty() {
        println!(
            "ores-trace-contract: OK -- {checked} source files checked, {markers} marker lines, 0 violations"
        );
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

    /// Messages the line-level rules produce for one source line.
    fn msgs(line: &str) -> Vec<String> {
        let mut f = Vec::new();
        check_line("x.rs", 1, line, &mut f);
        f.into_iter().map(|x| x.msg).collect()
    }

    const GOOD: &str = "ores-trace-V1sTq7bK2mNp4Rd8Xe0Lz"; // 21-char nanoid

    #[test]
    fn id_length_is_pinned_to_exactly_21() {
        assert!(id_ok(GOOD, "trace"));
        assert_eq!(GOOD.len(), "ores-trace-".len() + 21);
        // 20 and 22 are the off-by-one cases the loose {12,64} guard accepted.
        assert!(!id_ok("ores-trace-AAAAAAAAAAAAAAAAAAAA", "trace"));
        assert!(!id_ok("ores-trace-BBBBBBBBBBBBBBBBBBBBBB", "trace"));
        assert!(!id_ok("ores-trace-", "trace"));
        assert!(!id_ok("ores-routine-V1sTq7bK2mNp4Rd8Xe0Lz", "trace"));
        assert!(id_ok("ores-routine-V1sTq7bK2mNp4Rd8Xe0Lz", "routine"));
    }

    #[test]
    fn conforming_call_site_is_clean() {
        assert!(msgs(&format!(
            "    log.info(\"x\").add_trace(\"{GOOD}\", false).send();"
        ))
        .is_empty());
    }

    #[test]
    fn bad_call_site_is_flagged_in_every_surface_syntax() {
        // direct
        assert!(!msgs("log.info(\"x\").add_trace(\"ores-trace-SHORT\", false);").is_empty());
        // nested / chained
        assert!(!msgs("outer(inner(l.info(\"x\").add_trace(\"ores-trace-SHORT\", false)));").is_empty());
        // raw string (no leading quote, so only the shape rule can see it)
        assert!(!msgs("l.add_trace(r\"ores-trace-SHORT\", false);").is_empty());
        // macro argument
        assert!(!msgs("emit!(\"ores-trace-SHORT\");").is_empty());
        // the continuation line of a call split across lines
        assert!(!msgs("            \"ores-trace-SHORT\",").is_empty());
    }

    /// Regression: a matcher needle anywhere on the line used to suppress the
    /// WHOLE line, so a real call site could hide behind `.contains(`.
    #[test]
    fn matcher_needle_does_not_hide_a_real_call_site() {
        let line =
            "if p.contains(\"/health\") { l.info(\"x\").add_trace(\"ores-trace-SHORT\", false); }";
        assert!(
            msgs(line).iter().any(|m| m.contains("is not a static marker")),
            "matcher needle suppressed a call site: {:?}",
            msgs(line)
        );
    }

    /// Regression: `#[cfg(test)]` on a non-module item must not blind the
    /// checker to the rest of the file; only a trailing test MODULE does.
    #[test]
    fn cfg_test_stops_scanning_only_at_a_test_module() {
        let m = ["#[cfg(test)]", "mod tests {"];
        assert!(next_is_mod(&m, 0));
        let u = ["#[cfg(test)]", "use std::fmt;", "pub fn real() {}"];
        assert!(!next_is_mod(&u, 0));
        let f = ["#[cfg(test)]", "fn helper() {}"];
        assert!(!next_is_mod(&f, 0));
        let blank = ["#[cfg(test)]", "", "pub mod tests {"];
        assert!(next_is_mod(&blank, 0));
    }

    /// Regression: a bare `"ores-trace-"` is a PREFIX CONSTANT, not a hoisted
    /// marker, whatever the binding is called.
    #[test]
    fn prefix_constants_and_header_names_are_not_violations() {
        assert!(msgs("const TracePrefix = \"ores-trace-\"").is_empty());
        assert!(msgs("pub const TRACE_ID_PREFIX: &str = \"ores-trace-\";").is_empty());
        assert!(msgs("const TRACE_HEADER: &str = \"x-ores-trace-id\";").is_empty());
        assert!(msgs("    headers.insert(\"x-ores-trace-id\", v);").is_empty());
        assert!(marker_literals("\"x-ores-trace-id\"").is_empty());
    }

    /// Regression: visibility keywords used to let a hoisted id through.
    #[test]
    fn hoisted_trace_ids_are_flagged_through_visibility_keywords() {
        for decl in [
            format!("const SHARED_TRACE: &str = \"{GOOD}\";"),
            format!("pub const SHARED_TRACE: &str = \"{GOOD}\";"),
            format!("pub(crate) static SHARED_TRACE: &str = \"{GOOD}\";"),
            format!("export const traceId = \"{GOOD}\";"),
        ] {
            assert!(
                hoisted_trace_decl(&decl).is_some(),
                "hoisted id not detected: {}",
                decl
            );
        }
        // A routine id declared once per function is the required shape, not a
        // violation.
        assert!(msgs(&format!(
            "    const ROUTINE_ID: &str = \"ores-routine-V1sTq7bK2mNp4Rd8Xe0Lz\";"
        ))
        .is_empty());
    }

    /// Regression: `#` line comments (Elixir/Ruby) were read as code.
    #[test]
    fn hash_comments_are_prose_but_attributes_are_not() {
        assert!(is_comment("# the old marker was ores-trace-abc123"));
        assert!(msgs("  # historical: ores-trace-abc123 was retired").is_empty());
        assert!(!is_comment("#[derive(Debug)]"));
        assert!(!is_comment("#![allow(dead_code)]"));
    }

    #[test]
    fn auditor_and_doc_lines_do_not_self_trip() {
        assert!(msgs("    if line.contains(\"ores-trace-\") { }").is_empty());
        assert!(msgs("/// each call site carries its own ores-trace-abc marker").is_empty());
        assert!(msgs("    \"pattern\": \"^ores-trace-[A-Za-z0-9_-]{21}$\"").is_empty());
    }

    #[test]
    fn retired_prefix_and_wrong_method_are_flagged() {
        let dd = format!("l.add_trace(\"{}\", false);", concat!("dd", "-trace-abc"));
        assert!(!msgs(&dd).is_empty());
        assert!(msgs("l.addRoutine(ROUTINE_ID);")
            .iter()
            .any(|m| m.contains("addRoutineId")));
    }

    #[test]
    fn longer_identifiers_are_not_call_sites() {
        assert!(scan_calls("x.addTraceIdFrom(ctx)", TRACE_METHODS).is_empty());
        assert!(msgs("x.addTraceIdFrom(ctx)").is_empty());
    }
}
