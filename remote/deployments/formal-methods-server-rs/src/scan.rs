use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicU64,
};

use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use walkdir::WalkDir;

use crate::{
    annotations::parse_annotations,
    expr::SortHint,
    state::{AppState, Config},
    types::Finding,
    verify::{heuristic_checks, verify_block, VerifyContext},
};

// ---------------------------------------------------------------------------
// scanning the working tree
// ---------------------------------------------------------------------------

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn is_source_path(
    config: &Config,
    rel_path: &Path,
    languages_filter: &Option<HashSet<String>>,
) -> bool {
    let Some(ext) = extension_of(rel_path) else {
        return false;
    };
    if !config.allowed_extensions.contains(&ext) {
        return false;
    }
    if let Some(filter) = languages_filter {
        if !filter.contains(&ext) {
            return false;
        }
    }
    true
}

fn matches_paths(rel_path: &Path, filters: &Option<Vec<PathBuf>>) -> bool {
    let Some(filters) = filters else {
        return true;
    };
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|filter| {
        if filter.as_os_str().is_empty() || filter.as_os_str() == "." {
            return true;
        }
        rel_path.starts_with(filter)
    })
}

pub(crate) async fn analyze_tree(
    state: &AppState,
    root: &Path,
    languages_filter: &Option<HashSet<String>>,
    path_filter: &Option<Vec<PathBuf>>,
    heuristics_enabled: bool,
    log_path: &Path,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> (Vec<Finding>, usize) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut files_scanned = 0usize;
    let ctx = VerifyContext {
        config: &state.config,
    };

    let walker = WalkDir::new(root)
        .max_depth(20)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok);

    for entry in walker {
        if findings.len() >= state.config.max_findings_per_job {
            append_log(
                log_path,
                "max findings reached, stopping early\n",
                state.config.max_log_bytes,
            )
            .await;
            break;
        }
        if files_scanned >= state.config.max_files {
            append_log(
                log_path,
                "max file count reached, stopping early\n",
                state.config.max_log_bytes,
            )
            .await;
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if rel
            .components()
            .any(|c| matches!(c, Component::Normal(name) if name == ".git" || name == "node_modules" || name == "target" || name == "build" || name == "dist" || name == ".venv" || name == "venv"))
        {
            continue;
        }
        if !is_source_path(&state.config, rel, languages_filter) {
            continue;
        }
        if !matches_paths(rel, path_filter) {
            continue;
        }
        let meta = match fs::metadata(path).await {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.len() > state.config.max_file_bytes {
            continue;
        }
        let content = match fs::read_to_string(path).await {
            Ok(content) => content,
            Err(_) => continue,
        };
        files_scanned += 1;

        let file_label = rel.to_string_lossy().to_string();
        let parsed = parse_annotations(&file_label, &content);

        let mut decls_lookup: HashMap<String, SortHint> = HashMap::new();
        for block in &parsed.blocks {
            for decl in &block.decls {
                decls_lookup.insert(decl.name.clone(), decl.sort.clone());
            }
        }

        for block in &parsed.blocks {
            let mut block_findings = verify_block(&ctx, block, z3_calls, z3_failures).await;
            findings.append(&mut block_findings);
            if findings.len() >= state.config.max_findings_per_job {
                break;
            }
        }
        if findings.len() >= state.config.max_findings_per_job {
            break;
        }
        if heuristics_enabled && !decls_lookup.is_empty() {
            let mut h = heuristic_checks(&ctx, &parsed, &decls_lookup, z3_calls, z3_failures).await;
            findings.append(&mut h);
        }

        append_log(
            log_path,
            &format!(
                "scanned {} ({} blocks, {} decls)\n",
                file_label,
                parsed.blocks.len(),
                decls_lookup.len()
            ),
            state.config.max_log_bytes,
        )
        .await;
    }

    (findings, files_scanned)
}

// ---------------------------------------------------------------------------
// log writing
// ---------------------------------------------------------------------------

pub(crate) async fn append_log(path: &Path, message: &str, max_bytes: u64) {
    let current_len = fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0);
    if current_len >= max_bytes {
        return;
    }
    let remaining = (max_bytes - current_len) as usize;
    let bytes = message.as_bytes();
    let limit = remaining.min(bytes.len());
    if limit == 0 {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(&bytes[..limit]).await;
    }
}
