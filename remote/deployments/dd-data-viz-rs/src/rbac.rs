use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Role {
    Admin,
    Builder,
    Analyst,
    Viewer,
    Exporter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Permission {
    DatasetRead,
    DatasetWrite,
    QueryExecute,
    VisualizationSuggest,
    EvolutionRead,
    EvolutionRun,
    DashboardRead,
    DashboardWrite,
    AssociationRead,
    AlertRead,
    AlertWrite,
    AlertEvaluate,
    SemanticRead,
    SemanticWrite,
    SemanticCompile,
    InfraDiagramGenerate,
    PresentationExport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolePolicy {
    role: Role,
    description: &'static str,
    permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthContext {
    pub role: Role,
    pub permission: Permission,
    pub local_bypass: bool,
}

impl Role {
    pub(crate) fn from_header(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "admin" | "administrator" | "owner" => Some(Self::Admin),
            "builder" | "editor" | "author" => Some(Self::Builder),
            "analyst" | "sql" | "query" => Some(Self::Analyst),
            "viewer" | "reader" | "read-only" => Some(Self::Viewer),
            "exporter" | "presentation" | "deck" => Some(Self::Exporter),
            _ => None,
        }
    }

    pub(crate) fn allows(self, permission: Permission) -> bool {
        self.permissions().contains(&permission)
    }

    pub(crate) fn permissions(self) -> Vec<Permission> {
        match self {
            Self::Admin => all_permissions(),
            Self::Builder => vec![
                Permission::DatasetRead,
                Permission::DatasetWrite,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::EvolutionRead,
                Permission::EvolutionRun,
                Permission::DashboardRead,
                Permission::DashboardWrite,
                Permission::AssociationRead,
                Permission::AlertRead,
                Permission::AlertWrite,
                Permission::AlertEvaluate,
                Permission::SemanticRead,
                Permission::SemanticWrite,
                Permission::SemanticCompile,
                Permission::InfraDiagramGenerate,
                Permission::PresentationExport,
            ],
            Self::Analyst => vec![
                Permission::DatasetRead,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::EvolutionRead,
                Permission::DashboardRead,
                Permission::AssociationRead,
                Permission::AlertRead,
                Permission::AlertEvaluate,
                Permission::SemanticRead,
                Permission::SemanticCompile,
                Permission::InfraDiagramGenerate,
            ],
            Self::Viewer => vec![
                Permission::DatasetRead,
                Permission::VisualizationSuggest,
                Permission::DashboardRead,
                Permission::AssociationRead,
                Permission::AlertRead,
                Permission::SemanticRead,
            ],
            Self::Exporter => vec![
                Permission::DatasetRead,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::DashboardRead,
                Permission::AlertRead,
                Permission::SemanticRead,
                Permission::InfraDiagramGenerate,
                Permission::PresentationExport,
            ],
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Admin => {
                "Full operator access for all analytics, publishing, and hardening surfaces."
            }
            Self::Builder => {
                "Creates datasets, semantic models, dashboards, alert rules, visualizations, evolution runs, and exports."
            }
            Self::Analyst => {
                "Explores governed datasets and dashboards without mutating source data."
            }
            Self::Viewer => {
                "Reads datasets, dashboards, suggestions, and associative exploration results."
            }
            Self::Exporter => {
                "Builds presentation/export artifacts from existing analytical surfaces."
            }
        }
    }
}

impl Permission {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::DatasetRead => "Read dataset profiles and in-memory catalogs.",
            Self::DatasetWrite => "Ingest or replace in-memory datasets.",
            Self::QueryExecute => "Execute query dialects against datasets.",
            Self::VisualizationSuggest => "Generate visualization candidates.",
            Self::EvolutionRead => "Read evolution run summaries.",
            Self::EvolutionRun => "Run evolutionary visualization searches.",
            Self::DashboardRead => "Read saved dashboard definitions.",
            Self::DashboardWrite => "Create or replace saved dashboard definitions.",
            Self::AssociationRead => "Read Qlik-style associative graphs and selection state.",
            Self::AlertRead => "Read Grafana-style alert rules.",
            Self::AlertWrite => "Create or replace Grafana-style alert rules.",
            Self::AlertEvaluate => "Evaluate alert rules against analytical query results.",
            Self::SemanticRead => "Read governed semantic model definitions.",
            Self::SemanticWrite => "Create or replace governed semantic model definitions.",
            Self::SemanticCompile => {
                "Compile governed semantic model selections into query targets."
            }
            Self::InfraDiagramGenerate => {
                "Generate Terraform, AWS, and GCP infrastructure diagram render targets."
            }
            Self::PresentationExport => "Generate presentation/export layers.",
        }
    }
}

