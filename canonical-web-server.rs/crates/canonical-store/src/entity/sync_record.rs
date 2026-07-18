use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "sync_record")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub owner_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub collection: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub record_id: Uuid,
    pub version: i64,
    pub payload: Json,
    pub deleted_at: Option<DateTimeUtc>,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
