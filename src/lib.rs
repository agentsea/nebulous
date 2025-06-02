// src/lib.rs

pub mod accelerator;
pub mod agent;
pub mod auth;
pub mod cli;
pub mod client;
pub mod config;
pub mod db;
pub mod dns;
pub mod entities;
pub mod errors;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod mutation;
pub mod neblet;
pub mod oci;
pub mod org;
pub mod proxy;
pub mod query;
pub mod resources;
pub mod routes;
pub mod select;
pub mod ssh;
pub mod state;
pub mod streams;
pub mod validate;
pub mod volumes;

pub mod conversion;
pub mod controllers;
pub mod platforms;

use crate::config::SERVER_CONFIG;
use crate::handlers::v1::namespaces::ensure_namespace;
use crate::handlers::v1::volumes::ensure_volume;
use axum::Router;
use db::init_db;
use routes::create_routes;
use sea_orm::DatabaseConnection;
use state::AppState;
use state::MessageQueue;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// Create and return the application state.
pub async fn create_app_state() -> Result<AppState, Box<dyn std::error::Error>> {
    println!("Creating app state");
    let db_pool = init_db().await?;
    println!("Database pool created");

    // Initialize the appropriate message queue based on configuration
    let message_queue = match SERVER_CONFIG.message_queue_type.to_lowercase().as_str() {
        "redis" => {
            let redis_url = &SERVER_CONFIG.redis_url;

            // Create the Redis client using the constructed URL
            let redis_client = Arc::new(redis::Client::open(redis_url.as_str())?);

            MessageQueue::Redis {
                client: redis_client,
            }
        }
        unsupported => {
            return Err(format!("Unsupported message queue type: {}", unsupported).into())
        }
    };

    ensure_base_resources(&db_pool).await?;

    let app_state = AppState {
        db_pool,
        message_queue,
    };

    Ok(app_state)
}

/// Given the `AppState`, create and return the Axum `Router`.
pub async fn create_app(app_state: AppState) -> Router {
    let routes = create_routes(app_state.clone());

    // Define a CORS layer (this example allows any origin, headers, and methods)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = routes
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(app_state);

    app
}

pub async fn ensure_base_resources(
    db_pool: &DatabaseConnection,
) -> Result<(), Box<dyn std::error::Error>> {
    match ensure_namespace(
        db_pool,
        "root",
        &SERVER_CONFIG.root_owner,
        &SERVER_CONFIG.root_owner,
        None,
    )
    .await
    {
        Ok(_) => (),
        Err(e) => return Err(Box::new(e)),
    }

    match ensure_volume(
        db_pool,
        "root",
        "root",
        &SERVER_CONFIG.root_owner,
        format!("s3://{}", &SERVER_CONFIG.bucket_name).as_str(),
        "root",
        None,
    )
    .await
    {
        Ok(_) => (),
        Err(e) => return Err(Box::new(e)),
    }

    Ok(())
}
