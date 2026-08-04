use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Deserialize;

use crate::profiles;

const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_EXACT_REPOSITORIES: usize = 256;
const MAX_PROFILES_PER_REPOSITORY: usize = 32;
const EXACT_RULE_PREFIX: &str = "exact:";
const EXACT_RULE_SEPARATOR: char = '#';
const PROFILE_SEPARATOR: char = '|';

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactRepositoryRule {
    repository: String,
    profiles: Vec<String>,
}

pub(crate) fn compile_rules(
    prefix_rules: Vec<String>,
    exact_rules_json: Option<&str>,
    globally_allowed_profiles: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let mut compiled = Vec::with_capacity(prefix_rules.len());
    for prefix in prefix_rules {
        validate_prefix_rule(&prefix)?;
        compiled.push(prefix);
    }

    let Some(raw) = exact_rules_json
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    else {
        return Ok(compiled);
    };
    if raw.len() > MAX_POLICY_BYTES {
        return Err(format!(
            "BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON exceeds {MAX_POLICY_BYTES} bytes"
        ));
    }

    let rules: Vec<ExactRepositoryRule> =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {error}"))?;
    if rules.len() > MAX_EXACT_REPOSITORIES {
        return Err(format!(
            "at most {MAX_EXACT_REPOSITORIES} exact profile repository rules are allowed"
        ));
    }

    let mut repositories = BTreeSet::new();
    for rule in rules {
        validate_exact_repository_url(&rule.repository)?;
        if !repositories.insert(rule.repository.clone()) {
            return Err(format!(
                "duplicate exact profile repository rule for {:?}",
                rule.repository
            ));
        }
        if rule.profiles.is_empty() {
            return Err(format!(
                "exact profile repository rule for {:?} must contain at least one profile",
                rule.repository
            ));
        }
        if rule.profiles.len() > MAX_PROFILES_PER_REPOSITORY {
            return Err(format!(
                "exact profile repository rule for {:?} exceeds {MAX_PROFILES_PER_REPOSITORY} profiles",
                rule.repository
            ));
        }

        let mut profile_names = BTreeSet::new();
        for profile in rule.profiles {
            validate_profile_name(&profile)?;
            if profiles::find(&profile).is_none() {
                return Err(format!(
                    "exact profile repository rule for {:?} references unknown profile {profile:?}",
                    rule.repository
                ));
            }
            if !globally_allowed_profiles.contains(&profile) {
                return Err(format!(
                    "exact profile repository rule for {:?} references profile {profile:?} that is disabled by BUILD_SERVER_ALLOWED_PROFILES",
                    rule.repository
                ));
            }
            if !profile_names.insert(profile.clone()) {
                return Err(format!(
                    "exact profile repository rule for {:?} repeats profile {profile:?}",
                    rule.repository
                ));
            }
        }

        compiled.push(format!(
            "{EXACT_RULE_PREFIX}{}{EXACT_RULE_SEPARATOR}{}",
            rule.repository,
            profile_names
                .into_iter()
                .collect::<Vec<_>>()
                .join(&PROFILE_SEPARATOR.to_string())
        ));
    }

    Ok(compiled)
}

pub(crate) fn ensure_repository_profile_allowed(
    repository: &str,
    profile: &str,
    compiled_rules: &[String],
) -> Result<(), String> {
    if compiled_rules.is_empty() {
        return Err(
            "profile repoUrl is rejected because the profile repository policy is empty"
                .to_string(),
        );
    }

    let mut exact_profiles = None::<BTreeSet<&str>>;
    for rule in compiled_rules {
        if let Some(encoded) = rule.strip_prefix(EXACT_RULE_PREFIX) {
            let (exact_repository, profiles) = encoded
                .split_once(EXACT_RULE_SEPARATOR)
                .ok_or_else(|| "compiled exact profile repository rule is malformed".to_string())?;
            if exact_repository == repository {
                if exact_profiles.is_some() {
                    return Err(format!(
                        "multiple compiled exact profile repository rules match {repository:?}"
                    ));
                }
                let decoded = profiles
                    .split(PROFILE_SEPARATOR)
                    .filter(|value| !value.is_empty())
                    .collect::<BTreeSet<_>>();
                if decoded.is_empty() {
                    return Err(format!(
                        "compiled exact profile repository rule for {repository:?} has no profiles"
                    ));
                }
                exact_profiles = Some(decoded);
            }
        }
    }

    if let Some(allowed_profiles) = exact_profiles {
        if allowed_profiles.contains(profile) {
            return Ok(());
        }
        return Err(format!(
            "profile {profile:?} is not allowed for exact repository {repository:?} by BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"
        ));
    }

    if compiled_rules
        .iter()
        .filter(|rule| !rule.starts_with(EXACT_RULE_PREFIX))
        .any(|prefix| repository.starts_with(prefix))
    {
        Ok(())
    } else {
        Err(
            "profile repoUrl is not allowed by BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES or BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"
                .to_string(),
        )
    }
}

