use sea_orm::entity::prelude::*;

/// Compliance frameworks an engagement can audit against. Mirrors the
/// canonical-interfaces `AuditEngagement.framework` enum and the migration's
/// check constraint; handlers validate against this list so SQLite (which
/// lacks the check on older versions) is equally protected.
pub const FRAMEWORKS: [&str; 6] = ["soc2", "fedramp", "hipaa", "iso_27001", "pci_dss", "gdpr"];

/// Engagement lifecycle stages. Mirrors `AuditEngagement.status`.
pub const STATUSES: [&str; 4] = ["scoping", "remediation", "in_audit", "complete"];

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_engagement")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub owner_id: Uuid,
    pub company: String,
    pub framework: String,
    pub status: String,
    pub opened_at: DateTimeUtc,
    pub target_report_date: Option<Date>,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::engagement_note::Entity")]
    EngagementNote,
}

impl Related<super::engagement_note::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EngagementNote.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
