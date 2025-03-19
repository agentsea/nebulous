// src/lib.rs

pub mod accelerator;
pub mod auth;
pub mod cli;
pub mod clusters;
pub mod config;
pub mod container;
pub mod db;
pub mod entities;
pub mod errors;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod mutation;
pub mod org;
pub mod processors;
pub mod query;
pub mod routes;
pub mod ssh;
pub mod state;
pub mod streams;
pub mod validate;
pub mod volumes;

use crate::config::CONFIG;
use axum::Router;
use db::init_db;
use rdkafka::admin::AdminClient;
use rdkafka::producer::FutureProducer;
use rdkafka::ClientConfig;
use routes::create_routes;
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
    let message_queue = match CONFIG.message_queue_type.to_lowercase().as_str() {
        "redis" => {
            // Get the Redis URL from config
            let stripped_url = CONFIG
                .redis_url
                .strip_prefix("redis://")
                .unwrap_or_else(|| CONFIG.redis_url.as_str());

            // Check if REDIS_PASSWORD is set
            let redis_url = match std::env::var("REDIS_PASSWORD") {
                Ok(password) if !password.is_empty() => {
                    // If password is set, include it in the URL
                    format!("redis://:{password}@{stripped_url}")
                }
                _ => {
                    // If password is not set or empty, use URL without password
                    format!("redis://{stripped_url}")
                }
            };

            // Create the Redis client using the constructed URL
            let redis_client = Arc::new(redis::Client::open(redis_url)?);

            MessageQueue::Redis {
                client: redis_client,
            }
        }
        "kafka" => {
            let mut client_config = ClientConfig::new();
            let kafka_config = client_config
                .set("bootstrap.servers", &CONFIG.kafka_bootstrap_servers)
                .set("message.timeout.ms", &CONFIG.kafka_timeout_ms);

            let producer = Arc::new(kafka_config.clone().create::<FutureProducer>()?);
            let admin = Arc::new(kafka_config.create::<AdminClient<_>>()?);

            MessageQueue::Kafka { producer, admin }
        }
        unsupported => {
            return Err(format!("Unsupported message queue type: {}", unsupported).into())
        }
    };

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
