use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "engagement_note")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub engagement_id: Uuid,
    pub owner_id: Uuid,
    pub body: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::audit_engagement::Entity",
        from = "Column::EngagementId",
        to = "super::audit_engagement::Column::Id"
    )]
    AuditEngagement,
}

impl Related<super::audit_engagement::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuditEngagement.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
