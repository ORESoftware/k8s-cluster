//! Repository allowlist.
//!
//! The webhook secret is the primary defence against spoofed payloads, but
//! a leaked secret would otherwise let an attacker point the service at
//! arbitrary repos. We additionally pin the set of `owner/repo` slugs the
//! service is willing to analyze.
//!
//! Match rules:
//!
//! * Empty list (or a single `*` entry) means "allow every repo" — useful
//!   for local development and single-repo deployments.
//! * Entries of the form `owner/repo` must match exactly (case-insensitive).
//! * Entries of the form `owner/*` allow every repo under that owner.

#[derive(Debug, Clone)]
pub struct RepoAllowlist {
    entries: Vec<Entry>,
    allow_all: bool,
}

#[derive(Debug, Clone)]
enum Entry {
    Owner(String),
    Repo { owner: String, name: String },
}

impl RepoAllowlist {
    pub fn from_config(raw: &[String]) -> Self {
        if raw.is_empty() || raw.iter().any(|e| e.trim() == "*") {
            return Self {
                entries: Vec::new(),
                allow_all: true,
            };
        }
        let entries = raw
            .iter()
            .filter_map(|line| parse_entry(line.trim()))
            .collect();
        Self {
            entries,
            allow_all: false,
        }
    }

    pub fn allow_all(&self) -> bool {
        self.allow_all
    }

    /// `true` ⇒ this `owner/repo` slug is allowed.
    pub fn allows(&self, full_name: &str) -> bool {
        if self.allow_all {
            return true;
        }
        let Some((owner, repo)) = split_slug(full_name) else {
            return false;
        };
        self.entries.iter().any(|entry| match entry {
            Entry::Owner(o) => o.eq_ignore_ascii_case(owner),
            Entry::Repo { owner: o, name: n } => {
                o.eq_ignore_ascii_case(owner) && n.eq_ignore_ascii_case(repo)
            }
        })
    }
}

fn parse_entry(s: &str) -> Option<Entry> {
    if s.is_empty() {
        return None;
    }
    let (owner, repo) = split_slug(s)?;
    if repo == "*" {
        Some(Entry::Owner(owner.to_string()))
    } else {
        Some(Entry::Repo {
            owner: owner.to_string(),
            name: repo.to_string(),
        })
    }
}

fn split_slug(slug: &str) -> Option<(&str, &str)> {
    let (owner, repo) = slug.split_once('/')?;
    let owner = owner.trim();
    let repo = repo.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_allows_everything() {
        let a = RepoAllowlist::from_config(&[]);
        assert!(a.allow_all());
        assert!(a.allows("anyone/anything"));
    }

    #[test]
    fn star_entry_allows_everything() {
        let a = RepoAllowlist::from_config(&["*".to_string()]);
        assert!(a.allow_all());
        assert!(a.allows("anyone/anything"));
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        let a = RepoAllowlist::from_config(&["acme/widgets".to_string()]);
        assert!(a.allows("acme/widgets"));
        assert!(a.allows("Acme/Widgets"));
        assert!(!a.allows("attacker/widgets"));
        assert!(!a.allows("acme/other"));
    }

    #[test]
    fn owner_wildcard_matches_all_repos_under_owner() {
        let a = RepoAllowlist::from_config(&["acme/*".to_string()]);
        assert!(a.allows("acme/widgets"));
        assert!(a.allows("acme/sandbox"));
        assert!(!a.allows("attacker/widgets"));
    }

    #[test]
    fn rejects_malformed_slug() {
        let a = RepoAllowlist::from_config(&["just-owner".to_string()]);
        // No slash → no entries parsed → effectively empty deny-all.
        assert!(!a.allow_all());
        assert!(!a.allows("just-owner"));
    }

    #[test]
    fn rejects_slug_without_owner_or_repo() {
        let a = RepoAllowlist::from_config(&["/repo".to_string(), "owner/".to_string()]);
        assert!(!a.allow_all());
        assert!(!a.allows("/repo"));
        assert!(!a.allows("owner/"));
    }

    #[test]
    fn rejects_payload_with_missing_slash() {
        let a = RepoAllowlist::from_config(&["owner/repo".to_string()]);
        assert!(!a.allows("owner-repo"));
    }
}
