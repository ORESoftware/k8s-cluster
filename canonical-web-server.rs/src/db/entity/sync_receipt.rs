use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "sync_receipt")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub owner_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub client_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub mutation_id: Uuid,
    pub request_hash: String,
    pub result: Json,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
