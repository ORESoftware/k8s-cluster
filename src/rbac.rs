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
    ConnectionRead,
    ConnectionWrite,
    SqlLabRead,
    SqlLabWrite,
    QueryCacheRead,
    QueryExecute,
    VisualizationSuggest,
    EvolutionRead,
    EvolutionRun,
    DashboardRead,
    DashboardWrite,
    PublishingRead,
    PublishingRequest,
    PublishingReview,
    QuestionRead,
    QuestionSuggest,
    QuestionWrite,
    AssociationRead,
    AssociationWrite,
    AlertRead,
    AlertWrite,
    AlertEvaluate,
    SemanticRead,
    SemanticWrite,
    SemanticCompile,
    DaxCompile,
    EtlPlan,
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
                Permission::ConnectionRead,
                Permission::ConnectionWrite,
                Permission::SqlLabRead,
                Permission::SqlLabWrite,
                Permission::QueryCacheRead,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::EvolutionRead,
                Permission::EvolutionRun,
                Permission::DashboardRead,
                Permission::DashboardWrite,
                Permission::PublishingRead,
                Permission::PublishingRequest,
                Permission::PublishingReview,
                Permission::QuestionRead,
                Permission::QuestionSuggest,
                Permission::QuestionWrite,
                Permission::AssociationRead,
                Permission::AssociationWrite,
                Permission::AlertRead,
                Permission::AlertWrite,
                Permission::AlertEvaluate,
                Permission::SemanticRead,
                Permission::SemanticWrite,
                Permission::SemanticCompile,
                Permission::DaxCompile,
                Permission::EtlPlan,
                Permission::InfraDiagramGenerate,
                Permission::PresentationExport,
            ],
            Self::Analyst => vec![
                Permission::DatasetRead,
                Permission::ConnectionRead,
                Permission::SqlLabRead,
                Permission::SqlLabWrite,
                Permission::QueryCacheRead,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::EvolutionRead,
                Permission::DashboardRead,
                Permission::PublishingRead,
                Permission::PublishingRequest,
                Permission::QuestionRead,
                Permission::QuestionSuggest,
                Permission::QuestionWrite,
                Permission::AssociationRead,
                Permission::AssociationWrite,
                Permission::AlertRead,
                Permission::AlertEvaluate,
                Permission::SemanticRead,
                Permission::SemanticCompile,
                Permission::DaxCompile,
                Permission::InfraDiagramGenerate,
            ],
            Self::Viewer => vec![
                Permission::DatasetRead,
                Permission::VisualizationSuggest,
                Permission::DashboardRead,
                Permission::PublishingRead,
                Permission::QuestionRead,
                Permission::QuestionSuggest,
                Permission::AssociationRead,
                Permission::AlertRead,
                Permission::SemanticRead,
            ],
            Self::Exporter => vec![
                Permission::DatasetRead,
                Permission::ConnectionRead,
                Permission::SqlLabRead,
                Permission::QueryCacheRead,
                Permission::QueryExecute,
                Permission::VisualizationSuggest,
                Permission::DashboardRead,
                Permission::PublishingRead,
                Permission::PublishingRequest,
                Permission::QuestionRead,
                Permission::QuestionSuggest,
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
            Self::ConnectionRead => "Read secretRef-backed data connection metadata.",
            Self::ConnectionWrite => "Create or replace data connection metadata.",
            Self::SqlLabRead => "Read SQL Lab history and stored query metadata.",
            Self::SqlLabWrite => "Create SQL Lab history entries for bounded SELECT exploration.",
            Self::QueryCacheRead => "Read bounded in-memory query result cache entries.",
            Self::QueryExecute => "Execute query dialects against datasets.",
            Self::VisualizationSuggest => "Generate visualization candidates.",
            Self::EvolutionRead => "Read evolution run summaries.",
            Self::EvolutionRun => "Run evolutionary visualization searches.",
            Self::DashboardRead => "Read saved dashboard definitions.",
            Self::DashboardWrite => "Create or replace saved dashboard definitions.",
            Self::PublishingRead => "Read publishing approval requests.",
            Self::PublishingRequest => "Request publication of saved dashboards, questions, or charts.",
            Self::PublishingReview => "Approve or reject publishing requests.",
            Self::QuestionRead => "Read saved self-service questions and chart definitions.",
            Self::QuestionSuggest => {
                "Generate deterministic natural-language question proposals from dataset schemas."
            }
            Self::QuestionWrite => "Create or replace self-service questions and chart bindings.",
            Self::AssociationRead => "Read Qlik-style associative graphs and selection state.",
            Self::AssociationWrite => "Create or replace Qlik-style associative selection sessions.",
            Self::AlertRead => {
                "Read Grafana-style alert rules, notification routing metadata, and dispatch records."
            }
            Self::AlertWrite => {
                "Create or replace Grafana-style alert rules, contact points, and notification policies."
            }
            Self::AlertEvaluate => {
                "Evaluate alert rules and enqueue bounded notification dispatch attempts."
            }
            Self::SemanticRead => "Read governed semantic model definitions.",
            Self::SemanticWrite => "Create or replace governed semantic model definitions.",
            Self::SemanticCompile => {
                "Compile governed semantic model selections into query targets."
            }
            Self::DaxCompile => "Compile bounded Power BI DAX expressions against dataset fields.",
            Self::EtlPlan => "Compile Domo/Power Query-style ETL flow plans from dataset metadata.",
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
        Permission::ConnectionRead,
        Permission::ConnectionWrite,
        Permission::SqlLabRead,
        Permission::SqlLabWrite,
        Permission::QueryCacheRead,
        Permission::QueryExecute,
        Permission::VisualizationSuggest,
        Permission::EvolutionRead,
        Permission::EvolutionRun,
        Permission::DashboardRead,
        Permission::DashboardWrite,
        Permission::PublishingRead,
        Permission::PublishingRequest,
        Permission::PublishingReview,
        Permission::QuestionRead,
        Permission::QuestionSuggest,
        Permission::QuestionWrite,
        Permission::AssociationRead,
        Permission::AssociationWrite,
        Permission::AlertRead,
        Permission::AlertWrite,
        Permission::AlertEvaluate,
        Permission::SemanticRead,
        Permission::SemanticWrite,
        Permission::SemanticCompile,
        Permission::DaxCompile,
        Permission::EtlPlan,
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
        assert!(Role::Builder.allows(Permission::PublishingRequest));
        assert!(Role::Builder.allows(Permission::PublishingReview));
        assert!(Role::Builder.allows(Permission::ConnectionWrite));
        assert!(Role::Builder.allows(Permission::SqlLabWrite));
        assert!(Role::Builder.allows(Permission::QueryCacheRead));
        assert!(Role::Builder.allows(Permission::QuestionSuggest));
        assert!(Role::Builder.allows(Permission::QuestionWrite));
        assert!(Role::Builder.allows(Permission::AlertWrite));
        assert!(Role::Builder.allows(Permission::SemanticWrite));
        assert!(Role::Builder.allows(Permission::DaxCompile));
        assert!(Role::Builder.allows(Permission::EvolutionRun));
        assert!(Role::Builder.allows(Permission::EtlPlan));
        assert!(Role::Builder.allows(Permission::AssociationWrite));
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
        assert!(Role::Analyst.allows(Permission::DaxCompile));
        assert!(Role::Analyst.allows(Permission::ConnectionRead));
        assert!(!Role::Analyst.allows(Permission::ConnectionWrite));
        assert!(Role::Analyst.allows(Permission::SqlLabRead));
        assert!(Role::Analyst.allows(Permission::SqlLabWrite));
        assert!(Role::Analyst.allows(Permission::QueryCacheRead));
        assert!(Role::Analyst.allows(Permission::QuestionSuggest));
        assert!(Role::Analyst.allows(Permission::QuestionWrite));
        assert!(Role::Analyst.allows(Permission::PublishingRequest));
        assert!(!Role::Analyst.allows(Permission::PublishingReview));
        assert!(Role::Analyst.allows(Permission::AssociationWrite));
        assert!(!Role::Analyst.allows(Permission::SemanticWrite));
        assert!(!Role::Analyst.allows(Permission::EtlPlan));
        assert!(Role::Viewer.allows(Permission::SemanticRead));
        assert!(Role::Viewer.allows(Permission::PublishingRead));
        assert!(!Role::Viewer.allows(Permission::PublishingRequest));
        assert!(!Role::Viewer.allows(Permission::PublishingReview));
        assert!(Role::Viewer.allows(Permission::QuestionRead));
        assert!(Role::Viewer.allows(Permission::QuestionSuggest));
        assert!(!Role::Viewer.allows(Permission::SqlLabRead));
        assert!(!Role::Viewer.allows(Permission::SqlLabWrite));
        assert!(!Role::Viewer.allows(Permission::QueryCacheRead));
        assert!(!Role::Viewer.allows(Permission::QuestionWrite));
        assert!(!Role::Viewer.allows(Permission::SemanticCompile));
        assert!(!Role::Viewer.allows(Permission::DaxCompile));
        assert!(!Role::Viewer.allows(Permission::AssociationWrite));
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
