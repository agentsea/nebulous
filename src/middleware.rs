use crate::models::V1UserProfile;
use crate::AppState;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::ColIdx;
use serde_json::json;

pub async fn auth_middleware(
    State(_app_state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
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
        } else if token.starts_with("nebu-") {
            println!("🔐 Found Nebulous token: {}", token);
            internal_auth(token, request, next).await
        } else {
            println!("🔐 Found external token: {}", token);
            external_auth(&auth_header, request, next).await
        }
    } else {
        println!("Invalid Authorization header format");
        unauthorized_response()
    }
}

async fn internal_auth(token: &str, request: Request, next: Next) -> Response {
    // TODO: Validate token
    return next.run(request).await;
}

async fn external_auth(auth_header: &String, mut request: Request, next: Next) -> Response {
    let config = crate::config::GlobalConfig::read().unwrap();

    let auth_url = config
        .get_current_server_config()
        .unwrap()
        .auth_server
        .as_ref()
        .unwrap();

    println!("🔐 Making auth request to: {}", auth_url);

    // Validate the token with agentlabs
    let client = reqwest::Client::new();
    let user_profile_result = client
        .get(auth_url)
        .header("Authorization", auth_header)
        .send()
        .await;

    match user_profile_result {
        Ok(response) => {
            if response.status().is_success() {
                // Clone the response so we can read the body twice
                let response_text = response.text().await.unwrap_or_default();
                println!("✅ Auth response: {}", response_text);

                // Parse the user profile from the cloned response
                match serde_json::from_str::<V1UserProfile>(&response_text) {
                    Ok(user_profile) => {
                        request.extensions_mut().insert(user_profile);
                        return next.run(request).await;
                    }
                    Err(e) => {
                        println!("❌ Failed to parse user profile: {}", e);
                        unauthorized_response()
                    }
                }
            } else {
                println!("❌ Auth failed with status: {}", response.status());
                unauthorized_response()
            }
        }
        Err(e) => {
            println!("❌ Auth request failed: {}", e);
            unauthorized_response()
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
