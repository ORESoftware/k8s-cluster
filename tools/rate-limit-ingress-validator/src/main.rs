#![forbid(unsafe_code)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

const CATALOG_RELATIVE_PATH: &str = "remote/argocd/rate-limit-ingress";
const POLICY_ID: &str = "public-anonymous-ingress";
const PLACEHOLDER_TARGET: &str = "replace-me-before-enabling";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Controller {
    Nginx,
    EnvoyGateway,
    HaProxyIngress,
}

impl Controller {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Nginx => "ingress-nginx",
            Self::EnvoyGateway => "envoy-gateway",
            Self::HaProxyIngress => "haproxy-ingress",
        }
    }

    const fn signature(self) -> &'static str {
        match self {
            Self::Nginx => "nginx.ingress.kubernetes.io/limit-rps",
            Self::EnvoyGateway => "kind: backendtrafficpolicy",
            Self::HaProxyIngress => "haproxy-ingress.github.io/limit-rps",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    PerReplica,
    PeerSummed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Activation {
    DisabledTemplate,
    Enabled,
}

const CONTROLLERS: [Controller; 3] = [
    Controller::Nginx,
    Controller::EnvoyGateway,
    Controller::HaProxyIngress,
];
const SCOPES: [Scope; 2] = [Scope::PerReplica, Scope::PeerSummed];
const ACTIVATIONS: [Activation; 2] = [Activation::DisabledTemplate, Activation::Enabled];

struct TemplateSpec {
    controller: Controller,
    relative_path: &'static str,
}

const TEMPLATES: [TemplateSpec; 3] = [
    TemplateSpec {
        controller: Controller::Nginx,
        relative_path: "nginx/kustomization.yaml",
    },
    TemplateSpec {
        controller: Controller::EnvoyGateway,
        relative_path: "envoy-gateway/backend-traffic-policy.yaml",
    },
    TemplateSpec {
        controller: Controller::HaProxyIngress,
        relative_path: "haproxy-ingress/kustomization.yaml",
    },
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!(
                "rate-limit ingress catalog valid: 3 inert controllers, 12 exhaustive states, no raw identity selectors"
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("rate-limit ingress validation failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let repository_root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let catalog_root = repository_root.join(CATALOG_RELATIVE_PATH);

    validate_state_model()?;
    validate_catalog_is_not_activated(&repository_root, &catalog_root)?;
    for spec in &TEMPLATES {
        validate_template(&catalog_root, spec)?;
    }
    Ok(())
}

fn validate_state_model() -> Result<(), String> {
    let mut explored = 0_u8;
    let mut accepted = 0_u8;
    for controller in CONTROLLERS {
        for scope in SCOPES {
            for activation in ACTIVATIONS {
                explored = explored.saturating_add(1);
                if state_is_catalog_valid(controller, scope, activation) {
                    accepted = accepted.saturating_add(1);
                }
            }
        }
    }
    if explored != 12 || accepted != 3 {
        return Err(format!(
            "state model drifted: explored={explored}, accepted={accepted}"
        ));
    }
    Ok(())
}

const fn state_is_catalog_valid(
    controller: Controller,
    scope: Scope,
    activation: Activation,
) -> bool {
    match (controller, scope, activation) {
        (Controller::Nginx, Scope::PerReplica, Activation::DisabledTemplate) => true,
        (Controller::Nginx, Scope::PerReplica, Activation::Enabled) => false,
        (Controller::Nginx, Scope::PeerSummed, Activation::DisabledTemplate) => false,
        (Controller::Nginx, Scope::PeerSummed, Activation::Enabled) => false,
        (Controller::EnvoyGateway, Scope::PerReplica, Activation::DisabledTemplate) => true,
        (Controller::EnvoyGateway, Scope::PerReplica, Activation::Enabled) => false,
        (Controller::EnvoyGateway, Scope::PeerSummed, Activation::DisabledTemplate) => false,
        (Controller::EnvoyGateway, Scope::PeerSummed, Activation::Enabled) => false,
        (Controller::HaProxyIngress, Scope::PerReplica, Activation::DisabledTemplate) => false,
        (Controller::HaProxyIngress, Scope::PerReplica, Activation::Enabled) => false,
        (Controller::HaProxyIngress, Scope::PeerSummed, Activation::DisabledTemplate) => true,
        (Controller::HaProxyIngress, Scope::PeerSummed, Activation::Enabled) => false,
    }
}

fn validate_template(catalog_root: &Path, spec: &TemplateSpec) -> Result<(), String> {
    let path = catalog_root.join(spec.relative_path);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest = non_comment_body(&text);

    for required in [
        &format!("rate-limit.ores.io/policy-id: {POLICY_ID}"),
        "rate-limit.ores.io/activation: disabled-template",
        PLACEHOLDER_TARGET,
    ] {
        if !manifest.contains(required) {
            return Err(format!(
                "{} is missing required marker {required:?}",
                spec.relative_path
            ));
        }
    }

    let scope = parse_scope(&manifest, spec.relative_path)?;
    let activation = parse_activation(&manifest, spec.relative_path)?;
    if !state_is_catalog_valid(spec.controller, scope, activation) {
        return Err(format!(
            "{} has an invalid {:?}/{scope:?}/{activation:?} catalog state",
            spec.relative_path, spec.controller
        ));
    }

    let signatures = CONTROLLERS
        .into_iter()
        .filter(|controller| manifest.contains(controller.signature()))
        .collect::<Vec<_>>();
    if signatures != [spec.controller] {
        return Err(format!(
            "{} must contain exactly one controller signature; found {signatures:?}",
            spec.relative_path
        ));
    }

    validate_privacy(&manifest, spec.relative_path)?;
    validate_controller_primitives(&manifest, spec)?;
    Ok(())
}

fn parse_scope(manifest: &str, path: &str) -> Result<Scope, String> {
    let per_replica = manifest.contains("rate-limit.ores.io/scope: per-replica");
    let peer_summed = manifest.contains("rate-limit.ores.io/scope: peer-summed");
    match (per_replica, peer_summed) {
        (true, false) => Ok(Scope::PerReplica),
        (false, true) => Ok(Scope::PeerSummed),
        (false, false) => Err(format!("{path} has no explicit scope")),
        (true, true) => Err(format!("{path} declares two scopes")),
    }
}

fn parse_activation(manifest: &str, path: &str) -> Result<Activation, String> {
    let disabled = manifest.contains("rate-limit.ores.io/activation: disabled-template");
    let enabled = manifest.contains("rate-limit.ores.io/activation: enabled");
    match (disabled, enabled) {
        (true, false) => Ok(Activation::DisabledTemplate),
        (false, true) => Ok(Activation::Enabled),
        (false, false) => Err(format!("{path} has no activation marker")),
        (true, true) => Err(format!("{path} declares two activation states")),
    }
}

fn validate_privacy(manifest: &str, path: &str) -> Result<(), String> {
    let forbidden = [
        "authorization:",
        "cookie:",
        "clientselectors:",
        "x-user-id",
        "x-email",
        "x-subject",
        "x-session",
        "x-device",
        "x-api-key",
        "x-org-id",
        "redis://",
        "rediss://",
        "kind: secret",
        "stringdata:",
    ];
    for token in forbidden {
        if manifest.contains(token) {
            return Err(format!(
                "{path} contains forbidden identity/secret/backend token {token:?}"
            ));
        }
    }
    if manifest.contains('@') {
        return Err(format!("{path} contains a possible raw email address"));
    }
    Ok(())
}

fn validate_controller_primitives(manifest: &str, spec: &TemplateSpec) -> Result<(), String> {
    match spec.controller {
        Controller::Nginx => {
            require_all(
                manifest,
                spec.relative_path,
                &[
                    "nginx.ingress.kubernetes.io/limit-rps: \"20\"",
                    "nginx.ingress.kubernetes.io/limit-connections: \"10\"",
                    "nginx.ingress.kubernetes.io/limit-burst-multiplier: \"5\"",
                ],
            )?;
            forbid_all(
                manifest,
                spec.relative_path,
                &["limit-whitelist", "haproxy-ingress.github.io", "rateLimit:"],
            )?;
        }
        Controller::EnvoyGateway => {
            require_all(
                manifest,
                spec.relative_path,
                &[
                    "apiversion: gateway.envoyproxy.io/v1alpha1",
                    "kind: backendtrafficpolicy",
                    "kind: httproute",
                    "ratelimit:",
                    "local:",
                    "requests: 20",
                    "unit: second",
                ],
            )?;
            forbid_all(
                manifest,
                spec.relative_path,
                &["nginx.ingress.kubernetes.io", "haproxy-ingress.github.io"],
            )?;
        }
        Controller::HaProxyIngress => {
            require_all(
                manifest,
                spec.relative_path,
                &[
                    "haproxy-ingress.github.io/limit-rps: \"20\"",
                    "haproxy-ingress.github.io/limit-connections: \"10\"",
                    "rate-limit.ores.io/peers-required: \"true\"",
                ],
            )?;
            forbid_all(
                manifest,
                spec.relative_path,
                &["limit-whitelist", "nginx.ingress.kubernetes.io", "rateLimit:"],
            )?;
        }
    }
    Ok(())
}

fn require_all(manifest: &str, path: &str, tokens: &[&str]) -> Result<(), String> {
    for token in tokens {
        if !manifest.contains(token) {
            return Err(format!("{path} is missing required primitive {token:?}"));
        }
    }
    Ok(())
}

fn forbid_all(manifest: &str, path: &str, tokens: &[&str]) -> Result<(), String> {
    for token in tokens {
        if manifest.contains(token) {
            return Err(format!("{path} mixes forbidden primitive {token:?}"));
        }
    }
    Ok(())
}

fn non_comment_body(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_catalog_is_not_activated(
    repository_root: &Path,
    catalog_root: &Path,
) -> Result<(), String> {
    let argocd_root = repository_root.join("remote/argocd");
    for path in regular_files_below(&argocd_root)? {
        if path.starts_with(catalog_root) {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if text.contains("rate-limit-ingress") {
            return Err(format!(
                "catalog is activated or referenced outside itself by {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn regular_files_below(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let entries = fs::read_dir(&path)
            .map_err(|error| format!("cannot list {}: {error}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_space_is_exhaustive_and_only_inert_templates_are_valid() {
        let accepted = CONTROLLERS
            .into_iter()
            .flat_map(|controller| {
                SCOPES.into_iter().flat_map(move |scope| {
                    ACTIVATIONS
                        .into_iter()
                        .map(move |activation| (controller, scope, activation))
                })
            })
            .filter(|(controller, scope, activation)| {
                state_is_catalog_valid(*controller, *scope, *activation)
            })
            .collect::<Vec<_>>();

        assert_eq!(accepted.len(), 3);
        assert!(accepted.iter().all(|(_, _, activation)| {
            *activation == Activation::DisabledTemplate
        }));
    }

    #[test]
    fn controller_signatures_are_unique() {
        for left in CONTROLLERS {
            for right in CONTROLLERS {
                if left != right {
                    assert_ne!(left.signature(), right.signature());
                    assert_ne!(left.as_str(), right.as_str());
                }
            }
        }
    }

    #[test]
    fn privacy_guard_rejects_identity_and_secret_material() {
        for forbidden in [
            "Authorization: bearer secret",
            "Cookie: session=secret",
            "x-user-id: user-1",
            "x-email: person@example.com",
            "redis://cache.internal",
            "kind: Secret",
            "stringData: value",
        ] {
            assert!(validate_privacy(&forbidden.to_ascii_lowercase(), "test").is_err());
        }
    }
}
