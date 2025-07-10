use crate::agent::aws::{
    create_s3_scoped_user, delete_s3_scoped_user, generate_temporary_s3_credentials,
    IamCredentials, StsCredentials,
};
use crate::agent::ns::auth_ns;
use crate::config::SERVER_CONFIG;
use crate::models::{V1ResourceMeta, V1UserProfile};
use crate::state::AppState;
use aws_config::{self, BehaviorVersion, Region};
use aws_sdk_iam::Client as IamClient;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::json;
use tracing::{debug, error};

#[derive(Serialize)]
pub struct V1IamCredentialsResponse {
    kind: String,
    metadata: V1ResourceMeta,
    username: String,
    access_key_id: String,
    secret_access_key: String,
    base_key: String,
}

#[derive(Serialize)]
pub struct V1StsCredentialsResponse {
    kind: String,
    metadata: V1ResourceMeta,
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expiration: Option<i64>,
    s3_base_uri: String,
}

/// Handler: Create a new S3-scoped IAM user for a given namespace and name
pub async fn create_scoped_s3_token(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1IamCredentialsResponse>, (StatusCode, Json<serde_json::Value>)> {
    debug!(?namespace, ?name, "Entered create_scoped_s3_token handler");
    let db_pool = &state.db_pool;

    // --- Authorization ---
    debug!("Starting authorization step");
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    // Also allow authorization if the namespace matches the user's handle
    if let Some(handle) = &user_profile.handle {
        owner_ids.push(handle.clone());

        debug!("Ensuring namespace: {}", handle);
        match crate::handlers::v1::namespaces::ensure_namespace(
            db_pool,
            &handle,
            &user_profile.email,
            &user_profile.email,
            None,
        )
        .await
        {
            Ok(_) => (),
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("Invalid namespace: {}", e) })),
                ));
            }
        }
    }
    owner_ids.push(user_profile.email.clone());
    debug!(?owner_ids, "Constructed owner_ids for authorization check");

    debug!("Calling auth_ns");
    let owner = match auth_ns(db_pool, &owner_ids, &namespace).await {
        Ok(owner) => {
            debug!(?owner, "auth_ns successful");
            owner
        }
        Err(e) => {
            error!("Authorization failed for namespace {}: {}", namespace, e);
            debug!("Returning 403 Forbidden due to auth_ns failure");
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("Not authorized for namespace '{}'", namespace)})),
            ));
        }
    };

    // --- Get Bucket Name from Config ---
    // Bucket name is read from global SERVER_CONFIG at startup
    let bucket_name = SERVER_CONFIG.bucket_name.clone();
    debug!(?bucket_name, "Retrieved bucket name from config");

    // --- Call AWS Agent ---
    debug!("Calling create_s3_scoped_user");
    let credentials = match create_s3_scoped_user(&bucket_name, &namespace, &name).await {
        Ok(creds) => {
            debug!("create_s3_scoped_user successful");
            creds
        }
        Err(e) => {
            error!(
                "Failed to create S3 scoped user '{}/{}': {}",
                namespace, name, e
            );
            debug!("Returning 500 Internal Server Error due to create_s3_scoped_user failure");
            // Consider mapping specific AWS errors to different HTTP status codes
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to create AWS IAM user: {}", e)
                })),
            ));
        }
    };

    // --- Format Response ---
    debug!("Formatting successful response");
    // We re-use V1ResourceMeta but note some fields aren't directly applicable
    // We'll use the IAM username generated as the 'name' in the metadata
    let response = V1IamCredentialsResponse {
        kind: "IamCredentials".to_string(),
        metadata: V1ResourceMeta {
            id: credentials.username.clone(),
            name: name.clone(),
            namespace: namespace.clone(),
            owner: owner.clone(),
            owner_ref: None,
            created_by: user_profile.email,
            labels: None,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        },
        username: credentials.username.clone(),
        access_key_id: credentials.access_key_id,
        secret_access_key: credentials.secret_access_key,
        base_key: format!("s3://{}/data/{}", bucket_name, namespace),
    };

    debug!("Returning Ok response");
    Ok(Json(response))
}

