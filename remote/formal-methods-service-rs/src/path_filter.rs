//! PR-changed-file path filter.
//!
//! The formal-methods pipeline is expensive (the slowest step is Kani,
//! which can take minutes). It also has nothing useful to say about PRs
//! that don't touch the contract math. This filter looks at the set of
//! files changed in a PR and decides whether the pipeline should run.
//!
//! Match rules:
//!
//! * Empty prefix list ⇒ run on every PR (the original behaviour).
//! * Any prefix entry that the changed-file path `starts_with` matches.
//!   Prefixes are matched as plain string `starts_with` so `packages/contract`
//!   matches both `packages/contract/Cargo.toml` and
//!   `packages/contract-deploy.sh`. To restrict to a directory exactly,
//!   include the trailing `/` (e.g. `packages/contract/`).

#[derive(Debug, Clone)]
pub struct PathFilter {
    prefixes: Vec<String>,
}

impl PathFilter {
    pub fn from_config(raw: &[String]) -> Self {
        let prefixes: Vec<String> = raw
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        Self { prefixes }
    }

    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    pub fn prefixes(&self) -> &[String] {
        &self.prefixes
    }

    /// `true` ⇒ at least one changed file is in scope of the pipeline, or
    /// the filter is empty (run-on-everything).
    pub fn matches_any(&self, changed_files: &[String]) -> bool {
        if self.prefixes.is_empty() {
            return true;
        }
        changed_files
            .iter()
            .any(|f| self.prefixes.iter().any(|p| f.starts_with(p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_matches_anything() {
        let f = PathFilter::from_config(&[]);
        assert!(f.is_empty());
        assert!(f.matches_any(&["README.md".into()]));
        assert!(f.matches_any(&[]));
    }

    #[test]
    fn directory_prefix_matches() {
        let f = PathFilter::from_config(&["packages/contract/".into()]);
        assert!(f.matches_any(&["README.md".into(), "packages/contract/Cargo.toml".into(),]));
    }

    #[test]
    fn matches_only_when_at_least_one_file_in_scope() {
        let f = PathFilter::from_config(&["packages/contract/".into()]);
        assert!(!f.matches_any(&["apps/frontend/foo.tsx".into(), "README.md".into()]));
    }

    #[test]
    fn multiple_prefixes_treated_as_union() {
        let f =
            PathFilter::from_config(&["packages/contract/".into(), "packages/brine-fp/".into()]);
        assert!(f.matches_any(&["packages/brine-fp/src/lib.rs".into()]));
        assert!(f.matches_any(&["packages/contract/Cargo.toml".into()]));
        assert!(!f.matches_any(&["apps/frontend/foo.tsx".into()]));
    }

    #[test]
    fn whitespace_and_empty_entries_are_trimmed() {
        let f = PathFilter::from_config(&["  packages/contract/  ".into(), "".into(), "  ".into()]);
        assert_eq!(f.prefixes(), &["packages/contract/".to_string()]);
        assert!(f.matches_any(&["packages/contract/Cargo.toml".into()]));
    }
}
