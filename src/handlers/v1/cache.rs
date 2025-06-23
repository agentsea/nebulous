use crate::models::V1UserProfile;
use crate::query::Query;
use crate::state::{AppState, MessageQueue};
use axum::{
    extract::{Extension, Path, Query as QueryParam, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use futures::stream::{self, StreamExt};
use redis::AsyncCommands;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use tracing::{debug, error, info};

#[derive(Deserialize, Debug)]
pub struct CacheKeyParams {
    namespace: Option<String>,
}

/// Handler: List cache keys for a given namespace or all accessible namespaces.
pub async fn list_cache_keys(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    QueryParam(params): QueryParam<CacheKeyParams>,
) -> Result<Json<Vec<String>>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    let requested_namespace = params.namespace;

    info!(
        "Listing cache keys (namespace: {:?}) requested by user: {}",
        requested_namespace, user_profile.email
    );

    // 1. Get Redis client from AppState
    let redis_client = match &state.message_queue {
        MessageQueue::Redis { client } => client.clone(),
        _ => {
            error!("Redis client not available in AppState. Cache operations require Redis.");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Redis client not configured on the server." })),
            ));
        }
    };

    let mut all_keys = HashSet::new(); // Use HashSet to avoid duplicates

    // Determine which namespaces to scan
    let namespaces_to_scan: Vec<String> = if let Some(ns) = requested_namespace {
        // If a specific namespace is requested, just use that one
        info!("Scanning specific namespace: {}", ns);
        vec![ns]
    } else {
        // If no namespace is requested, find all namespaces the user has access to
        info!(
            "No specific namespace provided. Fetching all accessible namespaces for user {}",
            user_profile.email
        );
        let mut owner_ids: Vec<String> = user_profile
            .organizations
            .as_ref()
            .map(|orgs| orgs.keys().cloned().collect())
            .unwrap_or_default();
        owner_ids.push(user_profile.email.clone());
        let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

        match Query::find_namespaces_by_owners(db_pool, &owner_id_refs).await {
            Ok(namespaces) => {
                let ns_names: Vec<String> = namespaces.into_iter().map(|n| n.id).collect();
                info!(
                    "Found {} accessible namespaces: {:?}",
                    ns_names.len(),
                    ns_names
                );
                ns_names
            }
            Err(e) => {
                error!(
                    "Failed to query namespaces for owners {:?}: {}",
                    owner_id_refs, e
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Failed to retrieve accessible namespaces." })),
                ));
            }
        }
    };

    // 2. Scan Redis for keys in the determined namespaces concurrently
    let results = stream::iter(namespaces_to_scan)
        .map(|ns| {
            let client = redis_client.clone();
            async move {
                let pattern = format!("cache:{}:*", ns);
                debug!("Scanning Redis with pattern: {}", pattern);
                let mut keys_for_ns = Vec::new();
                let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        error!("Failed to get Redis connection for namespace {}: {}", ns, e);
                        return Err(format!("Redis connection failed for namespace {}", ns));
                    }
                };
                let mut iter: redis::AsyncIter<String> = match conn.scan_match(&pattern).await {
                    Ok(iter) => iter,
                    Err(e) => {
                        error!("Redis SCAN failed for pattern {}: {}", pattern, e);
                        return Err(format!("Redis SCAN failed for namespace {}", ns));
                    }
                };
                while let Some(key) = iter.next_item().await {
                    keys_for_ns.push(key);
                }
                debug!("Found {} keys for namespace {}", keys_for_ns.len(), ns);
                Ok(keys_for_ns)
            }
        })
        .buffer_unordered(10) // Allow up to 10 scans to run concurrently
        .collect::<Vec<Result<Vec<String>, String>>>()
        .await;

    // 3. Aggregate results and handle errors
    for result in results {
        match result {
            Ok(keys) => {
                for key in keys {
                    all_keys.insert(key);
                }
            }
            Err(e) => {
                // Log the error, but continue aggregating results from other namespaces
                error!("Error during Redis scan: {}", e);
                // Optionally, you could return an error here if any scan fails
                // return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))));
            }
        }
    }

    let final_keys: Vec<String> = all_keys.into_iter().collect();
    info!(
        "Found {} total unique cache keys across scanned namespaces",
        final_keys.len()
    );

    // 4. Return the aggregated keys
    Ok(Json(final_keys))
}

