// src/handlers/containers.rs

use crate::models::{
    V1Container, V1ContainerList, V1ContainerRequest, V1ResourceMeta, V1UserProfile,
};
use crate::resources::v1::containers::factory::platform_factory;

// Adjust the crate paths below to match your own project structure:
use crate::mutation::Mutation;
use crate::query::Query;
use crate::state::AppState;

use axum::{
    extract::Extension, extract::Json, extract::Path, extract::State, http::StatusCode,
    response::IntoResponse,
};
use sea_orm::*;
use serde_json::json;
use tracing::debug;

pub async fn get_container(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    debug!(
        "Getting container by namespace and name: {} {}",
        namespace, name
    );
    let container = match Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(container) => container,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    debug!("Container: {:?}", container.clone());

    debug!(
        "Getting container by id: {}",
        container.clone().id.to_string()
    );
    _get_container_by_id(db_pool, &container.clone().id.to_string(), &user_profile).await
}

pub async fn get_container_by_id(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(id): Path<String>,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    _get_container_by_id(db_pool, &id, &user_profile).await
}

pub async fn _get_container_by_id(
    db_pool: &DatabaseConnection,
    id: &str,
    user_profile: &V1UserProfile,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let container = Query::find_container_by_id_and_owners(db_pool, &id, &owner_id_refs)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            )
        })?;

    debug!("Found container by id and owners: {:?}", container);

    let out_container = V1Container {
        kind: "Container".to_string(),
        metadata: V1ResourceMeta {
            name: container.name.clone(),
            namespace: container.namespace.clone(),
            id: container.id.to_string(),
            owner: container.owner.clone(),
            created_at: container.created_at.timestamp(),
            updated_at: container.updated_at.timestamp(),
            created_by: container.created_by.unwrap_or_default(),
            owner_ref: container.owner_ref.clone(),
            labels: container
                .labels
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
        },
        image: container.image.clone(),
        platform: container.platform.unwrap_or_default(),
        env: container
            .env
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        command: container.command.clone(),
        args: container.args.clone(),
        volumes: container
            .volumes
            .and_then(|v| serde_json::from_value(v).ok()),
        accelerators: container.accelerators,
        meters: container
            .meters
            .and_then(|v| serde_json::from_value(v).ok()),
        status: container
            .status
            .and_then(|v| serde_json::from_value(v).ok()),
        restart: container.restart,
        queue: container.queue,
        timeout: container.timeout,
        resources: container
            .resources
            .and_then(|v| serde_json::from_value(v).ok()),
        ssh_keys: container
            .ssh_keys
            .and_then(|v| serde_json::from_value(v).ok()),
        ports: container.ports.and_then(|v| serde_json::from_value(v).ok()),
        proxy_port: container.proxy_port,
        authz: container.authz.and_then(|v| serde_json::from_value(v).ok()),
    };

    Ok(Json(out_container))
}

#[axum::debug_handler]
pub async fn list_containers(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
) -> Result<Json<V1ContainerList>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());

    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Query containers for all owner_ids
    let container_models = Query::find_containers_by_owners(db_pool, &owner_id_refs)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            )
        })?;

    // Convert database models to API response models
    let containers = container_models
        .into_iter()
        .map(|c| V1Container {
            kind: "Container".to_string(),
            metadata: V1ResourceMeta {
                name: c.name,
                namespace: c.namespace,
                id: c.id.to_string(),
                owner: c.owner,
                created_at: c.created_at.timestamp(),
                updated_at: c.updated_at.timestamp(),
                created_by: c.created_by.unwrap_or_default(),
                owner_ref: c.owner_ref.clone(),
                labels: c
                    .labels
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
            },
            image: c.image,
            env: c
                .env
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            command: c.command,
            args: c.args,
            platform: c.platform.unwrap_or_default(),
            volumes: c.volumes.and_then(|v| serde_json::from_value(v).ok()),
            accelerators: c.accelerators,
            meters: c.meters.and_then(|v| serde_json::from_value(v).ok()),
            status: c.status.and_then(|v| serde_json::from_value(v).ok()),
            restart: c.restart,
            queue: c.queue,
            timeout: c.timeout,
            resources: c.resources.and_then(|v| serde_json::from_value(v).ok()),
            ssh_keys: c.ssh_keys.and_then(|v| serde_json::from_value(v).ok()),
            ports: c.ports.and_then(|v| serde_json::from_value(v).ok()),
            proxy_port: c.proxy_port,
            authz: c.authz.and_then(|v| serde_json::from_value(v).ok()),
        })
        .collect();

    Ok(Json(V1ContainerList { containers }))
}

pub async fn create_container(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Json(container_request): Json<V1ContainerRequest>,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    match crate::validate::validate_name(
        &container_request
            .clone()
            .metadata
            .unwrap_or_default()
            .name
            .unwrap_or_default(),
    ) {
        Ok(_) => (),
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Invalid name: {}", e) })),
            ));
        }
    }

    match crate::validate::validate_namespace(
        &container_request
            .clone()
            .metadata
            .unwrap_or_default()
            .namespace
            .unwrap_or_default(),
    ) {
        Ok(_) => (),
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Invalid namespace: {}", e) })),
            ));
        }
    }

    let platform = platform_factory(
        container_request
            .clone()
            .platform
            .unwrap_or("runpod".to_string()),
    );
    let container = platform
        .declare(
            &container_request,
            db_pool,
            &user_profile,
            &user_profile.email,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    Ok(Json(container))
}

pub async fn delete_container(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let container = match Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(container) => container,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    _delete_container_by_id(db_pool, &container.clone().id.to_string(), &user_profile).await
}

pub async fn delete_container_by_id(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    _delete_container_by_id(db_pool, &id, &user_profile).await
}

pub async fn _delete_container_by_id(
    db_pool: &DatabaseConnection,
    id: &str,
    user_profile: &V1UserProfile,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let container = Query::find_container_by_id_and_owners(db_pool, &id, &owner_id_refs)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            )
        })?;

    // Check if user has permission to delete this container
    let _owner_id = container.owner.clone();

    let platform = platform_factory(container.platform.unwrap().clone());

    platform
        .delete(&container.id.to_string(), db_pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete container: {}", e)})),
            )
        })?;

    // Delete the container
    Mutation::delete_container(db_pool, id.to_string())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete container: {}", e)})),
            )
        })?;

    // Return just a 200 OK status code
    Ok(StatusCode::OK)
}

pub async fn fetch_container_logs_by_id(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(id): Path<String>,
) -> Result<Json<String>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    _fetch_container_logs_by_id(db_pool, &id, &user_profile).await
}

pub async fn fetch_container_logs(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<String>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let container = Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Database error: {}", e)})),
        )
    })?;

    _fetch_container_logs_by_id(db_pool, &container.clone().id.to_string(), &user_profile).await
}

pub async fn _fetch_container_logs_by_id(
    db_pool: &DatabaseConnection,
    id: &str,
    user_profile: &V1UserProfile,
) -> Result<Json<String>, (StatusCode, Json<serde_json::Value>)> {
    // Collect owner IDs from user_profile to use in your `Query` call
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();

    // Add user email if necessary for ownership checks
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the container in the DB, ensuring the user has permission
    let container = Query::find_container_by_id_and_owners(db_pool, &id, &owner_id_refs)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Database error: {}", err) })),
            )
        })?;

    let platform = platform_factory(container.platform.unwrap().clone());

    // Use the helper function to fetch logs
    let logs = platform
        .logs(&container.id.to_string(), db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to get logs: {}", err) })),
            )
        })?;

    Ok(Json(logs))
}