fn validate_prefix_rule(prefix: &str) -> Result<(), String> {
    if prefix.is_empty()
        || prefix.chars().any(char::is_whitespace)
        || prefix.chars().any(char::is_control)
    {
        return Err(
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES contains an empty or non-printable rule"
                .to_string(),
        );
    }
    if prefix.starts_with(EXACT_RULE_PREFIX) {
        return Err(format!(
            "prefix rule {prefix:?} uses the reserved internal {EXACT_RULE_PREFIX:?} prefix"
        ));
    }
    if !(prefix.starts_with("https://github.com/") || prefix.starts_with("git@github.com:")) {
        return Err(format!(
            "profile repository prefix {prefix:?} must target github.com over HTTPS or SSH"
        ));
    }
    Ok(())
}

fn validate_exact_repository_url(repository: &str) -> Result<(), String> {
    if repository.chars().any(char::is_whitespace) || repository.chars().any(char::is_control) {
        return Err(format!(
            "exact profile repository URL {repository:?} contains whitespace or control characters"
        ));
    }
    let Some(path) = repository
        .strip_prefix("https://github.com/")
        .and_then(|value| value.strip_suffix(".git"))
    else {
        return Err(format!(
            "exact profile repository URL {repository:?} must use canonical https://github.com/OWNER/REPO.git form"
        ));
    };
    let mut components = path.split('/');
    let Some(owner) = components.next() else {
        return Err("exact profile repository URL is missing an owner".to_string());
    };
    let Some(repo) = components.next() else {
        return Err("exact profile repository URL is missing a repository".to_string());
    };
    if components.next().is_some()
        || !valid_github_component(owner)
        || !valid_github_component(repo)
    {
        return Err(format!(
            "exact profile repository URL {repository:?} must contain one GitHub-safe owner and repository"
        ));
    }
    Ok(())
}

fn validate_profile_name(profile: &str) -> Result<(), String> {
    if profile.is_empty()
        || profile.len() > 100
        || profile.chars().any(char::is_whitespace)
        || !profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("invalid fixed profile name {profile:?}"));
    }
    Ok(())
}

