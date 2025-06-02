use sea_orm::DeriveEntityModel;
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;


#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "platforms")]
pub struct Model {
    #[sea_orm(primary_key, column_type = "Text", auto_increment = false)]
    pub id: String,
    pub version: i32,
    pub name: String
}

#[derive(Copy, Clone, Debug, EnumIter)]
pub enum Relation {}
