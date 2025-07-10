// src/handlers/containers.rs

use crate::config::SERVER_CONFIG;
use crate::entities::namespaces::{self, ActiveModel as NamespaceActiveModel};
use crate::handlers::v1::volumes::ensure_volume;
use crate::models::V1UserProfile;
use crate::resources::v1::namespaces::models::{V1Namespace, V1NamespaceRequest, V1Namespaces};
use crate::state::AppState;
use axum::{extract::Extension, extract::Json, extract::Path, extract::State, http::StatusCode};
use sea_orm::DbErr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::json;
use short_uuid;
use tracing::debug;

pub async fn get_namespace(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(name): Path<String>,
) -> Result<Json<V1Namespace>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let namespace_entity = namespaces::Entity::find()
        .filter(namespaces::Column::Name.eq(name.clone()))
        .filter(namespaces::Column::Owner.is_in(owner_id_refs))
        .one(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", err)})),
            )
        })?;

    let namespace_entity = namespace_entity.ok_or((
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": format!(
                "Namespace with name '{}' not found",
                name
            )
        })),
    ))?;

    Ok(Json(namespace_entity.to_v1()))
}

pub async fn create_namespace(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Json(namespace): Json<V1NamespaceRequest>,
) -> Result<Json<V1Namespace>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Get owner IDs from organizations and email
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let mut owner_id = user_profile.email.clone();
    if namespace.metadata.owner.is_some() {
        owner_id = namespace.metadata.owner.unwrap();
    }

    // check if owner_id is in owner_ids
    debug!("Owner ID: {}", owner_id);
    debug!("Owner IDs: {:?}", owner_ids);
    if !owner_ids.contains(&owner_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid owner ID"})),
        ));
    }

    // Check if a namespace with the same namespace and name already exists
    let existing_namespace = namespaces::Entity::find()
        .filter(namespaces::Column::Name.eq(&namespace.metadata.name))
        .filter(namespaces::Column::Owner.is_in(owner_id_refs))
        .one(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", err)})),
            )
        })?;

    if existing_namespace.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!(
                    "Namespace with name '{}' already exists",
                    namespace.metadata.name
                )
            })),
        ));
    }

    // Generate a unique ID for the namespace
    let id = short_uuid::ShortUuid::generate().to_string();

    // Create the namespace entity
    let namespace_entity = namespaces::Model::new(
        id,
        namespace.metadata.name.clone(),
        owner_id,
        user_profile.email.clone(), // Use the user's email as created_by
        namespace
            .metadata
            .labels
            .as_ref()
            .map(|labels| serde_json::to_value(labels).unwrap_or_default()),
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to create namespace: {}", err)})),
        )
    })?;

    // Insert the namespace into the database
    let namespace_entity = NamespaceActiveModel {
        id: Set(namespace_entity.id),
        name: Set(namespace_entity.name),
        owner: Set(namespace_entity.owner),
        owner_ref: Set(namespace_entity.owner_ref),
        labels: Set(namespace_entity.labels),
        created_by: Set(namespace_entity.created_by),
        updated_at: Set(namespace_entity.updated_at),
        created_at: Set(namespace_entity.created_at),
    };
    let namespace_entity = namespace_entity.insert(db_pool).await.map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Database error: {}", err)})),
        )
    })?;

    match ensure_volume(
        db_pool,
        &namespace_entity.name.clone(),
        &namespace_entity.owner.clone(),
        &namespace_entity.owner.clone(),
        &format!(
            "s3://{}/data/{}",
            &SERVER_CONFIG.bucket_name,
            &namespace_entity.name.clone()
        ),
        &namespace_entity.created_by.clone(),
        namespace_entity.labels.clone(),
    )
    .await
    {
        Ok(_) => (),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to create volume: {}", e)})),
            ))
        }
    }

    // Convert the entity back to V1Namespace and return it
    Ok(Json(namespace_entity.to_v1()))
}

pub async fn delete_namespace(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(name): Path<String>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Get owner IDs from organizations and email
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the namespace to delete
    let namespace_entity = namespaces::Entity::find()
        .filter(namespaces::Column::Name.eq(name.clone()))
        .filter(namespaces::Column::Owner.is_in(owner_id_refs))
        .one(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", err)})),
            )
        })?;

    let namespace_entity = namespace_entity.ok_or((
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": format!(
                "Namespace with name '{}' not found",
                name
            )
        })),
    ))?;

    // Delete the namespace
    namespaces::Entity::delete_by_id(namespace_entity.id)
        .exec(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete namespace: {}", err)})),
            )
        })?;

    Ok(())
}

/// Internal helper function to ensure a namespace exists with the given parameters.
/// Returns the namespace if it exists, or creates it if it doesn't.
pub async fn ensure_namespace(
    db_pool: &DatabaseConnection,
    name: &str,
    owner: &str,
    created_by: &str,
    labels: Option<serde_json::Value>,
) -> Result<(namespaces::Model, bool), DbErr> {
    debug!("Ensuring namespace: {:?}", name);
    // First, try to find the namespace by namespace and name
    let existing_namespace = namespaces::Entity::find()
        .filter(namespaces::Column::Name.eq(name))
        .one(db_pool)
        .await?;

    debug!("Existing namespace: {:?}", existing_namespace);
    // If the namespace exists, return it
    if let Some(namespace) = existing_namespace {
        debug!("Namespace already exists");
        return Ok((namespace, false));
    }
    debug!("Namespace does not exist");

    // If we get here, the namespace doesn't exist
    // Generate a unique ID for the new namespace
    let id = short_uuid::ShortUuid::generate().to_string();

    // Insert the namespace into the database
    let namespace_entity = NamespaceActiveModel {
        id: Set(id),
        name: Set(name.to_string()),
        owner: Set(owner.to_string()),
        owner_ref: Set(None),
        labels: Set(labels),
        created_by: Set(created_by.to_string()),
        updated_at: Set(chrono::Utc::now().into()),
        created_at: Set(chrono::Utc::now().into()),
    };

    let namespace_entity = namespace_entity.insert(db_pool).await?;
    debug!("Namespace inserted: {:?}", namespace_entity);

    Ok((namespace_entity, true))
}

/// Handler: List namespaces for the current user (and their organizations)
pub async fn list_namespaces(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
) -> Result<Json<V1Namespaces>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Gather all possible owner IDs from user + organizations
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Retrieve namespaces
    let namespaces_list = namespaces::Entity::find()
        .filter(namespaces::Column::Owner.is_in(owner_id_refs))
        .all(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Database error: {}", err) })),
            )
        })?;

    // Transform them into V1Namespace responses
    let namespaces = namespaces_list
        .into_iter()
        .map(|namespace| namespace.to_v1())
        .collect();

    Ok(Json(V1Namespaces { namespaces }))
}

pub async fn ensure_ns_and_resources(
    db_pool: &DatabaseConnection,
    name: &str,
    owner: &str,
    created_by: &str,
    labels: Option<serde_json::Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    match ensure_namespace(db_pool, name, owner, created_by, labels.clone()).await {
        Ok((_, created)) => {
            if created {
                ensure_volume(
                    db_pool,
                    name,
                    owner,
                    owner,
                    format!("s3://{}", &SERVER_CONFIG.bucket_name).as_str(),
                    created_by,
                    labels,
                )
                .await?;
            }
            Ok(())
        }
        Err(e) => return Err(Box::new(e)),
    }
}
