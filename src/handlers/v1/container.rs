// src/handlers/containers.rs

use crate::models::{V1AuthzConfig, V1Meter, V1ResourceMeta, V1ResourceMetaRequest, V1UserProfile};
use crate::resources::v1::containers::factory::platform_factory;
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerHealthCheck, V1ContainerRequest, V1ContainerResources,
    V1ContainerSearch, V1Containers, V1EnvVar, V1UpdateContainer,
};
use crate::resources::v1::volumes::models::V1VolumePath;
// Adjust the crate paths below to match your own project structure:
use crate::agent::ns::auth_ns;
use crate::entities::containers;
use crate::mutation::Mutation;
use crate::query::Query;
use crate::state::AppState;
use crate::utils::namespace::resolve_namespace;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{
    extract::Extension, extract::Json, extract::Path, extract::State, http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use sea_orm::sea_query::extension::postgres::PgExpr;
use sea_orm::sea_query::{Alias, Expr};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, error};

pub async fn get_container(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());

    let owner = auth_ns(db_pool, &owner_ids, &resolved_namespace)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Authorization error: {}", e)})),
            )
        })?;

    debug!(
        "Getting container by namespace and name: {} {}",
        resolved_namespace, name
    );
    let container = match Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &vec![owner.as_str()],
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

    let owner = auth_ns(db_pool, &owner_ids, &container.namespace)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Authorization error: {}", e)})),
            )
        })?;

    debug!("Found container by id and owners: {:?}", container);

    let out_container = V1Container {
        kind: "Container".to_string(),
        metadata: V1ResourceMeta {
            name: container.name.clone(),
            namespace: container.namespace.clone(),
            id: container.id.to_string(),
            owner: owner,
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
        health_check: container
            .health_check
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
) -> Result<Json<V1Containers>, (StatusCode, Json<serde_json::Value>)> {
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
            health_check: c.health_check.and_then(|v| serde_json::from_value(v).ok()),
            ssh_keys: c.ssh_keys.and_then(|v| serde_json::from_value(v).ok()),
            ports: c.ports.and_then(|v| serde_json::from_value(v).ok()),
            proxy_port: c.proxy_port,
            authz: c.authz.and_then(|v| serde_json::from_value(v).ok()),
        })
        .collect();

    Ok(Json(V1Containers { containers }))
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
    debug!("Container request: {:?}", container_request);

    let namespace_opt = container_request
        .clone()
        .metadata
        .unwrap_or_default()
        .namespace;

    let handle = match user_profile.handle.clone() {
        Some(handle) => handle,
        None => user_profile
            .email
            .clone()
            .replace("@", "-")
            .replace(".", "-"),
    };
    debug!("Handle: {:?}", handle);

    let namespace = match namespace_opt {
        Some(namespace) => resolve_namespace(&namespace, &user_profile),
        None => match crate::handlers::v1::namespaces::ensure_namespace(
            db_pool,
            &handle,
            &user_profile.email,
            &user_profile.email,
            None,
        )
        .await
        {
            Ok(_) => handle,
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("Invalid namespace: {}", e) })),
                ));
            }
        },
    };
    debug!(">> Using namespace for container creation: {:?}", namespace);

    crate::validate::validate_namespace(&namespace).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Invalid namespace: {}", err) })),
        )
    })?;
    debug!("Validated namespace");

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());

    debug!("Authorizing namespace");
    let owner = auth_ns(db_pool, &owner_ids, &namespace)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Authorization error: {}", e)})),
            )
        })?;
    debug!("Authorized namespace");
    let platform = platform_factory(
        container_request
            .clone()
            .platform
            .unwrap_or("runpod".to_string()),
    )
    .await;

    debug!("Declaring container with namespace: {:?}", namespace);
    let container = platform
        .declare(
            &container_request,
            db_pool,
            &user_profile,
            &owner,
            &namespace,
            None,
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
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

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
        &resolved_namespace,
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

    let platform = platform_factory(container.platform.unwrap().clone()).await;

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
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

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
        &resolved_namespace,
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

    let platform = platform_factory(container.platform.unwrap().clone()).await;

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

#[axum::debug_handler]
pub async fn patch_container(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
    Json(update_request): Json<V1UpdateContainer>,
) -> Result<Json<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

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
    let container = match Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
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

    let container_ref = &container;
    let no_delete = update_request.no_delete.unwrap_or(false);

    let container_env = container
        .env
        .clone()
        .and_then(|json_value| serde_json::from_value::<Vec<V1EnvVar>>(json_value).ok())
        .unwrap_or_default();

    let container_volumes = container
        .volumes
        .clone()
        .and_then(|json_value| serde_json::from_value::<Vec<V1VolumePath>>(json_value).ok())
        .unwrap_or_default();

    let container_resources = container
        .resources
        .clone()
        .and_then(|json_value| serde_json::from_value::<V1ContainerResources>(json_value).ok())
        .unwrap_or_default();

    let container_meters = container
        .meters
        .clone()
        .and_then(|json_value| serde_json::from_value::<Vec<V1Meter>>(json_value).ok())
        .unwrap_or_default();

    let container_health_check = container
        .health_check
        .clone()
        .and_then(|json_value| serde_json::from_value::<V1ContainerHealthCheck>(json_value).ok())
        .unwrap_or_default();

    let container_authz = container
        .authz
        .clone()
        .and_then(|json_value| serde_json::from_value::<V1AuthzConfig>(json_value).ok())
        .unwrap_or_default();

    //
    //
    //

    let updated_platform = update_request
        .platform
        .clone()
        .unwrap_or(container.platform.clone().unwrap_or_default());
    let updated_image = update_request
        .image
        .clone()
        .unwrap_or(container.image.clone());
    // let updated_ports = update_request.ports.clone().unwrap_or(container.ports);
    // let updated_authz = update_request.authz.clone().unwrap_or(container.authz);

    let updated_env = update_request.env.clone().unwrap_or(container_env.clone());
    let updated_command = update_request
        .command
        .clone()
        .unwrap_or_else(|| container.command.clone().unwrap_or_default());
    let updated_args = update_request
        .args
        .clone()
        .or_else(|| container.args.clone());
    let updated_volumes = update_request
        .volumes
        .clone()
        .unwrap_or(container_volumes.clone());
    let updated_accelerators = update_request
        .accelerators
        .clone()
        .unwrap_or_else(|| container.accelerators.clone().unwrap_or_default());
    let updated_resources = update_request
        .resources
        .clone()
        .unwrap_or(container_resources.clone());
    let updated_meters = update_request
        .meters
        .clone()
        .unwrap_or(container_meters.clone());
    let updated_restart = update_request
        .restart
        .clone()
        .unwrap_or_else(|| container.restart.clone());
    let updated_queue = update_request
        .queue
        .clone()
        .or_else(|| container.queue.clone());
    let updated_timeout = update_request
        .timeout
        .clone()
        .or_else(|| container.timeout.clone());
    let updated_proxy_port = update_request
        .proxy_port
        .clone()
        .unwrap_or_else(|| container.proxy_port.clone().unwrap_or_default());
    let updated_health_check = update_request
        .health_check
        .clone()
        .unwrap_or_else(|| container_health_check.clone());
    let updated_authz = update_request
        .authz
        .clone()
        .unwrap_or_else(|| container_authz.clone());

    // Log changes in debug
    {
        {
            debug!("Comparing new container fields with old container fields");
        }
    }
    {
        {
            if updated_platform != container.platform.clone().unwrap_or_default() {
                debug!(
                    "platform changed from '{:?}' to '{:?}'",
                    container.platform.clone().unwrap_or_default(),
                    updated_platform
                );
            }
        }
    }
    {
        {
            if updated_image != container.image {
                debug!(
                    "image changed from '{:?}' to '{:?}'",
                    container.image, updated_image
                );
            }
        }
    }
    {
        {
            let container_env_clone = container_env.clone();
            if Some(updated_env.clone()) != Some(container_env_clone.clone()) {
                debug!(
                    "env changed from '{:?}' to '{:?}'",
                    container_env_clone,
                    updated_env.clone()
                );
            }
        }
    }
    {
        {
            if Some(updated_command.clone()) != container.command.clone() {
                debug!(
                    "command changed from '{:?}' to '{:?}'",
                    container.command, updated_command
                );
            }
        }
    }
    {
        {
            if Some(updated_args.clone()) != Some(container.args.clone()) {
                debug!(
                    "args changed from '{:?}' to '{:?}'",
                    container.args, updated_args
                );
            }
        }
    }
    {
        {
            let container_volumes_clone = container_volumes.clone();
            if Some(updated_volumes.clone()) != Some(container_volumes_clone.clone()) {
                debug!(
                    "volumes changed from '{:?}' to '{:?}'",
                    container_volumes_clone,
                    updated_volumes.clone()
                );
            }
        }
    }
    {
        {
            if Some(updated_accelerators.clone()) != container.accelerators.clone() {
                debug!(
                    "accelerators changed from '{:?}' to '{:?}'",
                    container.accelerators, updated_accelerators
                );
            }
        }
    }
    {
        {
            let container_resources_clone = container_resources.clone();
            if Some(updated_resources.clone()) != Some(container_resources_clone.clone()) {
                debug!(
                    "resources changed from '{:?}' to '{:?}'",
                    container_resources_clone,
                    updated_resources.clone()
                );
            }
        }
    }
    {
        {
            let container_meters_clone = container_meters.clone();
            if Some(updated_meters.clone()) != Some(container_meters_clone.clone()) {
                debug!(
                    "meters changed from '{:?}' to '{:?}'",
                    container_meters_clone,
                    updated_meters.clone()
                );
            }
        }
    }
    {
        {
            if updated_restart != container.restart.clone() {
                debug!(
                    "restart changed from '{:?}' to '{:?}'",
                    container.restart, updated_restart
                );
            }
        }
    }
    {
        {
            if Some(updated_queue.clone()) != Some(container.queue.clone()) {
                debug!(
                    "queue changed from '{:?}' to '{:?}'",
                    container.queue, updated_queue
                );
            }
        }
    }
    {
        {
            if Some(updated_timeout.clone()) != Some(container.timeout.clone()) {
                debug!(
                    "timeout changed from '{:?}' to '{:?}'",
                    container.timeout, updated_timeout
                );
            }
        }
    }
    {
        {
            if Some(updated_proxy_port.clone()) != container.proxy_port.clone() {
                debug!(
                    "proxy_port changed from '{:?}' to '{:?}'",
                    container.proxy_port, updated_proxy_port
                );
            }
        }
    }
    {
        {
            if Some(updated_health_check.clone()) != Some(container_health_check.clone()) {
                debug!(
                    "health_check changed from '{:?}' to '{:?}'",
                    container.health_check, updated_health_check
                );
            }
        }
    }
    {
        {
            if Some(updated_authz.clone()) != Some(container_authz.clone()) {
                debug!(
                    "authz changed from '{:?}' to '{:?}'",
                    container.authz, updated_authz
                );
            }
        }
    }

    let changed_outside_metadata = {
        let container_platform = container.platform.clone();
        updated_platform.clone() != container_platform.unwrap_or_default()
            || updated_image.clone() != container.image
            // || updated_ssh_keys != container.ssh_keys
            // || updated_ports != container.ports
            // || updated_authz != container.authz
            || Some(updated_env.clone()) != Some(container_env)
            || Some(updated_command.clone()) != container.command
            || updated_args != container.args
            || Some(updated_volumes.clone()) != Some(container_volumes)
            || Some(updated_accelerators.clone()) != container.accelerators
            || Some(updated_resources.clone()) != Some(container_resources)
            || Some(updated_meters.clone()) != Some(container_meters)
            || updated_restart.clone() != container.restart
            || updated_queue != container.queue
            || updated_timeout != container.timeout
            || Some(updated_proxy_port.clone()) != container.proxy_port
            || Some(updated_health_check.clone()) != Some(container_health_check)
            || Some(updated_authz.clone()) != Some(container_authz)
    };

    // If anything changed, we may need to delete+recreate the container unless no_delete = true.
    if changed_outside_metadata {
        debug!("Container changed outside metadata");
        if no_delete {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Container changes require deletion, but no_delete=true"
                })),
            ));
        }

        debug!("Deleting old container");
        if let Err(e) = _delete_container_by_id(db_pool, &container.id, &user_profile).await {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete container: {:?}", e)})),
            ));
        }

        let request_meta = V1ResourceMetaRequest {
            name: Some(container.name.clone()),
            namespace: Some(container.namespace.clone()),
            ..Default::default()
        };

        // Now we create the new container with merged (old + new) values
        debug!("Creating new container with updated fields");
        let to_create = V1ContainerRequest {
            kind: "Container".to_string(),
            platform: Some(updated_platform),
            image: updated_image,
            ssh_keys: None,
            ports: None,
            metadata: Some(request_meta),
            env: Some(updated_env),
            command: Some(updated_command),
            args: updated_args,
            volumes: Some(updated_volumes),
            accelerators: Some(updated_accelerators),
            resources: Some(updated_resources),
            meters: Some(updated_meters),
            restart: updated_restart,
            queue: updated_queue,
            timeout: updated_timeout,
            proxy_port: Some(updated_proxy_port),
            health_check: Some(updated_health_check),
            authz: Some(updated_authz),
        };

        let platform = platform_factory(
            update_request
                .clone()
                .platform
                .unwrap_or("runpod".to_string()),
        )
        .await;
        let created = platform
            .declare(
                &to_create,
                db_pool,
                &user_profile,
                &user_profile.email,
                &namespace,
                None,
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
        debug!("Created new container: {:?}", created);

        return Ok(Json(created));
    } else {
        debug!("No changes to LLM server, skipping update");
    }

    Ok(Json(container_ref.to_v1_container().unwrap()))
}

