use std::collections::{BTreeSet, HashSet};

use serde::Deserialize;

use crate::profiles;

const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_EXACT_REPOSITORIES: usize = 256;
const MAX_PROFILES_PER_REPOSITORY: usize = 32;
const EXACT_RULE_PREFIX: &str = "exact-id:";
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

    let mut identities = BTreeSet::new();
    for rule in rules {
        let identity = exact_policy_identity(&rule.repository)?;
        if !identities.insert(identity.clone()) {
            return Err(format!(
                "duplicate exact profile repository identity {identity:?}"
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
            "{EXACT_RULE_PREFIX}{identity}{EXACT_RULE_SEPARATOR}{}",
            profile_names.into_iter().collect::<Vec<_>>().join("|")
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

    let identity = github_repository_identity(repository)?;
    let mut exact_profiles = None::<BTreeSet<&str>>;
    for rule in compiled_rules {
        let Some(encoded) = rule.strip_prefix(EXACT_RULE_PREFIX) else {
            continue;
        };
        let (exact_identity, profiles) = encoded
            .split_once(EXACT_RULE_SEPARATOR)
            .ok_or_else(|| "compiled exact profile repository rule is malformed".to_string())?;
        validate_compiled_identity(exact_identity)?;
        let decoded = profiles
            .split(PROFILE_SEPARATOR)
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>();
        if decoded.is_empty() {
            return Err(format!(
                "compiled exact profile repository rule for {exact_identity:?} has no profiles"
            ));
        }
        if exact_identity == identity {
            if exact_profiles.replace(decoded).is_some() {
                return Err(format!(
                    "multiple compiled exact profile repository rules match {identity:?}"
                ));
            }
        }
    }

    if let Some(allowed_profiles) = exact_profiles {
        if allowed_profiles.contains(profile) {
            return Ok(());
        }
        return Err(format!(
            "profile {profile:?} is not allowed for exact repository identity {identity:?} by BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"
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
        || prefix.contains(['?', '#'])
    {
        return Err(
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES contains an empty or unsafe rule"
                .to_string(),
        );
    }
    if prefix.starts_with(EXACT_RULE_PREFIX) {
        return Err(format!(
            "prefix rule {prefix:?} uses the reserved internal {EXACT_RULE_PREFIX:?} prefix"
        ));
    }
    if !(prefix.starts_with("https://github.com/")
        || prefix.starts_with("git@github.com:")
        || prefix.starts_with("ssh://git@github.com/"))
    {
        return Err(format!(
            "profile repository prefix {prefix:?} must target github.com over HTTPS or SSH"
        ));
    }
    Ok(())
}

fn exact_policy_identity(repository: &str) -> Result<String, String> {
    if !repository.starts_with("https://github.com/")
        || !repository.ends_with(".git")
        || repository.ends_with(".git/")
    {
        return Err(format!(
            "exact profile repository URL {repository:?} must use canonical https://github.com/OWNER/REPO.git form"
        ));
    }
    github_repository_identity(repository)
}

fn github_repository_identity(repository: &str) -> Result<String, String> {
    if repository.is_empty()
        || repository.chars().any(char::is_whitespace)
        || repository.chars().any(char::is_control)
        || repository.contains(['?', '#'])
    {
        return Err(format!(
            "profile repository URL {repository:?} contains unsupported characters"
        ));
    }

    let path = if let Some(path) = repository.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = repository.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = repository.strip_prefix("ssh://git@github.com/") {
        path
    } else {
        return Err(format!(
            "profile repository URL {repository:?} must use a supported github.com HTTPS or SSH form"
        ));
    };

    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut components = path.split('/');
    let Some(owner) = components.next() else {
        return Err("profile repository URL is missing an owner".to_string());
    };
    let Some(repo) = components.next() else {
        return Err("profile repository URL is missing a repository".to_string());
    };
    if components.next().is_some()
        || !valid_github_component(owner)
        || !valid_github_component(repo)
    {
        return Err(format!(
            "profile repository URL {repository:?} must contain one GitHub-safe owner and repository"
        ));
    }

    Ok(format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase()
    ))
}

fn validate_compiled_identity(identity: &str) -> Result<(), String> {
    let mut components = identity.split('/');
    let Some(owner) = components.next() else {
        return Err("compiled exact repository identity is missing an owner".to_string());
    };
    let Some(repo) = components.next() else {
        return Err("compiled exact repository identity is missing a repository".to_string());
    };
    if components.next().is_some()
        || !valid_github_component(owner)
        || !valid_github_component(repo)
        || owner.bytes().any(|byte| byte.is_ascii_uppercase())
        || repo.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(format!(
            "compiled exact repository identity {identity:?} is malformed"
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

    fn rules_with_exact_k8s_binding() -> Vec<String> {
        compile_rules(
            vec![
                "https://github.com/ORESoftware/".to_string(),
                "git@github.com:ORESoftware/".to_string(),
                "ssh://git@github.com/ORESoftware/".to_string(),
            ],
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy")
    }

    #[test]
    fn exact_repository_rule_overrides_every_supported_alias() {
        let rules = rules_with_exact_k8s_binding();
        for repository in [
            "https://github.com/ORESoftware/k8s-cluster.git",
            "https://github.com/ORESoftware/k8s-cluster",
            "https://github.com/oresoftware/K8S-CLUSTER.git/",
            "git@github.com:ORESoftware/k8s-cluster.git",
            "ssh://git@github.com/ORESoftware/k8s-cluster.git",
        ] {
            assert!(ensure_repository_profile_allowed(repository, "rust-verify", &rules).is_ok());
            assert!(ensure_repository_profile_allowed(repository, "node-verify", &rules).is_err());
        }
    }

    #[test]
    fn exact_match_never_falls_back_to_a_broad_prefix() {
        let error = ensure_repository_profile_allowed(
            "git@github.com:ORESoftware/k8s-cluster.git",
            "node-verify",
            &rules_with_exact_k8s_binding(),
        )
        .expect_err("SSH alias must not bypass the exact HTTPS binding");
        assert!(error.contains("not allowed for exact repository identity"));
    }

    #[test]
    fn unrelated_repositories_keep_reviewed_prefix_behavior() {
        let rules = rules_with_exact_k8s_binding();
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/another-repo.git",
            "node-verify",
            &rules,
        )
        .is_ok());
        assert!(ensure_repository_profile_allowed(
            "git@github.com:ORESoftware/another-repo.git",
            "python-verify",
            &rules,
        )
        .is_ok());
    }

    #[test]
    fn unsafe_query_fragment_and_nested_paths_fail_before_prefix_fallback() {
        let rules = rules_with_exact_k8s_binding();
        for repository in [
            "https://github.com/ORESoftware/k8s-cluster.git?profile=node-verify",
            "https://github.com/ORESoftware/k8s-cluster.git#node-verify",
            "https://github.com/ORESoftware/team/k8s-cluster.git",
        ] {
            assert!(ensure_repository_profile_allowed(repository, "rust-verify", &rules).is_err());
        }
    }

    #[test]
    fn exact_policy_keys_require_canonical_https_git_urls() {
        for repository in [
            "git@github.com:ORESoftware/k8s-cluster.git",
            "ssh://git@github.com/ORESoftware/k8s-cluster.git",
            "https://github.com/ORESoftware/k8s-cluster",
            "https://github.com/ORESoftware/k8s-cluster.git/",
            "https://github.com/ORESoftware/k8s-cluster.git?x=1",
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
    fn duplicate_repository_identities_are_case_insensitive() {
        let raw = r#"[
          {"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]},
          {"repository":"https://github.com/oresoftware/K8S-CLUSTER.git","profiles":["rust-verify"]}
        ]"#;
        assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
    }

    #[test]
    fn policy_rejects_unknown_disabled_duplicate_and_empty_profiles() {
        for raw in [
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["missing-profile"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["flutter-verify"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify","rust-verify"]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":[]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":[""]}]"#,
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify|node-verify"]}]"#,
        ] {
            assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
        }
    }

    #[test]
    fn policy_rejects_unknown_fields_and_non_array_top_levels() {
        for raw in [
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"],"command":"cargo test"}]"#,
            r#"{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}"#,
            "null",
        ] {
            assert!(compile_rules(Vec::new(), Some(raw), &globally_allowed()).is_err());
        }
    }

    #[test]
    fn policy_bounds_repository_count_profile_count_and_json_bytes() {
        let repositories = (0..=MAX_EXACT_REPOSITORIES)
            .map(|index| {
                serde_json::json!({
                    "repository": format!("https://github.com/ORESoftware/repo-{index}.git"),
                    "profiles": ["rust-verify"],
                })
            })
            .collect::<Vec<_>>();
        assert!(compile_rules(
            Vec::new(),
            Some(&serde_json::to_string(&repositories).unwrap()),
            &globally_allowed(),
        )
        .is_err());

        let profiles = (0..=MAX_PROFILES_PER_REPOSITORY)
            .map(|index| format!("profile-{index}"))
            .collect::<Vec<_>>();
        let raw = serde_json::json!([{
            "repository": "https://github.com/ORESoftware/k8s-cluster.git",
            "profiles": profiles,
        }])
        .to_string();
        assert!(compile_rules(Vec::new(), Some(&raw), &globally_allowed()).is_err());

        let oversized = " ".repeat(MAX_POLICY_BYTES + 1);
        assert!(compile_rules(Vec::new(), Some(&oversized), &globally_allowed()).is_err());
    }

    #[test]
    fn compiled_exact_rules_are_deterministic_and_lowercase_identity() {
        let rules = compile_rules(
            Vec::new(),
            Some(
                r#"[{"repository":"https://github.com/ORESoftware/K8S-Cluster.git","profiles":["python-verify","rust-verify"]}]"#,
            ),
            &globally_allowed(),
        )
        .expect("valid policy");
        assert_eq!(
            rules,
            vec!["exact-id:oresoftware/k8s-cluster#python-verify|rust-verify"]
        );
    }

    #[test]
    fn malformed_compiled_rules_fail_closed() {
        for rules in [
            vec!["exact-id:missing-separator".to_string()],
            vec!["exact-id:#rust-verify".to_string()],
            vec!["exact-id:ORESoftware/k8s-cluster#rust-verify".to_string()],
            vec!["exact-id:oresoftware/k8s-cluster#".to_string()],
            vec![
                "exact-id:oresoftware/k8s-cluster#rust-verify".to_string(),
                "exact-id:oresoftware/k8s-cluster#node-verify".to_string(),
            ],
        ] {
            assert!(ensure_repository_profile_allowed(
                "https://github.com/ORESoftware/k8s-cluster.git",
                "rust-verify",
                &rules,
            )
            .is_err());
        }
    }

    #[test]
    fn exact_profile_names_use_equality_not_substrings() {
        let rules = vec![
            "exact-id:oresoftware/k8s-cluster#rust-verify-extra".to_string(),
        ];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/k8s-cluster.git",
            "rust-verify",
            &rules,
        )
        .is_err());
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
    fn prefix_rules_are_validated_before_use() {
        for prefix in [
            "",
            "http://github.com/ORESoftware/",
            "https://github.com/ORESoftware/?x=1",
            "exact-id:reserved",
        ] {
            assert!(compile_rules(
                vec![prefix.to_string()],
                None,
                &globally_allowed(),
            )
            .is_err());
        }
        assert!(compile_rules(
            vec![
                "https://github.com/ORESoftware/".to_string(),
                "git@github.com:ORESoftware/".to_string(),
                "ssh://git@github.com/ORESoftware/".to_string(),
            ],
            None,
            &globally_allowed(),
        )
        .is_ok());
    }

    #[test]
    fn repositories_without_exact_rules_still_require_supported_github_urls() {
        let rules = vec!["https://github.com/ORESoftware/".to_string()];
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/example.git",
            "node-verify",
            &rules,
        )
        .is_ok());
        assert!(ensure_repository_profile_allowed(
            "https://github.com/ORESoftware/example.git?x=1",
            "node-verify",
            &rules,
        )
        .is_err());
        assert!(ensure_repository_profile_allowed(
            "file:///tmp/example.git",
            "node-verify",
            &rules,
        )
        .is_err());
    }
}