pub(crate) fn policy_catalog() -> Vec<RolePolicy> {
    [
        Role::Admin,
        Role::Builder,
        Role::Analyst,
        Role::Viewer,
        Role::Exporter,
    ]
    .into_iter()
    .map(|role| RolePolicy {
        role,
        description: role.description(),
        permissions: role.permissions(),
    })
    .collect()
}

pub(crate) fn policy_payload() -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.rbac.v1",
        "roleHeader": "X-Data-Viz-Role",
        "fallbackRole": "admin when operator auth succeeds and no role header is supplied",
        "roles": policy_catalog(),
        "permissions": all_permissions()
            .into_iter()
            .map(|permission| json!({
                "permission": permission,
                "description": permission.description()
            }))
            .collect::<Vec<_>>()
    })
}

fn all_permissions() -> Vec<Permission> {
    vec![
        Permission::DatasetRead,
        Permission::DatasetWrite,
        Permission::QueryExecute,
        Permission::VisualizationSuggest,
        Permission::EvolutionRead,
        Permission::EvolutionRun,
        Permission::DashboardRead,
        Permission::DashboardWrite,
        Permission::AssociationRead,
        Permission::AlertRead,
        Permission::AlertWrite,
        Permission::AlertEvaluate,
        Permission::SemanticRead,
        Permission::SemanticWrite,
        Permission::SemanticCompile,
        Permission::InfraDiagramGenerate,
        Permission::PresentationExport,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_cannot_write_datasets() {
        assert!(Role::Viewer.allows(Permission::DatasetRead));
        assert!(!Role::Viewer.allows(Permission::DatasetWrite));
        assert!(!Role::Viewer.allows(Permission::PresentationExport));
    }

    #[test]
    fn builder_can_publish_dashboards() {
        assert!(Role::Builder.allows(Permission::DashboardWrite));
        assert!(Role::Builder.allows(Permission::AlertWrite));
        assert!(Role::Builder.allows(Permission::SemanticWrite));
        assert!(Role::Builder.allows(Permission::EvolutionRun));
    }

    #[test]
    fn analyst_can_evaluate_but_not_write_alerts() {
        assert!(Role::Analyst.allows(Permission::AlertEvaluate));
        assert!(!Role::Analyst.allows(Permission::AlertWrite));
        assert!(!Role::Viewer.allows(Permission::AlertEvaluate));
    }

    #[test]
    fn analyst_can_compile_but_not_write_semantic_models() {
        assert!(Role::Analyst.allows(Permission::SemanticCompile));
        assert!(!Role::Analyst.allows(Permission::SemanticWrite));
        assert!(Role::Viewer.allows(Permission::SemanticRead));
        assert!(!Role::Viewer.allows(Permission::SemanticCompile));
    }

    #[test]
    fn infra_diagrams_require_non_viewer_role() {
        assert!(Role::Analyst.allows(Permission::InfraDiagramGenerate));
        assert!(Role::Exporter.allows(Permission::InfraDiagramGenerate));
        assert!(!Role::Viewer.allows(Permission::InfraDiagramGenerate));
    }

    #[test]
    fn role_header_aliases_are_supported() {
        assert_eq!(Role::from_header("reader"), Some(Role::Viewer));
        assert_eq!(Role::from_header("presentation"), Some(Role::Exporter));
        assert_eq!(Role::from_header("???"), None);
    }
}