pub async fn _search_containers(
    db_pool: &DatabaseConnection,
    search: &V1ContainerSearch,
    user_profile: &V1UserProfile,
) -> Result<Vec<V1Container>, (StatusCode, Json<serde_json::Value>)> {
    debug!("Searching for containers: {:?}", search);
    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();

    // Add user's email to owner IDs
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let mut conditions = Condition::all();

    // Add owner condition first
    conditions = conditions.add(containers::Column::Owner.is_in(owner_id_refs));

    // Rest of the search conditions remain the same
    if let Some(namespace) = &search.namespace {
        debug!("Searching for containers in namespace: {:?}", namespace);
        conditions = conditions.add(containers::Column::Namespace.eq(namespace));
    }

    // Rest of the search conditions remain the same
    if let Some(image) = &search.image {
        debug!("Searching for containers with image: {:?}", image);
        conditions = conditions.add(containers::Column::Image.eq(image));
    }

    if let Some(command) = &search.command {
        debug!("Searching for containers with command: {:?}", command);
        conditions = conditions.add(containers::Column::Command.eq(command));
    }

    if let Some(args) = &search.args {
        debug!("Searching for containers with args: {:?}", args);
        conditions = conditions.add(containers::Column::Args.eq(args));
    }

    if let Some(platform) = &search.platform {
        debug!("Searching for containers with platform: {:?}", platform);
        conditions = conditions.add(containers::Column::Platform.eq(platform));
    }

    if let Some(queue) = &search.queue {
        debug!("Searching for containers with queue: {:?}", queue);
        conditions = conditions.add(containers::Column::Queue.eq(queue));
    }

    if let Some(timeout) = &search.timeout {
        debug!("Searching for containers with timeout: {:?}", timeout);
        conditions = conditions.add(containers::Column::Timeout.eq(timeout));
    }

    if let Some(proxy_port) = &search.proxy_port {
        debug!("Searching for containers with proxy_port: {:?}", proxy_port);
        conditions = conditions.add(containers::Column::ProxyPort.eq(*proxy_port));
    }

    // For complex fields that are stored as JSON, we need to use proper JSON comparison operators
    if let Some(env) = &search.env {
        debug!("Searching for containers with env: {:?}", env);
        conditions = conditions.add(
            Expr::col(containers::Column::Env)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(env).unwrap()).cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(volumes) = &search.volumes {
        debug!("Searching for containers with volumes: {:?}", volumes);
        conditions = conditions.add(
            Expr::col(containers::Column::Volumes)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(volumes).unwrap()).cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(accelerators) = &search.accelerators {
        debug!(
            "Searching for containers with accelerators: {:?}",
            accelerators
        );
        conditions = conditions.add(
            Expr::col(containers::Column::Accelerators)
                .cast_as(Alias::new("text[]"))
                .eq(Expr::val(accelerators.clone())),
        );
    }

    if let Some(labels) = &search.labels {
        debug!("Searching for containers with labels: {:?}", labels);
        conditions = conditions.add(
            Expr::col(containers::Column::Labels)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(labels).unwrap()).cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(resources) = &search.resources {
        debug!("Searching for containers with resources: {:?}", resources);
        conditions = conditions.add(
            Expr::col(containers::Column::Resources)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(resources).unwrap())
                        .cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(meters) = &search.meters {
        debug!("Searching for containers with meters: {:?}", meters);
        conditions = conditions.add(
            Expr::col(containers::Column::Meters)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(meters).unwrap()).cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(health_check) = &search.health_check {
        debug!(
            "Searching for containers with health_check: {:?}",
            health_check
        );
        conditions = conditions.add(
            Expr::col(containers::Column::HealthCheck)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(health_check).unwrap())
                        .cast_as(Alias::new("jsonb")),
                ),
        );
    }

    if let Some(authz) = &search.authz {
        debug!("Searching for containers with authz: {:?}", authz);
        conditions = conditions.add(
            Expr::col(containers::Column::Authz)
                .cast_as(Alias::new("jsonb"))
                .contains(
                    Expr::val(serde_json::to_string(authz).unwrap()).cast_as(Alias::new("jsonb")),
                ),
        );
    }

    debug!("Conditions: {:?}", conditions);
    // Execute the query with ownership check included
    let containers = containers::Entity::find()
        .filter(conditions)
        .all(db_pool)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            )
        })?;

    debug!("Found {} containers", containers.len());

    // Convert the database models to V1Container
    let v1_containers = containers
        .into_iter()
        .filter_map(|c| c.to_v1_container().ok())
        .collect();

    debug!("Converted containers: {:?}", v1_containers);
    Ok(v1_containers)
}

