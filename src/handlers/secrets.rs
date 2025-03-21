use crate::{
    entities::secrets, models::V1UserProfile, mutation::Mutation, query::Query, state::AppState,
};
use axum::{
    extract::{Extension, Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing::info;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct V1Secret {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub created_by: String,
    pub labels: HashMap<String, serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Plaintext of the secret (be cautious about returning this!)
    pub value: Option<String>,
}

/// Request body used for creating or updating a secret
#[derive(Serialize, Deserialize, Debug)]
pub struct V1SecretRequest {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub labels: HashMap<String, serde_json::Value>,
}

/// Handler: List secrets for the current user (and their organizations)
pub async fn list_secrets(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
) -> Result<Json<Vec<V1Secret>>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Gather all possible owner IDs from user + organizations
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    info!("Listing secrets for user: {}", owner_ids.join(", "));

    // Retrieve secrets
    let secrets_list = Query::find_secrets_by_owners(db_pool, &owner_id_refs)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Database error: {}", err) })),
            )
        })?;

    info!("Found {} secrets", secrets_list.len());

    // Transform them into our V1Secret response (decrypted if needed)
    let response = secrets_list
        .into_iter()
        .map(|secret| {
            let decrypted_value = secret.decrypt_value().ok(); // returns Result; ignore errors
            V1Secret {
                kind: "Secret".to_string(),
                id: secret.id,
                name: secret.name,
                owner_id: secret.owner_id,
                created_by: secret.created_by.unwrap_or_default(),
                labels: secret
                    .labels
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
                created_at: secret.created_at.timestamp(),
                updated_at: secret.updated_at.timestamp(),
                value: decrypted_value,
            }
        })
        .collect();

    Ok(Json(response))
}

/// Handler: Get a single secret by ID
pub async fn get_secret(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(id): Path<String>,
) -> Result<Json<V1Secret>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Gather owners
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    info!("Getting secret for user: {}", owner_ids.join(", "));

    // Fetch secret from DB
    let secret_model = Query::find_secret_by_id_and_owners(db_pool, &id, &owner_id_refs)
        .await
        .map_err(|err| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Secret not found: {}", err) })),
            )
        })?;

    info!("Found secret: {}", secret_model.id);

    // Decrypt
    let decrypted_value = secret_model.decrypt_value().ok();

    let secret_response = V1Secret {
        kind: "Secret".to_string(),
        id: secret_model.id,
        name: secret_model.name,
        owner_id: secret_model.owner_id,
        created_by: secret_model.created_by.unwrap_or_default(),
        labels: secret_model
            .labels
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        created_at: secret_model.created_at.timestamp(),
        updated_at: secret_model.updated_at.timestamp(),
        value: decrypted_value,
    };

    Ok(Json(secret_response))
}

/// Handler: Create a new secret
pub async fn create_secret(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Json(payload): Json<V1SecretRequest>,
) -> Result<Json<V1Secret>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Generate a unique ID for the secret. You might use `short_uuid`, etc.
    let secret_id = format!("secret-{}", uuid::Uuid::new_v4());

    // Create the new Model, which will auto-encrypt the secret value
    let secret_model = secrets::Model::new(
        secret_id.clone(),
        payload.name.clone(),
        user_profile.email.clone(), // owner_id
        &payload.value,
        Some(user_profile.email.clone()),        // created_by
        Some(serde_json::json!(payload.labels)), // labels
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to encrypt secret: {}", err) })),
        )
    })?;

    // Insert into DB
    let inserted = secrets::ActiveModel::from(secret_model)
        .insert(db_pool)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to store secret: {}", err) })),
            )
        })?;

    let created_by = inserted.created_by.clone().unwrap_or_default();
    let labels = inserted
        .labels
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Now decrypt using `inserted`, leaving the prior fields intact
    let decrypted_value = inserted.decrypt_value().ok();

    // Finally build the response
    let response = V1Secret {
        kind: "Secret".to_string(),
        id: inserted.id,
        name: inserted.name,
        owner_id: inserted.owner_id,
        created_by,
        labels,
        created_at: inserted.created_at.timestamp(),
        updated_at: inserted.updated_at.timestamp(),
        value: decrypted_value,
    };

    Ok(Json(response))
}

/// Handler: Update an existing secret
pub async fn update_secret(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(secret_id): Path<String>,
    Json(payload): Json<V1SecretRequest>,
) -> Result<Json<V1Secret>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Gather owners
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Fetch the model to ensure it exists and user can access
    let existing_secret = Query::find_secret_by_id_and_owners(db_pool, &secret_id, &owner_id_refs)
        .await
        .map_err(|err| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Secret not found: {}", err) })),
            )
        })?;

    // Perform the update
    let updated_secret = Mutation::update_secret(
        db_pool,
        existing_secret,
        Some(payload.name.clone()),
        // Provide new_value if you want to re-encrypt. If you want partial updates, handle Option.
        Some(payload.value.clone()),
        Some(json!(payload.labels)),
    )
    .await
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to update secret: {}", err) })),
        )
    })?;

    // Decrypt the newly updated secret
    let decrypted_value = updated_secret.decrypt_value().ok();

    // Build response
    let response = V1Secret {
        kind: "Secret".to_string(),
        id: updated_secret.id,
        name: updated_secret.name,
        owner_id: updated_secret.owner_id,
        created_by: updated_secret.created_by.unwrap_or_default(),
        labels: updated_secret
            .labels
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        created_at: updated_secret.created_at.timestamp(),
        updated_at: updated_secret.updated_at.timestamp(),
        value: decrypted_value,
    };

    Ok(Json(response))
}

/// Handler: Delete a secret
pub async fn delete_secret(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path(secret_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // Gather owners
    let mut owner_ids: Vec<String> = user_profile
        .organizations
        .as_ref()
        .map(|orgs| orgs.keys().cloned().collect())
        .unwrap_or_default();
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Make sure the secret is accessible
    let _ = Query::find_secret_by_id_and_owners(db_pool, &secret_id, &owner_id_refs)
        .await
        .map_err(|err| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Secret not found: {}", err) })),
            )
        })?;

    // Actually delete
    let result = Mutation::delete_secret(db_pool, secret_id)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete secret: {}", err)})),
            )
        })?;

    if result.rows_affected == 0 {
        // No rows were deleted
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Secret not found or already deleted"})),
        ));
    }

    // Return a 200 OK
    Ok(StatusCode::OK)
}