/// Handler: Delete an S3-scoped IAM user for a given namespace and name
pub async fn delete_scoped_s3_token(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    // --- Authorization ---
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    // Also allow authorization if the namespace matches the user's handle
    if let Some(handle) = &user_profile.handle {
        owner_ids.push(handle.clone());
    }
    owner_ids.push(user_profile.email.clone());

    match auth_ns(db_pool, &owner_ids, &namespace).await {
        Ok(_) => (),
        Err(e) => {
            error!(
                "Authorization failed for delete request on namespace {}: {}",
                namespace, e
            );
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("Not authorized for namespace '{}'", namespace)})),
            ));
        }
    };

    // --- Call AWS Agent to Delete ---
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let client = IamClient::new(&config);

    match delete_s3_scoped_user(&namespace, &name).await {
        Ok(_) => {
            // Deletion successful
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            error!(
                "Failed to delete S3 scoped user '{}/{}': {}",
                namespace, name, e
            );
            // Consider mapping specific AWS errors (e.g., user not found) if needed
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to delete AWS IAM user: {}", e)
                })),
            ));
        }
    }
}

/// Handler: Generate temporary S3 credentials for a given namespace and name
pub async fn generate_temp_s3_credentials(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1StsCredentialsResponse>, (StatusCode, Json<serde_json::Value>)> {
    debug!(
        ?namespace,
        ?name,
        "Entered generate_temp_s3_credentials handler"
    );
    let db_pool = &state.db_pool;

    // --- Authorization ---
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    if let Some(handle) = &user_profile.handle {
        owner_ids.push(handle.clone());

        debug!("Ensuring namespace: {}", handle);
        match crate::handlers::v1::namespaces::ensure_namespace(
            db_pool,
            &handle,
            &user_profile.email,
            &user_profile.email,
            None,
        )
        .await
        {
            Ok(_) => (),
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("Invalid namespace: {}", e) })),
                ));
            }
        }
    }
    owner_ids.push(user_profile.email.clone());

    let owner = match auth_ns(db_pool, &owner_ids, &namespace).await {
        Ok(owner) => owner,
        Err(e) => {
            error!("Authorization failed for namespace {}: {}", namespace, e);
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("Not authorized for namespace '{}'", namespace)})),
            ));
        }
    };

    // --- Get Bucket Name from Config ---
    let bucket_name = SERVER_CONFIG.bucket_name.clone();

    // Default duration: 1 hour (3600 seconds)
    let duration_seconds = 3600;

    // --- Call AWS Agent with inline policy ---
    let credentials =
        match generate_temporary_s3_credentials(&bucket_name, &namespace, duration_seconds).await {
            Ok(creds) => creds,
            Err(e) => {
                error!(
                    "Failed to generate temporary S3 credentials '{}/{}': {}",
                    namespace, name, e
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to generate temporary AWS credentials: {}", e)
                    })),
                ));
            }
        };

    // Convert expiration DateTime to Unix timestamp if present
    let expiration_timestamp = credentials.expiration.map(|dt| dt.as_secs_f64() as i64);

    // --- Format Response ---
    let response = V1StsCredentialsResponse {
        kind: "StsCredentials".to_string(),
        metadata: V1ResourceMeta {
            id: format!("sts-{}-{}", namespace, name),
            name: name.clone(),
            namespace: namespace.clone(),
            owner: owner.clone(),
            owner_ref: None,
            created_by: user_profile.email,
            labels: None,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        },
        access_key_id: credentials.access_key_id,
        secret_access_key: credentials.secret_access_key,
        session_token: credentials.session_token,
        expiration: expiration_timestamp,
        s3_base_uri: format!("s3://{}/data/{}", bucket_name, namespace),
    };

    Ok(Json(response))
}