#[axum::debug_handler]
pub async fn search_containers(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Json(search): Json<V1ContainerSearch>,
) -> Result<Json<V1Containers>, (StatusCode, Json<serde_json::Value>)> {
    debug!("Searching for containers: {:?}", search);
    let db_pool = &state.db_pool;

    let containers = _search_containers(db_pool, &search, &user_profile).await?;

    Ok(Json(V1Containers { containers }))
}

// At the end of the file, add WebSocket support for streaming logs
pub async fn stream_logs_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> impl IntoResponse {
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);
    ws.on_upgrade(move |socket| {
        handle_socket(socket, state, user_profile, resolved_namespace, name)
    })
}

pub async fn stream_logs_ws_by_id(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket_by_id(socket, state, user_profile, id))
}

async fn handle_socket_by_id(
    socket: WebSocket,
    state: AppState,
    user_profile: V1UserProfile,
    id: String,
) {
    debug!(
        "WebSocket upgrade request received for container ID: {}",
        id
    );
    // Middleware already handled auth. User profile is passed via Extension.
    let db_pool = &state.db_pool;
    let (sender, _receiver) = socket.split(); // Receiver is not used anymore

    // Fetch container info
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    match Query::find_container_by_id_and_owners(db_pool, &id, &owner_id_refs).await {
        Ok(container) => {
            // Start streaming logs (passing only the sender)
            stream_container_logs(sender, container.id.to_string()).await;
        }
        Err(e) => {
            // If container fetch fails AFTER successful auth/upgrade, send error on socket
            let mut sender_locked = sender;
            let _ = sender_locked
                .send(Message::Text(format!(
                    "Error fetching container data: {}",
                    e
                )))
                .await;
            let _ = sender_locked.close().await;
        }
    }
}

