use crate::auth;
use crate::config::{ClientConfig, ServerConfig, SERVER_CONFIG};
use crate::models::V1UserProfile;
use crate::AppState;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::DatabaseConnection;
use serde_json::json;
use tracing::debug;

pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let db_pool = &state.db_pool;
    let auth_header = {
        match request.headers().get("Authorization") {
            Some(header) => header.to_str().unwrap_or("").to_string(),
            None => {
                println!("No Authorization header");
                return unauthorized_response();
            }
        }
    };
    if auth_header.starts_with("Bearer ") {
        let token = auth_header.trim_start_matches("Bearer ");

        if token.is_empty() {
            println!("Bearer token is empty");
            unauthorized_response()
        } else {
            match get_user_profile_from_token(db_pool, token).await {
                Ok(user_profile) => {
                    let mut req = request;
                    req.extensions_mut().insert(user_profile);
                    next.run(req).await
                }
                Err(_) => unauthorized_response(),
            }
        }
    } else {
        println!("Invalid Authorization header format");
        unauthorized_response()
    }
}

pub async fn internal_auth(
    db_conn: &DatabaseConnection,
    token: &str,
    mut request: Request,
    next: Next,
) -> Response {
    match get_user_profile_from_internal_token(db_conn, token).await {
        Ok(user_profile) => {
            request.extensions_mut().insert(user_profile);
            next.run(request).await
        }
        Err(_) => unauthorized_response(),
    }
}

pub async fn external_auth(auth_header: &String, mut request: Request, next: Next) -> Response {
    let token = auth_header.trim_start_matches("Bearer ");
    match get_user_profile_from_external_token(token).await {
        Ok(user_profile) => {
            request.extensions_mut().insert(user_profile);
            next.run(request).await
        }
        Err(_) => unauthorized_response(),
    }
}

/// Get a user profile from any token (internal or external)
pub async fn get_user_profile_from_token(
    db_conn: &DatabaseConnection,
    token: &str,
) -> Result<V1UserProfile, StatusCode> {
    if token.starts_with("nebu-") {
        get_user_profile_from_internal_token(db_conn, token).await
    } else {
        get_user_profile_from_external_token(token).await
    }
}

/// Get a user profile from an internal token
pub async fn get_user_profile_from_internal_token(
    db_conn: &DatabaseConnection,
    token: &str,
) -> Result<V1UserProfile, StatusCode> {
    debug!("Validating internal token: {}", token);
    let is_valid = auth::api::validate_api_key(db_conn, token).await;
    match is_valid {
        Ok(is_valid) => {
            if is_valid {
                println!("✅ Internal token is valid");

                let user_profile = V1UserProfile {
                    email: "dummy@example.com".to_string(),
                    display_name: None,
                    handle: None,
                    picture: None,
                    organization: None,
                    role: None,
                    external_id: None,
                    actor: None,
                    organizations: None,
                    created: None,
                    updated: None,
                    token: None,
                };
                Ok(user_profile)
            } else {
                println!("❌ Internal token is invalid");
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        Err(_) => {
            println!("❌ Failed to validate internal token");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Get a user profile from an external token
pub async fn get_user_profile_from_external_token(
    token: &str,
) -> Result<V1UserProfile, StatusCode> {
    let client_config = ClientConfig::read().unwrap();

    let auth_server = client_config.get_current_server_config().map_or_else(
        || SERVER_CONFIG.auth.url.clone(),
        |server_config| {
            server_config
                .auth_server
                .clone()
                .unwrap_or_else(|| SERVER_CONFIG.auth.url.clone())
        },
    );

    let auth_url = format!("{}/v1/users/me", auth_server);
    println!("🔐 Making auth request to: {}", auth_url);

    // Validate the token with auth server
    let client = reqwest::Client::new();
    let auth_header = format!("Bearer {}", token);
    let user_profile_result = client
        .get(auth_url)
        .header("Authorization", &auth_header)
        .send()
        .await;

    match user_profile_result {
        Ok(response) => {
            if response.status().is_success() {
                let response_text = response.text().await.unwrap_or_default();
                println!("✅ External auth response received");

                match serde_json::from_str::<V1UserProfile>(&response_text) {
                    Ok(user_profile) => Ok(user_profile),
                    Err(e) => {
                        println!("❌ Failed to parse user profile: {}", e);
                        Err(StatusCode::UNAUTHORIZED)
                    }
                }
            } else {
                println!("❌ External auth failed with status: {}", response.status());
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        Err(e) => {
            println!("❌ External auth request failed: {}", e);
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

fn unauthorized_response() -> Response {
    let error_response = json!({
        "error": {
            "message": "Unauthorized",
            "type": "authentication_error",
            "param": null,
            "code": null
        }
    });
    (StatusCode::UNAUTHORIZED, Json(error_response)).into_response()
}