/// Handler: Get a specific cache key's value
pub async fn get_cache_key(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, key_suffix)): Path<(String, String)>,
) -> Result<Json<String>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    info!(
        "Getting cache key suffix '{}' in namespace '{}' requested by user: {}",
        key_suffix, namespace, user_profile.email
    );

    // 1. Verify user access to the namespace
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let accessible_namespaces =
        match Query::find_namespaces_by_owners(db_pool, &owner_id_refs).await {
            Ok(namespaces) => namespaces
                .into_iter()
                .map(|n| n.id)
                .collect::<HashSet<String>>(),
            Err(e) => {
                error!(
                    "Failed to query namespaces for owners {:?}: {}",
                    owner_id_refs, e
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Failed to retrieve accessible namespaces." })),
                ));
            }
        };

    if !accessible_namespaces.contains(&namespace) {
        error!(
            "User {} does not have access to namespace '{}'",
            user_profile.email, namespace
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied to the specified namespace." })),
        ));
    }

    // 2. Get Redis client
    let redis_client = match &state.message_queue {
        MessageQueue::Redis { client } => client.clone(),
        _ => {
            error!("Redis client not available in AppState.");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Redis client not configured." })),
            ));
        }
    };

    // 3. Get Redis connection
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to get async Redis connection: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to connect to Redis." })),
            ));
        }
    };

    // 4. Construct the full key
    // The key_suffix captured by `*` might start with a '/', remove it if present.
    let clean_key_suffix = key_suffix.strip_prefix('/').unwrap_or(&key_suffix);
    let full_key = format!("cache:{}:{}", namespace, clean_key_suffix);
    info!("Attempting to GET key: {}", full_key);

    // 5. Execute GET command
    match conn.get::<_, Option<String>>(&full_key).await {
        Ok(Some(value)) => {
            debug!("Found value for key {}: (value hidden)", full_key);
            Ok(Json(value))
        }
        Ok(None) => {
            info!("Key not found: {}", full_key);
            Err((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Cache key not found." })),
            ))
        }
        Err(e) => {
            error!("Redis GET command failed for key {}: {}", full_key, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to execute Redis GET command." })),
            ))
        }
    }
}

/// Handler: Delete a specific cache key
pub async fn delete_cache_key(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, key_suffix)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    // Return StatusCode on success
    let db_pool = &state.db_pool;
    info!(
        "Deleting cache key suffix '{}' in namespace '{}' requested by user: {}",
        key_suffix, namespace, user_profile.email
    );

    // 1. Verify user access to the namespace (same logic as get_cache_key)
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let accessible_namespaces =
        match Query::find_namespaces_by_owners(db_pool, &owner_id_refs).await {
            Ok(namespaces) => namespaces
                .into_iter()
                .map(|n| n.id)
                .collect::<HashSet<String>>(),
            Err(e) => {
                error!(
                    "Failed to query namespaces for owners {:?}: {}",
                    owner_id_refs, e
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Failed to retrieve accessible namespaces." })),
                ));
            }
        };

    if !accessible_namespaces.contains(&namespace) {
        error!(
            "User {} does not have access to namespace '{}'",
            user_profile.email, namespace
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Access denied to the specified namespace." })),
        ));
    }

    // 2. Get Redis client
    let redis_client = match &state.message_queue {
        MessageQueue::Redis { client } => client.clone(),
        _ => {
            error!("Redis client not available in AppState.");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Redis client not configured." })),
            ));
        }
    };

    // 3. Get Redis connection
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to get async Redis connection: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to connect to Redis." })),
            ));
        }
    };

    // 4. Construct the full key
    let clean_key_suffix = key_suffix.strip_prefix('/').unwrap_or(&key_suffix);
    let full_key = format!("cache:{}:{}", namespace, clean_key_suffix);
    info!("Attempting to DEL key: {}", full_key);

    // 5. Execute DEL command
    match conn.del::<_, i32>(&full_key).await {
        // DEL returns the number of keys deleted
        Ok(num_deleted) => {
            info!(
                "DEL command for key '{}' deleted {} keys.",
                full_key, num_deleted
            );
            Ok(StatusCode::OK) // Return 200 OK regardless of whether the key existed
        }
        Err(e) => {
            error!("Redis DEL command failed for key {}: {}", full_key, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to execute Redis DEL command." })),
            ))
        }
    }
}