async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    user_profile: V1UserProfile,
    namespace: String,
    name: String,
) {
    debug!(
        "WebSocket upgrade request received for container: {}/{}",
        namespace, name
    );
    // Middleware already handled auth. User profile is passed via Extension.
    let db_pool = &state.db_pool;
    let (sender, _receiver) = socket.split(); // Receiver is not used anymore

    // Fetch container info
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    match Query::find_container_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(container) => {
            // Start streaming logs
            stream_container_logs(sender, container.id.to_string()).await;
        }
        Err(e) => {
            // If container fetch fails AFTER successful auth/upgrade, send error on socket
            let mut sender_locked = sender;
            let _ = sender_locked
                .send(Message::Text(format!(
                    "Error fetching container data: {}",
                    e
                )))
                .await;
            let _ = sender_locked.close().await;
        }
    }
}

async fn stream_container_logs<S>(sender: S, container_id: String)
where
    S: SinkExt<Message> + Unpin + Send + 'static,
    <S as futures::Sink<Message>>::Error: std::fmt::Debug + Send,
{
    let ssh_host = format!("container-{}", container_id);

    // Use tokio::process to spawn an async process
    let mut cmd = tokio::process::Command::new("ssh");
    cmd.arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-l")
        .arg("root") // TODO: get user from API
        .arg(ssh_host)
        .arg("tail")
        .arg("-f")
        .arg("$HOME/.logs/nebu_container.log")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()); // Capture stderr too

    // Wrap the sender in Arc<Mutex> *before* the match
    let sender = Arc::new(Mutex::new(sender));

    // Execute command
    match cmd.spawn() {
        Ok(mut child) => {
            let stdout = child.stdout.take().expect("Failed to capture stdout");
            let stderr = child.stderr.take().expect("Failed to capture stderr");

            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stderr_reader = BufReader::new(stderr).lines();

            // Clone the Arc for the tasks
            let stdout_sender = Arc::clone(&sender);
            let stdout_handle = tokio::spawn(async move {
                while let Ok(Some(line)) = stdout_reader.next_line().await {
                    let mut sender_lock = stdout_sender.lock().await;
                    if sender_lock.send(Message::Text(line)).await.is_err() {
                        break; // Client disconnected
                    }
                }
            });

            let stderr_sender = Arc::clone(&sender);
            let stderr_handle = tokio::spawn(async move {
                while let Ok(Some(line)) = stderr_reader.next_line().await {
                    let err_line = format!("STDERR: {}", line); // Prefix stderr lines
                    let mut sender_lock = stderr_sender.lock().await;
                    if sender_lock.send(Message::Text(err_line)).await.is_err() {
                        break; // Client disconnected
                    }
                }
            });

            // Wait for child process to exit or for streams to end
            let status = child.wait().await;
            debug!("SSH command finished with status: {:?}", status);

            // Wait for reader tasks to finish
            let _ = tokio::join!(stdout_handle, stderr_handle);

            // Send final message indicating process exit
            {
                let mut sender_lock = sender.lock().await; // Use the Arc<Mutex> sender
                let final_message = format!(
                    "Log stream ended (SSH process exited with status: {:?})",
                    status
                );
                let _ = sender_lock.send(Message::Text(final_message)).await;
            }
        }
        Err(e) => {
            let mut sender_lock = sender.lock().await; // Now this works
            let _ = sender_lock
                .send(Message::Text(format!("Error starting log stream: {}", e)))
                .await;
        }
    }

    // Close the WebSocket connection from the server side
    {
        let mut sender_lock = sender.lock().await;
        let _ = sender_lock.close().await;
        debug!("WebSocket connection closed by server.");
    }
}
