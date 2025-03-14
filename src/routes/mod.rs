use crate::handlers::{
    create_container, delete_container, get_container, health_handler, list_containers,
    root_handler,
};
use crate::middleware::auth_middleware;
use crate::state::AppState;
use axum::{middleware, routing::get, Router};
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

pub fn create_routes(app_state: AppState) -> Router<AppState> {
    // Public routes that do not require authentication
    let public_routes = Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler));

    // Private routes that require authentication
    let private_routes = Router::new()
        // Chat
        .route(
            "/v1/containers",
            get(list_containers).post(create_container),
        )
        .route(
            "/v1/containers/:id",
            get(get_container).delete(delete_container),
        )
        // Apply the authentication middleware to private routes
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ));

    // Combine public and private routes
    public_routes.merge(private_routes).layer(
        TraceLayer::new_for_http()
            .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
            .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
    )
}
