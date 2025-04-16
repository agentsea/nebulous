use crate::config::SERVER_CONFIG;
use crate::entities::namespaces;
use anyhow::Result;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::{debug, error};

pub async fn auth_ns(
    db_pool: &DatabaseConnection,
    owner_ids: &Vec<String>,
    namespace: &str,
) -> Result<String> {
    debug!("Authorizing namespace: {:?}", namespace);
    let namespace_entity = match namespaces::Entity::find()
        .filter(namespaces::Column::Name.eq(namespace))
        .one(db_pool)
        .await?
    {
        Some(namespace) => namespace,
        None => {
            error!("Namespace {} not found", namespace);
            return Err(anyhow::anyhow!("Namespace not found"));
        }
    };
    debug!("Namespace found: {:?}", namespace_entity);

    if namespace == "root" {
        debug!("Namespace is root");
        let root_owner = SERVER_CONFIG.root_owner.clone();
        if !owner_ids.contains(&root_owner) {
            error!("User not authorized to access root namespace");
            return Err(anyhow::anyhow!("User not authorized to access namespace"));
        }
        debug!("User is authorized to access root namespace");
    }

    if !owner_ids.contains(&namespace_entity.owner) {
        error!("User not authorized to access namespace");
        return Err(anyhow::anyhow!("User not authorized to access namespace"));
    }
    debug!("User is authorized to access namespace");
    Ok(namespace_entity.owner)
}