fn valid_github_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globally_allowed() -> HashSet<String> {
        HashSet::from([
            "rust-verify".to_string(),
            "node-verify".to_string(),
            "python-verify".to_string(),
        ])
    }

    #[test]
    fn exact_repository_rule_overrides_broad_prefix_fallback() {
        let rules = compile_rules(
            vec!["https://github.com/ORESoftware/".to_string()],
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy");

        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_ok());
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "node-verify",
            &rules,
        )
        .is_err());
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/another-repo.git",
            "node-verify",
            &rules,
        )
        .is_ok());
    }

    #[test]
    fn policy_rejects_duplicate_unknown_disabled_and_noncanonical_rules() {
        for raw in [
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]},{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["missing-profile"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["flutter-verify"]}]"#,
            r#"[{"repository":"git@github.com:ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster","profiles":["rust-verify"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":[]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify","rust-verify"]}]"#,
        ] {
            assert!(
                compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err(),
                "policy unexpectedly accepted {raw}"
            );
        }
    }

    #[test]
    fn policy_rejects_lookalike_repository_urls() {
        let rules = compile_rules(
            Vec::new(),
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy");

        for repository in [
            "https://github.com/ORESoftware/k8s-cluster.git-evil",
            "https://github.com/ORESoftware/k8s-cluster-extra.git",
            "git@github.com:ORESoftware/k8s-cluster.git",
        ] {
            assert!(ensure_repository_profile_allowed(repository, "rust-verify", &rules).is_err());
        }
    }

    #[test]
    fn empty_policy_fails_closed() {
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &[],
        )
        .is_err());
    }

    #[test]
    fn encoded_exact_rules_are_deterministic() {
        let rules = compile_rules(
            Vec::new(),
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["python-verify","rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy");
        assert_eq!(
            rules,
            vec!["exact:https://github.com/ORESoftware/k8s-cluster.git#python-verify|rust-verify"]
        );
    }

    #[test]
    fn exact_rule_parser_rejects_malformed_compiled_state() {
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &["exact:missing-separator".to_string()],
        )
        .is_err());
    }

    #[test]
    fn valid_exact_rule_count_is_bounded() {
        let mut rules = Vec::new();
        for index in 0..=MAX_EXACT_REPOSITORIES {
            rules.push(format!(
                r#"{{"repository":"https://github.com/ORESoftware/repo-{index}.git","profiles":["rust-verify"]}}"#
            ));
        }
        let raw = format!("[{}]", rules.join(","));
        assert!(compile_rules(Vec::new(), Some(&raw), &globally_allowed()).is_err());
    }

    #[test]
    fn prefix_rules_are_validated_before_use() {
        assert!(compile_rules(
            vec!["https://github.com/ORESoftware/".to_string()],
            None,
            &globally_allowed(),
        )
        .is_ok());
        for prefix in ["", "http://github.com/ORESoftware/", "exact:reserved"] {
            assert!(compile_rules(vec![prefix.to_string()], None, &globally_allowed(),).is_err());
        }
    }

    #[test]
    fn exact_rule_json_denies_unknown_fields() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"],"command":"cargo test"}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rules_can_be_omitted_without_changing_prefix_behavior() {
        let rules = compile_rules(
            vec!["https://github.com/sonus-auris/".to_string()],
            None,
            &globally_allowed(),
        )
        .expect("prefix-only policy");
        assert!(ensure_repository_profile_allowed(
            "https://github.com/sonus-auris/example.git",
            "node-verify",
            &rules,
        )
        .is_ok());
    }

    #[test]
    fn exact_rule_never_falls_back_after_repository_match() {
        let rules = vec![
            "https://github.com/ORESoftware/".to_string(),
            "exact:https://github.com/ORESoftware/k8s-cluster.git#rust-verify".to_string(),
        ];
        let error = ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "node-verify",
            &rules,
        )
        .expect_err("exact match must block prefix downgrade");
        assert!(error.contains("not allowed for exact repository"));
    }

    #[test]
    fn exact_rule_profiles_are_not_inferred_from_substrings() {
        let rules = vec![
            "exact:https://github.com/ORESoftware/k8s-cluster.git#rust-verify-extra".to_string(),
        ];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
    }

    #[test]
    fn exact_policy_uses_canonical_case_sensitive_url_identity() {
        let rules =
            vec!["exact:https://github.com/ORESoftware/k8s-cluster.git#rust-verify".to_string()];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/oresoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
    }

    #[test]
    fn exact_policy_can_list_multiple_reviewed_profiles() {
        let rules = compile_rules(
            Vec::new(),
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify","python-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy");
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "python-verify",
            &rules,
        )
        .is_ok());
    }

    #[test]
    fn profile_name_validation_rejects_separator_injection() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify|node-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_repository_validation_rejects_fragment_injection() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git#node-verify","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_repository_validation_rejects_query_injection() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git?x=1","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn profile_policy_compilation_does_not_log_or_retain_raw_json() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#;
        let rules = compile_rules(Vec::new(), Some(raw), &globally_allowed()).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(!rules[0].contains("repository\""));
        assert!(!rules[0].contains("profiles\""));
    }

    #[test]
    fn exact_rule_compile_rejects_disabled_but_known_profile() {
        let globally_allowed = HashSet::from(["rust-verify".to_string()]);
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["node-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed).is_err());
    }

    #[test]
    fn exact_rule_compile_accepts_existing_enabled_profile() {
        let globally_allowed = HashSet::from(["rust-verify".to_string()]);
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed).is_ok());
    }

    #[test]
    fn exact_rule_compile_rejects_non_array_top_level() {
        let raw = r#"{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_rejects_null_top_level() {
        assert!(compile_rules(Vec::new(), Some("null"), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_accepts_empty_array() {
        assert_eq!(
            compile_rules(Vec::new(), Some("[]"), &globally_allowed()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn profile_repository_policy_has_no_wildcard_syntax() {
        let raw =
            r#"[{"repository":"https://github.com/ORESoftware/*.git","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_profiles_are_sorted_for_stable_health_output() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify","node-verify"]}]"#;
        let rules = compile_rules(Vec::new(), Some(raw), &globally_allowed()).unwrap();
        assert_eq!(
            rules[0],
            "exact:https://github.com/ORESoftware/k8s-cluster.git#node-verify|rust-verify"
        );
    }

    #[test]
    fn exact_rule_compile_rejects_too_many_profiles() {
        let profiles = (0..=MAX_PROFILES_PER_REPOSITORY)
            .map(|index| format!("profile-{index}"))
            .collect::<Vec<_>>();
        let raw = serde_json::json!([{
            "repository": "https://github.com/ORESoftware/k8s-cluster.git",
            "profiles": profiles,
        }])
        .to_string();
        assert!(compile_rules(Vec::new(), Some(&raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_policy_requires_global_allowlist_even_if_compiled_rule_is_manually_injected() {
        // Startup compilation prevents this state. This test documents that the
        // request path receives a profile name separately and performs exact
        // equality only; the global profile allowlist remains the first gate in
        // validation.rs.
        let rules = vec![
            "exact:https://github.com/ORESoftware/k8s-cluster.git#unknown-profile".to_string(),
        ];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "unknown-profile",
            &rules,
        )
        .is_ok());
    }

    #[test]
    fn exact_rule_compile_rejects_control_characters() {
        let raw = "[{\"repository\":\"https://github.com/ORESoftware/k8s-cluster.git\\n\",\"profiles\":[\"rust-verify\"]}]";
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_policy_error_names_the_policy_source() {
        let rules =
            vec!["exact:https://github.com/ORESoftware/k8s-cluster.git#rust-verify".to_string()];
        let error = ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "node-verify",
            &rules,
        )
        .unwrap_err();
        assert!(error.contains("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"));
    }

    #[test]
    fn exact_policy_prefix_fallback_error_names_both_sources() {
        let error = ensure_repository_profile_allowed(
            "https://github.com/attacker/repo.git",
            "rust-verify",
            &["https://github.com/ORESoftware/".to_string()],
        )
        .unwrap_err();
        assert!(error.contains("BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES"));
        assert!(error.contains("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"));
    }

    #[test]
    fn compile_rules_preserves_prefix_order_and_appends_exact_rules() {
        let rules = compile_rules(
            vec![
                "https://github.com/ORESoftware/".to_string(),
                "git@github.com:sonus-auris/".to_string(),
            ],
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .unwrap();
        assert_eq!(rules[0], "https://github.com/ORESoftware/");
        assert_eq!(rules[1], "git@github.com:sonus-auris/");
        assert!(rules[2].starts_with(EXACT_RULE_PREFIX));
    }

    #[test]
    fn exact_rule_compile_rejects_trailing_slash_repository() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git/","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_rejects_nested_repository_path() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/team/k8s-cluster.git","profiles":["rust-verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_rejects_empty_owner_or_repo() {
        for repository in [
            "https://github.com//k8s-cluster.git",
            "https://github.com/ORESoftware/.git",
        ] {
            let raw = serde_json::json!([{
                "repository": repository,
                "profiles": ["rust-verify"],
            }])
            .to_string();
            assert!(compile_rules(Vec::new(), Some(&raw), &globally_allowed()).is_err());
        }
    }

    #[test]
    fn exact_rule_compile_rejects_whitespace_profile() {
        let raw = r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust verify"]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_rejects_empty_profile() {
        let raw =
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":[""]}]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_rule_compile_rejects_profile_over_length_limit() {
        let long_profile = "a".repeat(101);
        let raw = serde_json::json!([{
            "repository": "https://github.com/ORESoftware/k8s-cluster.git",
            "profiles": [long_profile],
        }])
        .to_string();
        assert!(compile_rules(Vec::new(), Some(&raw), &globally_allowed()).is_err());
    }

    #[test]
    fn exact_policy_rejects_second_matching_compiled_rule() {
        let rules = vec![
            "exact:https://github.com/ORESoftware/k8s-cluster.git#rust-verify".to_string(),
            "exact:https://github.com/ORESoftware/k8s-cluster.git#node-verify".to_string(),
        ];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
    }

    #[test]
    fn exact_policy_rejects_empty_encoded_profile_set() {
        let rules = vec!["exact:https://github.com/ORESoftware/k8s-cluster.git#".to_string()];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
    }

    #[test]
    fn exact_policy_rejects_missing_encoded_repository() {
        let rules = vec!["exact:#rust-verify".to_string()];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
    }
}
