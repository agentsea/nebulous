use crate::agent::ns::auth_ns;
use crate::config::SERVER_CONFIG;
use crate::entities::processors;
use crate::middleware::get_user_profile_from_token;
use crate::models::{V1ResourceMetaRequest, V1StreamData, V1StreamMessage, V1UserProfile};
use crate::query::Query;
use crate::resources::v1::processors::base::ProcessorPlatform;
use crate::resources::v1::processors::models::{
    V1Processor, V1ProcessorHealthResponse, V1ProcessorRequest, V1ProcessorScaleRequest,
    V1Processors, V1ReadStreamRequest, V1UpdateProcessor,
};
use crate::resources::v1::processors::standard::StandardProcessor;
use crate::state::AppState;
use crate::utils::namespace::resolve_namespace;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{
    extract::Extension, extract::Json, extract::Path, extract::State, http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use short_uuid::ShortUuid;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

pub async fn create_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Json(processor_request): Json<V1ProcessorRequest>,
) -> Result<Json<V1Processor>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    match crate::validate::validate_name(
        &processor_request.clone().metadata.name.unwrap_or_default(),
    ) {
        Ok(_) => (),
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Invalid name: {}", e) })),
            ));
        }
    }
    debug!("Processor request: {:?}", processor_request);

    let namespace_opt = processor_request.clone().metadata.namespace;

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
        Some(namespace) => namespace,
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
    debug!(">> Using namespace for processor creation: {:?}", namespace);

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

    debug!(
        "Authorizing namespace {:?} with owner_ids {:?}",
        namespace, owner_ids
    );
    let owner = auth_ns(db_pool, &owner_ids, &namespace)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Authorization error: {}", e)})),
            )
        })?;
    debug!("Authorized namespace");

    // Create the standard processor platform
    let app_state = Arc::new(AppState {
        db_pool: db_pool.clone(),
        message_queue: state.message_queue.clone(),
    });
    let platform = StandardProcessor::new(app_state);

    debug!("Declaring processor with namespace: {:?}", namespace);
    let processor = match platform
        .declare(
            &processor_request,
            db_pool,
            &user_profile,
            &owner,
            &namespace,
        )
        .await
    {
        Ok(processor) => processor,
        Err(e) => {
            error!("Error declaring processor: {:?}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            ));
        }
    };

    Ok(Json(processor))
}

pub async fn scale_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
    Json(scale_request): Json<V1ProcessorScaleRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = _scale_processor(
        &state.db_pool,
        &namespace,
        &name,
        &user_profile,
        scale_request,
    )
    .await?;

    Ok(Json(result))
}

// Internal function that performs the actual scaling
async fn _scale_processor(
    db_pool: &DatabaseConnection,
    namespace: &str,
    name: &str,
    user_profile: &V1UserProfile,
    scale_request: V1ProcessorScaleRequest,
) -> Result<V1Processor, (StatusCode, Json<serde_json::Value>)> {
    // Validate we have at least one parameter
    if scale_request.replicas.is_none() && scale_request.min_replicas.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "At least one of 'replicas' or 'min_replicas' must be provided"})),
        ));
    }

    // Collect owner IDs
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the processor
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        namespace,
        name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    let mut active_model = processors::ActiveModel::from(processor);

    // Handle min_replicas update if provided
    if let Some(min_replicas) = scale_request.min_replicas {
        if min_replicas <= 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "min_replicas must be a positive integer"})),
            ));
        }
        debug!("Setting min_replicas to {}", min_replicas);
        active_model.min_replicas = ActiveValue::Set(Some(min_replicas));
    }

    // Handle desired_replicas update if provided or if min_replicas requires an update
    match scale_request.replicas {
        // If replicas is explicitly set
        Some(replicas) => {
            if replicas <= 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "replicas must be a positive integer"})),
                ));
            }
            debug!("Setting desired_replicas to {}", replicas);
            active_model.desired_replicas = ActiveValue::Set(Some(replicas));
        }
        // If only min_replicas is provided, ensure desired_replicas is at least that amount
        None => {
            if let Some(min_replicas) = scale_request.min_replicas {
                let current_desired = match &active_model.desired_replicas {
                    ActiveValue::Set(val) => val.clone(),
                    ActiveValue::Unchanged(val) => val.clone(),
                    _ => None,
                };

                // If current desired_replicas is less than the new min_replicas or not set
                if current_desired.is_none() || current_desired.unwrap_or(0) < min_replicas {
                    debug!(
                        "Setting desired_replicas to match min_replicas: {}",
                        min_replicas
                    );
                    active_model.desired_replicas = ActiveValue::Set(Some(min_replicas));
                }
            }
        }
    }

    // Update the processor in the database
    let updated_processor = active_model.update(db_pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to update processor: {}", e)})),
        )
    })?;

    // Convert the updated processor model to V1Processor for the response
    let processor_v1 = updated_processor.to_v1_processor().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to convert processor: {}", e)})),
        )
    })?;

    Ok(processor_v1)
}

#[axum::debug_handler]
pub async fn check_processor_health(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1ProcessorHealthResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Changed return type
    debug!(
        "Entering check_processor_health for processor: {} in namespace: {}, user_profile: {:?}",
        name, namespace, user_profile
    );
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);
    debug!(
        "Resolved namespace: {} to {}",
        namespace, resolved_namespace
    );

    // --- Authorization and Processor Fetching ---
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        debug!("User organizations found: {:?}", orgs.keys());
        orgs.keys().cloned().collect()
    } else {
        debug!("No organizations found for user.");
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    debug!("Collected owner_ids: {:?}", owner_ids);
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    debug!(
        "Attempting to find processor with namespace: {}, name: {}, owner_ids: {:?}",
        resolved_namespace, name, owner_id_refs
    );
    let processor = Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    .map_err(|e| {
        error!(
            "Database error finding processor {}:{}: {}",
            resolved_namespace, name, e
        );
        (
            StatusCode::NOT_FOUND, // Changed to NOT_FOUND for clarity
            Json(json!({"error": format!("Processor not found or access denied: {}", e)})),
        )
    })?;
    debug!("Successfully found processor: {:?}", processor);
    // --- End Authorization ---

    // Construct health stream name
    let health_stream_name = format!("{}.health", processor.stream);
    let message_id = ShortUuid::generate().to_string();
    let return_stream_name = format!("{}.return.{}", health_stream_name, message_id);

    debug!(
        "Health stream name: {}, Message ID: {}, Return stream name: {}",
        health_stream_name, message_id, return_stream_name
    );

    let health_check_content = json!({
        "type": "HEALTH_CHECK_REQUEST",
        "request_id": message_id.clone(),
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    debug!("Health check content: {:?}", health_check_content);

    debug!(
        "Attempting to get user profile from token for health check message. Token: {:?}",
        user_profile.token
    );
    let user_prof = match get_user_profile_from_token(
        &state.db_pool,
        &user_profile.token.clone().unwrap_or_default(),
    )
    .await
    {
        Ok(user_prof) => {
            debug!(
                "Successfully retrieved user profile for health check: {:?}",
                user_prof
            );
            user_prof
        }
        Err(e) => {
            error!("Failed to get user profile for health check: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"error": format!("Failed to get user profile for health check: {}", e)}),
                ),
            ));
        }
    };

    let message = V1StreamMessage {
        kind: "HealthCheckRequest".to_string(),
        id: message_id.clone(),
        content: health_check_content,
        created_at: chrono::Utc::now().timestamp(),
        return_stream: Some(return_stream_name.clone()),
        user_id: Some(user_prof.email.clone()),
        orgs: user_prof.organizations.clone().map(|orgs| json!(orgs)),
        handle: user_prof.handle.clone(),
        adapter: Some(format!("processor-health:{}", processor.id)),
        api_key: None, // Removed agent key
    };
    debug!(
        "Constructed V1StreamMessage for health check: {:?}",
        message
    );

    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            debug!("Using Redis message queue for health check.");
            let mut conn = client.get_connection().map_err(|e| {
                error!("Redis connection error for health check: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        json!({"error": format!("Redis connection error for health check: {}", e)}),
                    ),
                )
            })?;
            debug!("Successfully obtained Redis connection for health check.");

            let message_json = serde_json::to_string(&message).map_err(|e| {
                error!("Failed to serialize health check message: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to serialize health check message: {}", e)})),
                )
            })?;
            debug!("Serialized health check message to JSON: {}", message_json);

            debug!(
                "Attempting to XADD health check message to stream: {}",
                health_stream_name
            );
            let _stream_id: String = redis::cmd("XADD")
                .arg(&health_stream_name)
                .arg("*")
                .arg("data")
                .arg(&message_json)
                .query(&mut conn)
                .map_err(|e| {
                    error!(
                        "Failed to send health check message to stream '{}': {}",
                        health_stream_name, e
                    );
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to send health check to stream: {}", e)})),
                    )
                })?;
            debug!(
                "Successfully sent health check message to stream: {}, Stream ID: {}",
                health_stream_name, _stream_id
            );

            // Initialize return stream
            debug!(
                "Attempting to XADD init message to return stream: {}",
                return_stream_name
            );
            let init_message_id: String = match redis::cmd("XADD")
                .arg(&return_stream_name)
                .arg("*")
                .arg("init")
                .arg("true")
                .query(&mut conn)
            {
                Ok(id) => {
                    debug!(
                        "Successfully added init message to return stream: {}, Init Message ID: {}",
                        return_stream_name, id
                    );
                    id
                }
                Err(e) => {
                    error!(
                        "Failed to add init message to return stream '{}': {}",
                        return_stream_name, e
                    );
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({"error": format!("Failed to initialize health return stream: {}", e)}),
                        ),
                    ));
                }
            };

            const HEALTH_CHECK_TIMEOUT_MS: u64 = 30000; // 30 seconds timeout for health check
            debug!("Health check timeout set to: {}ms", HEALTH_CHECK_TIMEOUT_MS);

            let client_clone = client.clone();
            let return_stream_name_clone = return_stream_name.clone();

            debug!(
                "Spawning blocking task to read from return stream: {} after init message ID: {}",
                return_stream_name_clone, init_message_id
            );
            let read_result = tokio::task::spawn_blocking(move || {
                debug!(
                    "[spawn_blocking] Attempting to get Redis connection for health check read."
                );
                let mut conn_blocking = client_clone.get_connection().map_err(|e| {
                    error!(
                        "[spawn_blocking] Failed to get Redis connection: {}",
                        e.to_string()
                    );
                    redis::RedisError::from((
                        redis::ErrorKind::IoError,
                        "Failed to get connection in spawn_blocking for health check",
                        e.to_string(),
                    ))
                })?;
                debug!(
                    "[spawn_blocking] Successfully obtained Redis connection. Reading stream: {} with timeout: {}",
                    return_stream_name_clone, HEALTH_CHECK_TIMEOUT_MS
                );
                redis::cmd("XREAD")
                    .arg("BLOCK")
                    .arg(HEALTH_CHECK_TIMEOUT_MS)
                    .arg("STREAMS")
                    .arg(&return_stream_name_clone)
                    .arg(&init_message_id) // Read after the init message
                    .query::<redis::streams::StreamReadReply>(&mut conn_blocking)
            })
            .await;
            debug!("Spawn_blocking task for health check read completed.");

            // Handle result from spawn_blocking
            let response_data = match read_result {
                Ok(Ok(reply)) => {
                    debug!("Received reply from XREAD: {:?}", reply);
                    if reply.keys.is_empty() {
                        warn!(
                            "Timed out waiting for processor health response from stream: {}",
                            return_stream_name
                        );
                        Err((
                            StatusCode::REQUEST_TIMEOUT,
                            Json(
                                json!({"error": "Timed out waiting for processor health response"}),
                            ),
                        ))
                    } else {
                        debug!(
                            "Processing {} keys from health response stream: {}",
                            reply.keys.len(),
                            return_stream_name
                        );
                        // Process the first valid message found
                        let mut processed_response = None;
                        for key_entry in reply.keys {
                            debug!("Processing key_entry: {:?}", key_entry.key);
                            for id_entry in key_entry.ids {
                                debug!(
                                    "Processing id_entry: {:?}, map: {:?}",
                                    id_entry.id, id_entry.map
                                );
                                if id_entry.map.contains_key("init") {
                                    debug!("Skipping init message with ID: {}", id_entry.id);
                                    continue;
                                }
                                if let Some(data_val) = id_entry.map.get("data") {
                                    debug!("Found 'data' field in health response: {:?}", data_val);
                                    let data_str = match data_val {
                                        redis::Value::BulkString(bytes) => {
                                            String::from_utf8_lossy(bytes).to_string()
                                        }
                                        redis::Value::SimpleString(s) => s.clone(),
                                        _ => {
                                            warn!("Unexpected type for 'data' field in health response: {:?}", data_val);
                                            continue;
                                        }
                                    };
                                    debug!("Health response data string: '{}'", data_str);
                                    match serde_json::from_str::<V1ProcessorHealthResponse>(
                                        &data_str,
                                    ) {
                                        // Deserialize into V1ProcessorHealthResponse
                                        Ok(json_data) => {
                                            debug!(
                                                "Successfully parsed health response: {:?}",
                                                json_data
                                            );
                                            processed_response = Some(json_data);
                                            break; // Found a valid message
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to parse health response data as V1ProcessorHealthResponse: {}. Raw: '{}'",
                                                e, data_str
                                            );
                                            // If parsing fails, return a generic error or a default health response
                                            processed_response = Some(V1ProcessorHealthResponse {
                                                status: "error".to_string(),
                                                message: Some(format!(
                                                    "Failed to parse health response: {}",
                                                    e
                                                )),
                                                details: Some(json!({ "raw_response": data_str })),
                                            });
                                            break;
                                        }
                                    }
                                } else {
                                    debug!("'data' field not found in message: {:?}", id_entry.id);
                                }
                            }
                            if processed_response.is_some() {
                                break;
                            }
                        }
                        match processed_response {
                            Some(data) => {
                                debug!("Processed health response: {:?}", data);
                                Ok(data)
                            }
                            None => {
                                error!(
                                    "Received health response without data or only init message from stream: {}",
                                    return_stream_name
                                );
                                Err((
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    Json(
                                        json!({"error": "Received health response without data or only init message"}),
                                    ),
                                ))
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    error!(
                        "Redis error during health check XREAD on stream '{}': {}",
                        return_stream_name, e
                    );
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({"error": format!("Error reading health response stream: {}", e)}),
                        ),
                    ))
                }
                Err(e) => {
                    error!(
                        "Spawn_blocking task failed for health check on stream '{}': {}",
                        return_stream_name, e
                    );
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Health check task execution error: {}", e)})),
                    ))
                }
            };

            // Cleanup the return stream
            debug!(
                "Attempting to get Redis connection for cleanup of stream: {}",
                return_stream_name
            );
            let mut conn_cleanup = client.get_connection().unwrap(); // Assume connection is fine after read
            debug!("Successfully obtained Redis connection for cleanup.");
            debug!("Attempting to DEL return stream: {}", return_stream_name);
            let del_result: Result<(), redis::RedisError> = redis::cmd("DEL")
                .arg(&return_stream_name)
                .query(&mut conn_cleanup);
            if let Err(e) = del_result {
                warn!(
                    "Failed to delete health check return stream '{}': {}",
                    return_stream_name, e
                );
            } else {
                debug!(
                    "Successfully deleted health check return stream: {}",
                    return_stream_name
                );
            }

            // Return the processed response or error
            match response_data {
                Ok(data) => {
                    debug!("Returning successful health check response: {:?}", data);
                    Ok(Json(data))
                }
                Err(err_tuple) => {
                    error!(
                        "Returning error for health check: Status {:?}, Error: {:?}",
                        err_tuple.0, err_tuple.1
                    );
                    Err(err_tuple)
                }
            }
        }
        crate::state::MessageQueue::Kafka { .. } => {
            error!("Kafka not supported for processor health checks.");
            Err((
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "Kafka streams are not currently supported for health checks"}),
                ),
            ))
        }
    }
}

#[axum::debug_handler]
pub async fn list_processors(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
) -> Result<Json<V1Processors>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };

    // Include user's email (assuming owner_id is user's email)
    owner_ids.push(user_profile.email.clone());

    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Query processors for all owner_ids
    let processor_models = Query::find_processors_by_owners(db_pool, &owner_id_refs)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            )
        })?;

    // Convert database models to API response models
    let processors_result: Result<Vec<V1Processor>, _> = processor_models
        .into_iter()
        .map(|p| p.to_v1_processor())
        .collect();

    let processors = processors_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to convert processors: {}", e)})),
        )
    })?;

    Ok(Json(V1Processors { processors }))
}

pub async fn get_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<V1Processor>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    let processor_v1 = processor.to_v1_processor().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to convert processor: {}", e)})),
        )
    })?;

    Ok(Json(processor_v1))
}

/// Send a message to a processor
///
/// # Request Parameters
/// - `stream_data.content`: The message content to send to the processor
/// - `stream_data.wait`: If true, wait for a response (default: false)
/// - `stream_data.stream`: If true, indicates the processor is a generator that streams responses (default: false)
/// - `stream_data.user_key`: Optional user authentication key
///
/// # Streaming Support
/// When `stream=true` and `wait=true`, the API will suggest using the WebSocket endpoint
/// for real-time streaming. This is designed for processor generators that:
///
/// 1. Return multiple incremental response chunks ("StreamChunkMessage" with "status": "stream")
/// 2. Signal completion with "StreamResponseMessage" having "status": "success" and "content": null
/// 3. Benefit from real-time message delivery via WebSocket
///
/// # Response Modes
/// - `wait=false`: Returns immediately with success confirmation and return stream info
/// - `wait=true, stream=false`: Waits for single response and returns it
/// - `wait=true, stream=true`: Suggests WebSocket endpoint with connection info
///
/// # WebSocket URL Format
/// For streaming responses: `/v1/processors/{namespace}/{name}/return/{message_id}/stream`
pub async fn send_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
    Json(stream_data): Json<V1StreamData>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    debug!(
        "Sending processor with namespace: {} and name: {}",
        namespace, name
    );

    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);
    debug!("Resolved namespace: {}", resolved_namespace);

    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();
    debug!("Owner IDs: {:?}", owner_ids);

    // Find the processor
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    debug!("Processor: {:?}", processor);

    // --- Generate a temporary agent key for this operation --- //
    let user_token = stream_data
        .user_key
        .clone()
        .unwrap_or_else(|| user_profile.token.clone().unwrap_or_default());

    if user_token.is_empty() {
        error!("User token is missing, cannot generate agent key.");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Authentication token missing"})),
        ));
    }
    debug!("User token: {}", user_token);

    let auth_server = SERVER_CONFIG.auth.url.clone();
    if auth_server.is_empty() {
        error!("Auth server URL is empty.");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Auth server configuration is empty"})),
        ));
    }

    // --- Conditionally Generate Agent Key ---
    let agent_key: String;
    if user_token.starts_with("a.") || user_token.starts_with("k.") {
        debug!("Using existing user_token as agent key: {}", user_token);
        agent_key = user_token.clone();
    } else {
        debug!(
            "Creating agent key request for processor: {} and auth server: {}",
            processor.id, auth_server
        );
        let agent_key_request = crate::models::V1CreateAgentKeyRequest {
            agent_id: format!("processor-{}", processor.id),
            name: format!(
                "send-processor-{}-{}",
                processor.id,
                ShortUuid::generate().to_string()
            ),
            duration: 86400, // e.g., 24 hour validity
        };
        debug!("Creating agent key request: {:?}", agent_key_request);

        let agent_key_response = crate::agent::agent::create_agent_key(
            &auth_server,
            &user_token,
            agent_key_request,
        )
        .await
        .map_err(|e| {
            error!("Failed to create agent key: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to generate temporary agent key: {}", e)})),
            )
        })?;

        debug!("Agent key response: {:?}", agent_key_response);
        agent_key = agent_key_response.key.ok_or_else(|| {
            error!("Generated agent key response did not contain a key.");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to obtain temporary agent key value"})),
            )
        })?;
    }
    // --- End Agent Key Generation ---

    // Get the stream name
    let stream_name = processor.stream;
    let id = ShortUuid::generate().to_string();

    // Always generate the actual return stream name
    let actual_return_stream_name = format!("{}.return.{}", stream_name, id.clone());

    // Determine if the handler should wait based on the request's 'wait' field.
    // Default to false (not waiting) if 'wait' is not specified.
    let should_wait_for_response = stream_data.wait.unwrap_or(false);

    // Check if this is a streaming request
    let is_streaming_request = stream_data.stream.unwrap_or(false);

    debug!(
        "Message ID: {}, Target Stream: {}, Actual Return Stream: {}, Should Wait: {}, Is Streaming: {}",
        id, stream_name, actual_return_stream_name, should_wait_for_response, is_streaming_request
    );

    let user_prof = match get_user_profile_from_token(&state.db_pool, &user_token).await {
        Ok(user_prof) => user_prof,
        Err(e) => {
            error!("Failed to get user profile: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to get user profile: {}", e)})),
            ));
        }
    }; // TODO: make more efficient
    debug!("Sending message with user profile: {:?}", user_prof);

    // Create a stream message
    let message = V1StreamMessage {
        kind: "StreamMessage".to_string(),
        id: id.clone(),
        content: stream_data.content,
        created_at: chrono::Utc::now().timestamp(),
        return_stream: Some(actual_return_stream_name.clone()), // Always provide the return stream to the message
        user_id: Some(user_prof.email.clone()),
        orgs: user_prof.organizations.clone().map(|orgs| json!(orgs)),
        handle: user_prof.handle.clone(),
        adapter: Some(format!("processor:{}", processor.id)),
        api_key: Some(agent_key),
    };

    // Access the Redis client from the message queue
    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            // Get a Redis connection
            let mut conn = match client.get_connection() {
                Ok(conn) => {
                    debug!("Successfully obtained Redis connection.");
                    conn
                }
                Err(e) => {
                    error!("Redis connection error: {}", e);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Redis connection error: {}", e)})),
                    ));
                }
            };

            // Serialize the message to JSON
            let message_json = serde_json::to_string(&message).map_err(|e| {
                error!("Failed to serialize message: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to serialize message: {}", e)})),
                )
            })?;
            debug!("Message serialized successfully: {}", message_json);

            // Add the message to the stream using higher-level xadd
            let stream_id_result: Result<String, redis::RedisError> = redis::cmd("XADD")
                .arg(stream_name.clone())
                .arg("*") // Auto-generate ID
                .arg("data")
                .arg(&message_json)
                .query(&mut conn);

            let stream_id = match stream_id_result {
                Ok(id) => {
                    debug!("Message added to stream '{}' with ID: {}", stream_name, id);
                    id
                }
                Err(e) => {
                    error!("Failed to send message to stream '{}': {}", stream_name, e);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to send message to stream: {}", e)})),
                    ));
                }
            };

            // If client requested streaming, suggest using the websocket endpoint instead
            if is_streaming_request && should_wait_for_response {
                debug!("Client requested streaming with wait=true, suggesting websocket endpoint");
                return Ok(Json(json!({
                    "success": true,
                    "stream_id": stream_id,
                    "message_id": id,
                    "return_stream": actual_return_stream_name,
                    "streaming": true,
                    "websocket_url": format!("/v1/processors/{}/{}/return/{}/stream", namespace, name, id),
                    "note": "For streaming responses, consider using the WebSocket endpoint for real-time message delivery"
                })).into_response());
            }

            // If the client requested to wait for a response on the actual_return_stream_name
            if should_wait_for_response {
                tracing::debug!(
                    "Waiting for response on return stream: {}",
                    actual_return_stream_name
                );

                // Create the return stream with a dummy message to ensure it exists, and capture its ID
                let init_message_id: String = match redis::cmd("XADD")
                    .arg(&actual_return_stream_name) // Use actual_return_stream_name
                    .arg("*")
                    .arg("init")
                    .arg("true")
                    .query(&mut conn)
                {
                    Ok(id) => {
                        debug!(
                            "Added init message to return stream '{}' with ID: {}",
                            actual_return_stream_name, id
                        );
                        id // This value will be assigned to init_message_id
                    }
                    Err(e) => {
                        error!(
                            "Failed to add init message to return stream '{}': {}. Cannot proceed.",
                            actual_return_stream_name, e
                        );
                        // If we can't even add the init message, waiting is unlikely to work
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(
                                json!({"error": format!("Failed to initialize return stream: {}", e)}),
                            ),
                        ));
                    }
                };

                // Wait for response with a timeout (1 hour)
                const TIMEOUT_MS: u64 = 3600000;

                // --- Prepare for spawn_blocking ---
                let actual_return_stream_name_clone = actual_return_stream_name.clone();
                let client_clone = client.clone(); // Clone the client Arc for the blocking task
                                                   // --- Move blocking call to spawn_blocking ---
                let read_result = tokio::task::spawn_blocking(move || {
                    // Get a new connection from the pool inside the blocking task
                    let mut conn = client_clone.get_connection().map_err(|e| {
                        redis::RedisError::from((
                            redis::ErrorKind::IoError,
                            "Failed to get connection in spawn_blocking",
                            e.to_string(),
                        ))
                    })?;

                    debug!(
                        "Attempting blocking XREAD on stream '{}' with timeout {}ms",
                        actual_return_stream_name_clone, TIMEOUT_MS
                    );

                    redis::cmd("XREAD")
                        .arg("BLOCK")
                        .arg(TIMEOUT_MS)
                        .arg("STREAMS")
                        .arg(&actual_return_stream_name_clone) // Use the clone
                        .arg(&init_message_id)
                        .query::<redis::streams::StreamReadReply>(&mut conn)
                })
                .await;
                // --- End spawn_blocking ---

                // Handle the result from spawn_blocking (which itself returns a Result)
                let result = match read_result {
                    Ok(Ok(reply)) => {
                        // Outer Ok is from spawn_blocking, inner Ok is from redis::cmd
                        debug!("XREAD successful. Raw reply: {:?}", reply);
                        reply
                    }
                    Ok(Err(e)) => {
                        // Outer Ok, inner Err (Redis error)
                        error!(
                            "Error reading from response stream '{}' inside spawn_blocking: {}",
                            actual_return_stream_name, // Use original name for logging
                            e
                        );
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(
                                json!({"error": format!("Error reading from response stream: {}", e)}),
                            ),
                        ));
                    }
                    Err(e) => {
                        // Outer Err (spawn_blocking join error)
                        error!(
                            "Spawn_blocking task failed for stream '{}': {}",
                            actual_return_stream_name, // Use original name for logging
                            e
                        );
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": format!("Task execution error: {}", e)})),
                        ));
                    }
                };

                // Clean up the return stream - Requires getting a connection again
                let mut conn = match client.get_connection() {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to get connection for DEL command: {}", e);
                        // Log and continue with processing if response was received.
                        // If DEL must succeed, return an error here.
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(
                                json!({"error": format!("Failed get Redis conn for cleanup: {}", e)}),
                            ),
                        ));
                    }
                };
                debug!(
                    "Attempting to delete return stream '{}'",
                    actual_return_stream_name // Use actual_return_stream_name
                );
                let del_result: Result<(), redis::RedisError> = redis::cmd("DEL")
                    .arg(&actual_return_stream_name) // Use actual_return_stream_name
                    .query(&mut conn);
                if let Err(e) = del_result {
                    // Log error but continue processing the response if we got one
                    error!(
                        "Failed to delete return stream '{}': {}. Processing response anyway.",
                        actual_return_stream_name, // Use actual_return_stream_name
                        e
                    );
                } else {
                    debug!(
                        "Successfully deleted return stream '{}'",
                        actual_return_stream_name // Use actual_return_stream_name
                    );
                }

                // Check if we got a response
                if result.keys.is_empty() {
                    error!(
                        "Timed out or received empty response from return stream '{}'",
                        actual_return_stream_name // Use actual_return_stream_name
                    );
                    return Err((
                        StatusCode::REQUEST_TIMEOUT,
                        Json(json!({"error": "Timed out waiting for processor response"})),
                    ));
                }
                debug!(
                    "Received {} keys in response from stream '{}'",
                    result.keys.len(),
                    actual_return_stream_name // Use actual_return_stream_name
                );

                // Process the response
                for key in result.keys {
                    debug!("Processing key (stream): {:?}", key.key);
                    for id in key.ids {
                        debug!("Processing message ID: {:?}, Map: {:?}", id.id, id.map);
                        if let Some(data_value) = id.map.get("data") {
                            debug!("Found 'data' field: {:?}", data_value);
                            // Convert the Redis value to a string
                            let data_str = match data_value {
                                redis::Value::BulkString(bytes) => {
                                    let s = String::from_utf8_lossy(bytes).to_string();
                                    debug!("Converted BulkString to string: '{}'", s);
                                    String::from_utf8_lossy(bytes).to_string()
                                }
                                redis::Value::SimpleString(s) => s.clone(),
                                _ => format!("{:?}", data_value),
                            };
                            debug!("Final data_str: '{}'", data_str);

                            // Try to parse the data as JSON
                            match serde_json::from_str::<serde_json::Value>(&data_str) {
                                Ok(json_data) => {
                                    debug!("Successfully parsed data as JSON: {:?}", json_data);
                                    return Ok(Json(json_data).into_response());
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to parse response data as JSON: {}. Returning raw string.",
                                        e
                                    );
                                    return Ok(Json(json!({"raw": data_str})).into_response());
                                }
                            }
                        } else {
                            debug!("'data' field not found in message map for ID: {:?}", id.id);
                        }
                    }
                }

                // If we couldn't find data in the response
                error!(
                    "Processed all messages in response stream '{}', but none contained a 'data' field.",
                    actual_return_stream_name // Use actual_return_stream_name
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Received response without data field"})),
                ));
            } else {
                // If not waiting, just return success
                debug!(
                    "Not waiting for response. Returning success for message ID {}",
                    message.id
                );
                Ok(Json(json!({
                    "success": true,
                    "stream_id": stream_id,
                    "message_id": message.id,
                    "return_stream": actual_return_stream_name, // Always include the name of the return stream
                }))
                .into_response())
            }
        }
        crate::state::MessageQueue::Kafka { .. } => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Kafka streams are not currently supported"})),
        )),
    }
}

pub async fn delete_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    debug!("Deleting processor: {} in namespace: {}", name, namespace);
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    debug!(
        "Finding processor: {} in namespace: {}",
        name, resolved_namespace
    );
    let processor = Query::find_processor_by_namespace_name_and_owners(
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

    debug!("Deleting processor: {}", processor.id);
    let app_state = Arc::new(AppState {
        db_pool: db_pool.clone(),
        message_queue: state.message_queue.clone(),
    });
    let platform = StandardProcessor::new(app_state);

    let redis = match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => client,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Kafka streams are not currently supported"})),
            ))
        }
    };

    platform
        .delete(&processor.id, db_pool, redis)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to delete processor: {}", e)})),
            )
        })?;

    debug!("Deleted processor: {}", processor.id);

    Ok(StatusCode::OK)
}

pub async fn update_processor(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
    Json(update_request): Json<V1UpdateProcessor>,
) -> Result<Json<V1Processor>, (StatusCode, Json<serde_json::Value>)> {
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the processor
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    let no_delete = update_request.no_delete.unwrap_or(false);

    // Convert processor model to V1Processor for comparison and potential return value
    let processor_v1 = processor.to_v1_processor().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to convert processor: {}", e)})),
        )
    })?;

    // --- Start: Determine if recreation is required ---
    let mut requires_recreation = false;

    // Check stream (Assuming processor_v1 has stream: String)
    // Note: processor_v1 doesn't directly expose stream, it's part of the DB model 'processor'
    if let Some(update_stream) = &update_request.stream {
        if *update_stream != processor.stream {
            // Compare with the original db model field
            requires_recreation = true;
            debug!(
                "Stream changed ('{}' vs '{}'), requires recreation",
                update_stream, processor.stream
            );
        }
    }

    // Check schema
    if !requires_recreation
        && update_request.schema.is_some()
        && update_request.schema != processor_v1.schema
    {
        debug!("Schema changed, does not require recreation");
    }

    // Check common_schema
    if !requires_recreation
        && update_request.common_schema.is_some()
        && update_request.common_schema != processor_v1.common_schema
    {
        debug!("Common schema changed, does not require recreation");
    }

    // Check scale
    if !requires_recreation
        && update_request.scale.is_some()
        && update_request.scale != processor_v1.scale
    {
        debug!("Scale changed, does not require recreation");
    }

    // Check max_replicas
    if !requires_recreation
        && update_request.max_replicas.is_some()
        && update_request.max_replicas != processor_v1.max_replicas
    {
        debug!("Max replicas changed, does not require recreation");
    }

    // Check container (ignoring status)
    if !requires_recreation {
        match (&update_request.container, &processor_v1.container) {
            (Some(update_req), Some(existing_container)) => {
                let mut container_changed = false;

                // Explicitly compare fields relevant to recreation
                if update_req.platform.as_deref().unwrap_or_default()
                    != existing_container.platform.as_deref().unwrap_or_default()
                {
                    container_changed = true;
                    debug!(
                        "Container platform changed. Old: {:?}, New: {:?}",
                        existing_container.platform.as_deref().unwrap_or_default(),
                        update_req.platform.as_deref().unwrap_or_default()
                    );
                }
                if update_req.image != existing_container.image {
                    container_changed = true;
                    debug!(
                        "Container image changed. Old: {:?}, New: {:?}",
                        existing_container.image, update_req.image
                    );
                }
                // Compare effective env vars (request is Option<Vec>, existing is Vec)
                if update_req.env.as_deref().unwrap_or_default()
                    != existing_container.env.as_deref().unwrap_or_default()
                {
                    container_changed = true;
                    debug!(
                        "Container env changed. Old: {:?}, New: {:?}",
                        existing_container.env.as_deref().unwrap_or_default(),
                        update_req.env.as_deref().unwrap_or_default()
                    );
                }
                if update_req.command != existing_container.command {
                    container_changed = true;
                    debug!(
                        "Container command changed. Old: {:?}, New: {:?}",
                        existing_container.command, update_req.command
                    );
                }
                if update_req.args != existing_container.args {
                    container_changed = true;
                    debug!(
                        "Container args changed. Old: {:?}, New: {:?}",
                        existing_container.args, update_req.args
                    );
                }
                if update_req.volumes != existing_container.volumes {
                    container_changed = true;
                    debug!(
                        "Container volumes changed. Old: {:?}, New: {:?}",
                        existing_container.volumes, update_req.volumes
                    );
                }
                if update_req.accelerators != existing_container.accelerators {
                    container_changed = true;
                    debug!(
                        "Container accelerators changed. Old: {:?}, New: {:?}",
                        existing_container.accelerators, update_req.accelerators
                    );
                }
                if update_req.resources != existing_container.resources {
                    container_changed = true;
                    debug!(
                        "Container resources changed. Old: {:?}, New: {:?}",
                        existing_container.resources, update_req.resources
                    );
                }
                if update_req.meters != existing_container.meters {
                    container_changed = true;
                    debug!(
                        "Container meters changed. Old: {:?}, New: {:?}",
                        existing_container.meters, update_req.meters
                    );
                }
                if update_req.restart != existing_container.restart {
                    container_changed = true;
                    debug!(
                        "Container restart policy changed. Old: {:?}, New: {:?}",
                        existing_container.restart, update_req.restart
                    );
                }
                if update_req.queue != existing_container.queue {
                    container_changed = true;
                    debug!(
                        "Container queue changed. Old: {:?}, New: {:?}",
                        existing_container.queue, update_req.queue
                    );
                }
                if update_req.timeout != existing_container.timeout {
                    container_changed = true;
                    debug!(
                        "Container timeout changed. Old: {:?}, New: {:?}",
                        existing_container.timeout, update_req.timeout
                    );
                }
                if update_req.proxy_port != existing_container.proxy_port {
                    container_changed = true;
                    debug!(
                        "Container proxy_port changed. Old: {:?}, New: {:?}",
                        existing_container.proxy_port, update_req.proxy_port
                    );
                }
                if update_req.health_check != existing_container.health_check {
                    container_changed = true;
                    debug!(
                        "Container health_check changed. Old: {:?}, New: {:?}",
                        existing_container.health_check, update_req.health_check
                    );
                }
                if update_req.authz != existing_container.authz {
                    container_changed = true;
                    debug!(
                        "Container authz changed. Old: {:?}, New: {:?}",
                        existing_container.authz, update_req.authz
                    );
                }
                if update_req.ssh_keys != existing_container.ssh_keys {
                    container_changed = true;
                    debug!(
                        "Container ssh_keys changed. Old: {:?}, New: {:?}",
                        existing_container.ssh_keys, update_req.ssh_keys
                    );
                }
                // Assuming update_req.ports exists and is comparable to existing_container.ports
                if update_req.ports != existing_container.ports {
                    container_changed = true;
                    debug!(
                        "Container ports changed. Old: {:?}, New: {:?}",
                        existing_container.ports, update_req.ports
                    );
                }

                if container_changed {
                    requires_recreation = true;
                    debug!("Container config changed, requires recreation");
                } else {
                    debug!("Container config unchanged, no recreation needed based on container.");
                }
            }
            (Some(_), None) => {
                // Adding a container where none existed
                requires_recreation = true;
                debug!(
                    "Container added (was None), requires recreation. New: {:?}",
                    update_request.container
                );
            }
            (None, Some(_)) => {
                // Container exists but update request doesn't specify one.
                // Current logic treats this as no-change for the container config.
                debug!("Container exists but not specified in update. No change triggered for container.");
            }
            (None, None) => {
                // No container before or after
                debug!("No container specified in update or existing. No change for container.");
            }
        }
    }
    // --- End: Determine if recreation is required ---

    // If changes require recreation
    if requires_recreation {
        debug!("Processor configuration changed, recreation required.");
        if no_delete {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Processor changes require deletion, but no_delete=true"
                })),
            ));
        }

        debug!("Deleting old processor");
        let app_state = Arc::new(AppState {
            db_pool: db_pool.clone(),
            message_queue: state.message_queue.clone(),
        });
        let platform = StandardProcessor::new(app_state);

        let redis = match &state.message_queue {
            crate::state::MessageQueue::Redis { client } => client,
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "Kafka streams are not currently supported"})),
                ))
            }
        };

        platform
            .delete(&processor.id, db_pool, redis)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to delete processor: {}", e)})),
                )
            })?;

        // --- Start: Create the potential final processor state by merging updates ---
        // This is needed for the declare call if recreation happens.
        let merged_processor_request = V1ProcessorRequest {
            kind: update_request
                .kind
                .clone()
                .unwrap_or_else(|| processor_v1.kind.clone()), // Use existing if not provided
            metadata: V1ResourceMetaRequest {
                name: Some(processor.name.clone()), // Name doesn't change on update
                namespace: Some(processor.namespace.clone()), // Namespace doesn't change on update
                labels: update_request
                    .metadata
                    .as_ref()
                    .and_then(|m| m.labels.clone())
                    .or_else(|| processor_v1.metadata.labels.clone()), // processor_v1.metadata is V1ResourceMeta
                owner: None,     // Usually set during creation/retrieval, not update
                owner_ref: None, // Usually set during creation/retrieval, not update
            },
            container: update_request
                .container
                .clone()
                .or(processor_v1.container.clone()), // Merge container
            schema: update_request
                .schema
                .clone()
                .or(processor_v1.schema.clone()), // Merge schema
            common_schema: update_request
                .common_schema
                .clone()
                .or(processor_v1.common_schema.clone()), // Merge common schema
            min_replicas: update_request.min_replicas.or(processor_v1.min_replicas), // Merge min_replicas
            max_replicas: update_request.max_replicas.or(processor_v1.max_replicas), // Merge max_replicas
            scale: update_request.scale.clone().or(processor_v1.scale.clone()),      // Merge scale
        };
        // --- End: Create the potential final processor state ---

        // Create the new processor with merged values
        debug!("Creating new processor with updated fields");
        let app_state = Arc::new(AppState {
            db_pool: db_pool.clone(),
            message_queue: state.message_queue.clone(),
        });
        let platform = StandardProcessor::new(app_state);

        let created = platform
            .declare(
                &merged_processor_request, // Use the merged request
                db_pool,
                &user_profile,
                &user_profile.email,
                &resolved_namespace,
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
        debug!("Created new processor: {:?}", created);

        return Ok(Json(created));
    } else {
        debug!("No changes requiring processor recreation detected. Checking for other updatable fields.");
        // --- Start: Handle updates if no recreation needed ---
        let mut processor_active_model = processors::ActiveModel::from(processor.clone()); // Use clone as processor is used later
        let mut model_updated = false;

        // Check metadata labels
        if let Some(metadata_req) = &update_request.metadata {
            if let Some(labels) = &metadata_req.labels {
                let current_labels_json = processor_active_model
                    .labels
                    .as_ref()
                    .clone()
                    .unwrap_or(serde_json::Value::Null);
                let new_labels_json = serde_json::to_value(labels).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to serialize labels: {}", e)})),
                    )
                })?;

                if current_labels_json != new_labels_json {
                    processor_active_model.labels = ActiveValue::Set(Some(new_labels_json));
                    model_updated = true;
                    debug!("Processor labels updated.");
                }
            }
            // Add checks for other metadata fields here if they become updatable without recreation
        }

        // Check min_replicas
        if let Some(new_min_replicas) = update_request.min_replicas {
            if new_min_replicas <= 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "min_replicas must be a positive integer"})),
                ));
            }
            let current_min_replicas = processor.min_replicas;
            if current_min_replicas != Some(new_min_replicas) {
                processor_active_model.min_replicas = ActiveValue::Set(Some(new_min_replicas));
                model_updated = true;
                debug!("Processor min_replicas updated to {}.", new_min_replicas);

                // Ensure desired_replicas is at least min_replicas
                let current_desired = processor.desired_replicas.unwrap_or(0);
                if current_desired < new_min_replicas {
                    debug!(
                        "Adjusting desired_replicas from {} to match new min_replicas {}",
                        current_desired, new_min_replicas
                    );
                    processor_active_model.desired_replicas =
                        ActiveValue::Set(Some(new_min_replicas));
                    // model_updated is already true
                }
            }
        }

        // Check max_replicas
        if let Some(new_max_replicas) = update_request.max_replicas {
            if new_max_replicas <= 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "max_replicas must be a positive integer"})),
                ));
            }
            let current_max_replicas = processor.max_replicas;
            if current_max_replicas != Some(new_max_replicas) {
                processor_active_model.max_replicas = ActiveValue::Set(Some(new_max_replicas));
                model_updated = true;
                debug!("Processor max_replicas updated to {}.", new_max_replicas);
            }
        }

        // Check schema
        if let Some(new_schema) = &update_request.schema {
            if processor_v1.schema != Some(new_schema.clone()) {
                processor_active_model.schema = ActiveValue::Set(Some(new_schema.clone()));
                model_updated = true;
                debug!("Processor schema updated.");
            }
        }

        // Check common_schema
        if let Some(new_common_schema) = &update_request.common_schema {
            if processor_v1.common_schema != Some(new_common_schema.clone()) {
                processor_active_model.common_schema =
                    ActiveValue::Set(Some(new_common_schema.clone()));
                model_updated = true;
                debug!("Processor common_schema updated.");
            }
        }

        // Check scale
        if let Some(new_scale) = &update_request.scale {
            if processor_v1.scale.as_ref() != Some(new_scale) {
                let new_scale_json = serde_json::to_value(new_scale).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to serialize scale: {}", e)})),
                    )
                })?;
                processor_active_model.scale = ActiveValue::Set(new_scale_json);
                model_updated = true;
                debug!("Processor scale updated.");
            }
        }

        if model_updated {
            debug!("Applying updates to processor.");
            let updated_processor_model =
                processor_active_model.update(db_pool).await.map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to update processor: {}", e)})),
                    )
                })?;
            let updated_processor_v1 = updated_processor_model.to_v1_processor().map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to convert updated processor: {}", e)})),
                )
            })?;
            return Ok(Json(updated_processor_v1));
        } else {
            debug!("No recreation required and no other updates detected. Returning original processor state.");
            // If no recreation and no other changes, return the original state
            Ok(Json(processor_v1))
        }
        // --- End: Handle updates ---
    }
}

pub async fn get_processor_logs(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    debug!(
        "Fetching logs for processor: {} in namespace: {}",
        name, namespace
    );
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    // --- Authorization and Processor Fetching (similar to get_processor) ---
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let processor = Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    .map_err(|e| {
        // Consider returning 404 if e indicates "not found"
        error!(
            "Database error finding processor {}:{}: {}",
            resolved_namespace, name, e
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to retrieve processor: {}", e)})),
        )
    })?;
    // --- End Authorization ---

    // --- Find Containers using owner_ref ---
    let owner_ref_string = format!("{}.{}.Processor", processor.name, processor.namespace);
    debug!(
        "Looking for containers with owner_ref: {}",
        owner_ref_string
    );

    let associated_containers = match Query::find_containers_by_owner_ref(
        db_pool,
        &owner_ref_string,
    )
    .await
    {
        Ok(containers) => containers,
        Err(e) => {
            error!(
                "Database error finding containers for processor {}:{} with owner_ref '{}': {}",
                resolved_namespace, name, owner_ref_string, e
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to retrieve associated containers: {}", e)})),
            ));
        }
    };

    if associated_containers.is_empty() {
        debug!(
            "No containers found associated with processor {}:{} (owner_ref: {})",
            resolved_namespace, name, owner_ref_string
        );
        return Ok(Json(json!({}))); // Return empty JSON if no containers found
    }
    // --- End Find Containers ---

    // --- Fetch Logs for Each Container ---
    let mut all_logs: HashMap<String, serde_json::Value> = HashMap::new();
    let mut container_errors: HashMap<String, String> = HashMap::new();

    for container in associated_containers {
        let container_id = container.id;
        let log_key = if container.name.is_empty() {
            container_id.clone()
        } else {
            container.name.clone()
        }; // Use container name or ID as key

        match crate::handlers::v1::container::_fetch_container_logs_by_id(
            db_pool,
            &container_id,
            &user_profile,
        )
        .await
        {
            Ok(Json(logs)) => {
                all_logs.insert(log_key, json!(logs));
            }
            Err((status, error_json)) => {
                let error_message = error_json
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                error!(
                    "Failed to fetch logs for container {}: Status {:?}, Error: {}",
                    container_id, status, error_message
                );
                // Store the error to potentially include in the response
                container_errors.insert(log_key, format!("Status {}: {}", status, error_message));
                all_logs.insert(container_id.clone(), json!({ "error": error_message }));
            }
        }
    }
    // --- End Fetch Logs ---

    // --- Prepare Response ---
    // Optionally, include errors in the response if needed
    // let response_json = if container_errors.is_empty() {
    //     json!(all_logs)
    // } else {
    //     json!({
    //         "logs": all_logs,
    //         "errors": container_errors
    //     })
    // };

    Ok(Json(json!(all_logs)))
}

#[axum::debug_handler]
pub async fn read_processor_stream(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
    Json(read_request): Json<V1ReadStreamRequest>,
) -> Result<Json<Vec<V1StreamMessage>>, (StatusCode, Json<serde_json::Value>)> {
    debug!(
        "Reading processor stream for {}/{} with group {}, max_records: {}, wait_ms: {}",
        namespace,
        name,
        read_request.consumer_group,
        read_request.max_records,
        read_request.wait_time_ms
    );
    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    let processor = Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    .map_err(|e| {
        error!(
            "Database error finding processor {}:{}: {}",
            resolved_namespace, name, e
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to retrieve processor: {}", e) })),
        )
    })?;

    let stream_name = processor.stream;

    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            let mut conn = client.get_connection().map_err(|e| {
                error!("Redis connection error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": format!("Redis connection error: {}", e) })),
                )
            })?;

            // Ensure consumer group exists, create if not (MKSTREAM handles stream non-existence)
            let group_create_result: Result<(), redis::RedisError> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(stream_name.clone())
                .arg(read_request.consumer_group.clone())
                .arg("0") // Start from the beginning if creating new
                .arg("MKSTREAM")
                .query(&mut conn);

            match group_create_result {
                Ok(_) => debug!(
                    "Consumer group '{}' ensured for stream '{}'",
                    read_request.consumer_group, stream_name
                ),
                Err(e) => {
                    // BUSYGROUP error is fine, means group already exists
                    if e.to_string().contains("BUSYGROUP") {
                        debug!(
                            "Consumer group '{}' already exists for stream '{}'",
                            read_request.consumer_group, stream_name
                        );
                    } else {
                        error!(
                            "Failed to create/ensure consumer group '{}' for stream '{}': {}",
                            read_request.consumer_group, stream_name, e
                        );
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(
                                json!({ "error": format!("Failed to setup consumer group: {}", e) }),
                            ),
                        ));
                    }
                }
            }

            debug!(
                "Reading from stream '{}' with group '{}', count {}, block {}ms",
                stream_name,
                read_request.consumer_group,
                read_request.max_records,
                read_request.wait_time_ms
            );

            let reply: redis::streams::StreamReadReply = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(read_request.consumer_group.clone())
                .arg(user_profile.email.clone()) // Consumer name, using user's email for now
                .arg("COUNT")
                .arg(read_request.max_records)
                .arg("BLOCK")
                .arg(read_request.wait_time_ms)
                .arg("STREAMS")
                .arg(stream_name.clone())
                .arg(">") // Read new messages not yet delivered to other consumers in this group
                .query(&mut conn)
                .map_err(|e| {
                    error!("XREADGROUP error for stream '{}': {}", stream_name, e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": format!("Failed to read from stream: {}", e) })),
                    )
                })?;

            let mut messages: Vec<V1StreamMessage> = Vec::new();
            if reply.keys.is_empty() {
                debug!(
                    "No new messages in stream '{}' for group '{}' within timeout",
                    stream_name, read_request.consumer_group
                );
                return Ok(Json(messages)); // Return empty list if no messages
            }

            for key in reply.keys {
                for id_entry in key.ids {
                    if let Some(data_val) = id_entry.map.get("data") {
                        let data_str = match data_val {
                            redis::Value::BulkString(bytes) => {
                                String::from_utf8_lossy(&bytes).to_string()
                            }
                            redis::Value::SimpleString(s) => s.clone(),
                            _ => {
                                warn!("Unexpected data format in stream: {:?}", data_val);
                                continue;
                            }
                        };
                        match serde_json::from_str::<V1StreamMessage>(&data_str) {
                            Ok(msg) => messages.push(msg),
                            Err(e) => {
                                error!("Failed to deserialize V1StreamMessage from stream data '{}': {}", data_str, e);
                                // Optionally, decide how to handle deserialization errors (e.g., skip, error out)
                            }
                        }
                    } else {
                        warn!(
                            "'data' field not found in message map for ID: {:?}",
                            id_entry.id
                        );
                    }
                }
            }

            Ok(Json(messages))
        }
        crate::state::MessageQueue::Kafka { .. } => Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "Kafka streams are not currently supported for consumer group reads" }),
            ),
        )),
    }
}

#[axum::debug_handler]
pub async fn read_return_message(
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name, message_id)): Path<(String, String, String)>,
    Json(read_request): Json<V1ReadStreamRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    debug!(
        "Reading return message for processor {}/{} with message_id: {}, wait_time: {}ms",
        namespace, name, message_id, read_request.wait_time_ms
    );

    let db_pool = &state.db_pool;
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);

    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the processor to get its stream name
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &resolved_namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {}", e)})),
            ));
        }
    };

    // Construct the return stream name
    let return_stream_name = format!("{}.return.{}", processor.stream, message_id);
    debug!("Constructed return stream name: {}", return_stream_name);

    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            // Get a Redis connection
            let mut conn = match client.get_connection() {
                Ok(conn) => {
                    debug!("Successfully obtained Redis connection for return stream read.");
                    conn
                }
                Err(e) => {
                    error!("Redis connection error: {}", e);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Redis connection error: {}", e)})),
                    ));
                }
            };

            // Check if the return stream exists
            let stream_exists: bool = match redis::cmd("EXISTS")
                .arg(&return_stream_name)
                .query(&mut conn)
            {
                Ok(exists) => exists,
                Err(e) => {
                    error!("Failed to check if return stream exists: {}", e);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to check stream existence: {}", e)})),
                    ));
                }
            };

            if !stream_exists {
                debug!("Return stream '{}' does not exist", return_stream_name);
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(
                        json!({"error": "Return stream not found - message may not exist or may have already been consumed"}),
                    ),
                ));
            }

            // Read from the return stream using XREAD with BLOCK and COUNT 1
            let wait_time_ms = read_request.wait_time_ms;
            debug!(
                "Reading ONE message from return stream '{}' with timeout {}ms",
                return_stream_name, wait_time_ms
            );

            // --- Move blocking call to spawn_blocking ---
            let return_stream_name_clone = return_stream_name.clone();
            let client_clone = client.clone();

            let read_result = tokio::task::spawn_blocking(move || {
                // Get a new connection from the pool inside the blocking task
                let mut conn = client_clone.get_connection().map_err(|e| {
                    redis::RedisError::from((
                        redis::ErrorKind::IoError,
                        "Failed to get connection in spawn_blocking",
                        e.to_string(),
                    ))
                })?;

                debug!(
                    "Attempting blocking XREAD on return stream '{}' with timeout {}ms and COUNT 1",
                    return_stream_name_clone, wait_time_ms
                );

                redis::cmd("XREAD")
                    .arg("COUNT")
                    .arg("1") // Only read one message at a time
                    .arg("BLOCK")
                    .arg(wait_time_ms)
                    .arg("STREAMS")
                    .arg(&return_stream_name_clone)
                    .arg("0") // Read from the beginning of the stream
                    .query::<redis::streams::StreamReadReply>(&mut conn)
            })
            .await;

            // Handle the result from spawn_blocking
            let result = match read_result {
                Ok(Ok(reply)) => {
                    debug!("XREAD successful for return stream. Raw reply: {:?}", reply);
                    reply
                }
                Ok(Err(e)) => {
                    error!(
                        "Error reading from return stream '{}': {}",
                        return_stream_name, e
                    );
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Error reading from return stream: {}", e)})),
                    ));
                }
                Err(e) => {
                    error!(
                        "Spawn_blocking task failed for return stream '{}': {}",
                        return_stream_name, e
                    );
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Task execution error: {}", e)})),
                    ));
                }
            };

            // Process the result without cleanup - keep stream intact for subsequent polls
            process_return_stream_result(result, &return_stream_name)
        }
        crate::state::MessageQueue::Kafka { .. } => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Kafka streams are not currently supported"})),
        )),
    }
}

// Helper function to process the Redis stream result
fn process_return_stream_result(
    result: redis::streams::StreamReadReply,
    return_stream_name: &str,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Check if we got a response
    if result.keys.is_empty() {
        error!(
            "Timed out or received empty response from return stream '{}'",
            return_stream_name
        );
        return Err((
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({"error": "Timed out waiting for return message"})),
        ));
    }

    debug!(
        "Received {} keys in response from return stream '{}'",
        result.keys.len(),
        return_stream_name
    );

    // Process the response
    for key in result.keys {
        debug!("Processing key (stream): {:?}", key.key);
        for id in key.ids {
            debug!("Processing message ID: {:?}, Map: {:?}", id.id, id.map);

            // Skip the init message if present
            if id.map.contains_key("init") {
                debug!("Skipping init message");
                continue;
            }

            if let Some(data_value) = id.map.get("data") {
                debug!("Found 'data' field: {:?}", data_value);
                // Convert the Redis value to a string
                let data_str = match data_value {
                    redis::Value::BulkString(bytes) => {
                        let s = String::from_utf8_lossy(bytes).to_string();
                        debug!("Converted BulkString to string: '{}'", s);
                        s
                    }
                    redis::Value::SimpleString(s) => s.clone(),
                    _ => format!("{:?}", data_value),
                };
                debug!("Final data_str: '{}'", data_str);

                // Try to parse the data as JSON
                match serde_json::from_str::<serde_json::Value>(&data_str) {
                    Ok(json_data) => {
                        debug!("Successfully parsed data as JSON: {:?}", json_data);
                        return Ok(Json(json_data));
                    }
                    Err(e) => {
                        warn!(
                            "Failed to parse return data as JSON: {}. Returning raw string.",
                            e
                        );
                        return Ok(Json(json!({"raw_health_response": data_str})));
                    }
                }
            } else {
                debug!("'data' field not found in message map for ID: {:?}", id.id);
            }
        }
    }

    // If we couldn't find data in the response
    error!(
        "Processed all messages in return stream '{}', but none contained a 'data' field.",
        return_stream_name
    );
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Received return message without data field"})),
    ))
}

/// Bidirectional WebSocket endpoint for interactive communication with processors  
/// Allows sending and receiving messages through a persistent WebSocket connection
///
/// # Features
/// - Send messages to processor via WebSocket (no HTTP POST needed)
/// - Receive responses via the same WebSocket  
/// - Supports streaming generators with multiple chunks
/// - Persistent connection for ongoing conversation
/// - Automatic message ID generation and return stream management
///
/// # Message Format
/// Send JSON messages like: `{"content": {"greeting": "Hello"}}`
/// Receive responses as they arrive from the processor
///
/// # URL Format  
/// `/v1/processors/{namespace}/{name}/ws`
pub async fn processor_websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name)): Path<(String, String)>,
) -> impl IntoResponse {
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);
    ws.on_upgrade(move |socket| {
        handle_bidirectional_processor_socket(socket, state, user_profile, resolved_namespace, name)
    })
}

/// WebSocket endpoint for reading processor return messages
/// Supports both single responses and streaming generators
///
/// # Streaming Support
/// This endpoint is designed to handle processor generators that can return multiple
/// stream chunks followed by a final response. When a processor has `stream=true`,
/// it can:
///
/// 1. Return multiple chunk messages with incremental data ("StreamChunkMessage" with "status": "stream")
/// 2. Signal completion with "StreamResponseMessage" having "status": "success" and "content": null
/// 3. Support real-time streaming of responses via WebSocket
///
/// # Usage
/// - For single responses: Connect to the WebSocket and receive one message
/// - For streaming responses: Connect and receive multiple messages until completion
/// - The WebSocket will automatically clean up the return stream when done
///
/// # URL Format
/// `/v1/processors/{namespace}/{name}/return/{message_id}/stream`
pub async fn stream_processor_return_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(user_profile): Extension<V1UserProfile>,
    Path((namespace, name, message_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let resolved_namespace = resolve_namespace(&namespace, &user_profile);
    ws.on_upgrade(move |socket| {
        handle_processor_return_socket(
            socket,
            state,
            user_profile,
            resolved_namespace,
            name,
            message_id,
        )
    })
}

/// Handle WebSocket connection for processor return messages
async fn handle_processor_return_socket(
    socket: WebSocket,
    state: AppState,
    user_profile: V1UserProfile,
    namespace: String,
    name: String,
    message_id: String,
) {
    debug!(
        "WebSocket upgrade request received for processor return: {}/{} message_id: {}",
        namespace, name, message_id
    );

    let db_pool = &state.db_pool;
    let (sender, _receiver) = socket.split();

    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the processor to get its stream name
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            let mut sender_locked = sender;
            let _ = sender_locked
                .send(Message::Text(format!("Error fetching processor: {}", e)))
                .await;
            let _ = sender_locked.close().await;
            return;
        }
    };

    // Construct the return stream name
    let return_stream_name = format!("{}.return.{}", processor.stream, message_id);
    debug!("Constructed return stream name: {}", return_stream_name);

    // Start streaming processor return messages
    stream_processor_return_messages(sender, state, return_stream_name).await;
}

/// Stream processor return messages via WebSocket
async fn stream_processor_return_messages<S>(sender: S, state: AppState, return_stream_name: String)
where
    S: SinkExt<Message> + Unpin + Send + 'static,
    <S as futures::Sink<Message>>::Error: std::fmt::Debug + Send,
{
    let sender = Arc::new(Mutex::new(sender));

    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            // Get a Redis connection
            let mut conn = match client.get_connection() {
                Ok(conn) => {
                    debug!("Successfully obtained Redis connection for return stream read.");
                    conn
                }
                Err(e) => {
                    error!("Redis connection error: {}", e);
                    let mut sender_lock = sender.lock().await;
                    let _ = sender_lock
                        .send(Message::Text(format!("Redis connection error: {}", e)))
                        .await;
                    let _ = sender_lock.close().await;
                    return;
                }
            };

            // Check if the return stream exists
            let stream_exists: bool = match redis::cmd("EXISTS")
                .arg(&return_stream_name)
                .query(&mut conn)
            {
                Ok(exists) => exists,
                Err(e) => {
                    error!("Failed to check if return stream exists: {}", e);
                    let mut sender_lock = sender.lock().await;
                    let _ = sender_lock
                        .send(Message::Text(format!(
                            "Failed to check stream existence: {}",
                            e
                        )))
                        .await;
                    let _ = sender_lock.close().await;
                    return;
                }
            };

            if !stream_exists {
                debug!("Return stream '{}' does not exist", return_stream_name);
                let mut sender_lock = sender.lock().await;
                let _ = sender_lock
                    .send(Message::Text("Return stream not found - message may not exist or may have already been consumed".to_string()))
                    .await;
                let _ = sender_lock.close().await;
                return;
            }

            // Stream messages continuously until the stream is complete
            let mut last_id = "0".to_string(); // Start from beginning
            let mut stream_complete = false;
            let mut message_count = 0;
            const MAX_WAIT_MS: u64 = 30000; // 30 seconds timeout per read
            const MAX_MESSAGES: usize = 1000; // Prevent infinite loops

            while !stream_complete && message_count < MAX_MESSAGES {
                debug!(
                    "Reading from return stream '{}' starting from ID '{}'",
                    return_stream_name, last_id
                );

                // Move the blocking call to spawn_blocking
                let return_stream_name_clone = return_stream_name.clone();
                let last_id_clone = last_id.clone();
                let client_clone = client.clone();

                let read_result = tokio::task::spawn_blocking(move || {
                    let mut conn = client_clone.get_connection().map_err(|e| {
                        redis::RedisError::from((
                            redis::ErrorKind::IoError,
                            "Failed to get connection in spawn_blocking",
                            e.to_string(),
                        ))
                    })?;

                    debug!(
                        "Attempting blocking XREAD on return stream '{}' with timeout {}ms from ID '{}'",
                        return_stream_name_clone, MAX_WAIT_MS, last_id_clone
                    );

                    redis::cmd("XREAD")
                        .arg("BLOCK")
                        .arg(MAX_WAIT_MS)
                        .arg("STREAMS")
                        .arg(&return_stream_name_clone)
                        .arg(&last_id_clone)
                        .query::<redis::streams::StreamReadReply>(&mut conn)
                })
                .await;

                // Handle the result from spawn_blocking
                let result = match read_result {
                    Ok(Ok(reply)) => {
                        debug!("XREAD successful for return stream. Raw reply: {:?}", reply);
                        reply
                    }
                    Ok(Err(e)) => {
                        if e.to_string().contains("timeout") {
                            debug!(
                                "Timeout waiting for messages on return stream '{}'",
                                return_stream_name
                            );
                            stream_complete = true;
                            break;
                        } else {
                            error!(
                                "Error reading from return stream '{}': {}",
                                return_stream_name, e
                            );
                            let mut sender_lock = sender.lock().await;
                            let _ = sender_lock
                                .send(Message::Text(format!(
                                    "Error reading from return stream: {}",
                                    e
                                )))
                                .await;
                            break;
                        }
                    }
                    Err(e) => {
                        error!(
                            "Spawn_blocking task failed for return stream '{}': {}",
                            return_stream_name, e
                        );
                        let mut sender_lock = sender.lock().await;
                        let _ = sender_lock
                            .send(Message::Text(format!("Task execution error: {}", e)))
                            .await;
                        break;
                    }
                };

                // Process messages from this read
                if result.keys.is_empty() {
                    debug!("No more messages in return stream '{}'", return_stream_name);
                    stream_complete = true;
                    break;
                }

                for key in result.keys {
                    debug!("Processing key (stream): {:?}", key.key);
                    for id_entry in key.ids {
                        debug!(
                            "Processing message ID: {:?}, Map: {:?}",
                            id_entry.id, id_entry.map
                        );
                        last_id = id_entry.id.clone(); // Update last processed ID
                        message_count += 1;

                        // Skip the init message if present
                        if id_entry.map.contains_key("init") {
                            debug!("Skipping init message");
                            continue;
                        }

                        if let Some(data_value) = id_entry.map.get("data") {
                            debug!("Found 'data' field: {:?}", data_value);

                            // Convert the Redis value to a string
                            let data_str = match data_value {
                                redis::Value::BulkString(bytes) => {
                                    String::from_utf8_lossy(bytes).to_string()
                                }
                                redis::Value::SimpleString(s) => s.clone(),
                                _ => format!("{:?}", data_value),
                            };
                            debug!("Data string: '{}'", data_str);

                            // Try to parse the data as JSON
                            match serde_json::from_str::<serde_json::Value>(&data_str) {
                                Ok(json_data) => {
                                    debug!("Successfully parsed data as JSON: {:?}", json_data);

                                    // Check if this is a stream completion message
                                    if let Some(stream_complete_flag) =
                                        json_data.get("stream_complete")
                                    {
                                        if stream_complete_flag.as_bool().unwrap_or(false) {
                                            debug!("Received stream completion signal");
                                            stream_complete = true;
                                        }
                                    }

                                    // Also check for StreamResponseMessage with success status (actual completion pattern)
                                    if let Some(kind) = json_data.get("kind") {
                                        if kind.as_str() == Some("StreamResponseMessage") {
                                            if let Some(status) = json_data.get("status") {
                                                if status.as_str() == Some("success") {
                                                    debug!("Received StreamResponseMessage with success status - stream complete");
                                                    stream_complete = true;
                                                }
                                            }
                                        }
                                    }

                                    // Send the message to the client
                                    let mut sender_lock = sender.lock().await;
                                    match sender_lock.send(Message::Text(data_str)).await {
                                        Ok(_) => debug!("Sent message to WebSocket client"),
                                        Err(e) => {
                                            debug!("Client disconnected: {:?}", e);
                                            stream_complete = true;
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to parse return data as JSON: {}. Sending raw string.", e);
                                    let mut sender_lock = sender.lock().await;
                                    let raw_message = json!({"raw_response": data_str, "parse_error": e.to_string()});
                                    if let Err(send_err) = sender_lock
                                        .send(Message::Text(raw_message.to_string()))
                                        .await
                                    {
                                        debug!("Client disconnected: {:?}", send_err);
                                        stream_complete = true;
                                        break;
                                    }
                                }
                            }
                        } else {
                            debug!(
                                "'data' field not found in message map for ID: {:?}",
                                id_entry.id
                            );
                        }
                    }
                }
            }

            // Clean up the return stream after reading
            debug!("Cleaning up return stream '{}'", return_stream_name);
            let mut conn = match client.get_connection() {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to get connection for DEL command: {}", e);
                    let mut sender_lock = sender.lock().await;
                    let _ = sender_lock.close().await;
                    return;
                }
            };

            let del_result: Result<(), redis::RedisError> =
                redis::cmd("DEL").arg(&return_stream_name).query(&mut conn);

            if let Err(e) = del_result {
                warn!(
                    "Failed to delete return stream '{}': {}",
                    return_stream_name, e
                );
            } else {
                debug!(
                    "Successfully deleted return stream '{}'",
                    return_stream_name
                );
            }

            // Send completion message and close connection
            {
                let mut sender_lock = sender.lock().await;
                if message_count == 0 {
                    let _ = sender_lock
                        .send(Message::Text(
                            "No messages found in return stream".to_string(),
                        ))
                        .await;
                } else {
                    let completion_msg = json!({
                        "stream_complete": true,
                        "message_count": message_count,
                        "reason": if message_count >= MAX_MESSAGES { "max_messages_reached" } else { "stream_ended" }
                    });
                    let _ = sender_lock
                        .send(Message::Text(completion_msg.to_string()))
                        .await;
                }
                let _ = sender_lock.close().await;
                debug!("WebSocket connection closed by server.");
            }
        }
        crate::state::MessageQueue::Kafka { .. } => {
            let mut sender_lock = sender.lock().await;
            let _ = sender_lock
                .send(Message::Text(
                    "Kafka streams are not currently supported for WebSocket streaming".to_string(),
                ))
                .await;
            let _ = sender_lock.close().await;
        }
    }
}

/// Handle bidirectional WebSocket connection for interactive processor communication
async fn handle_bidirectional_processor_socket(
    socket: WebSocket,
    state: AppState,
    user_profile: V1UserProfile,
    namespace: String,
    name: String,
) {
    debug!(
        "Bidirectional WebSocket connection established for processor: {}/{}",
        namespace, name
    );

    let db_pool = &state.db_pool;
    let (ws_sender, mut ws_receiver) = socket.split();
    let ws_sender = Arc::new(Mutex::new(ws_sender));

    // Collect owner IDs from user_profile
    let mut owner_ids: Vec<String> = if let Some(orgs) = &user_profile.organizations {
        orgs.keys().cloned().collect()
    } else {
        Vec::new()
    };
    owner_ids.push(user_profile.email.clone());
    let owner_id_refs: Vec<&str> = owner_ids.iter().map(|s| s.as_str()).collect();

    // Find the processor
    let processor = match Query::find_processor_by_namespace_name_and_owners(
        db_pool,
        &namespace,
        &name,
        &owner_id_refs,
    )
    .await
    {
        Ok(processor) => processor,
        Err(e) => {
            let mut sender_lock = ws_sender.lock().await;
            let _ = sender_lock
                .send(Message::Text(format!("Error fetching processor: {}", e)))
                .await;
            let _ = sender_lock.close().await;
            return;
        }
    };

    // Get user token for agent key generation
    let user_token = user_profile.token.clone().unwrap_or_default();
    if user_token.is_empty() {
        let mut sender_lock = ws_sender.lock().await;
        let _ = sender_lock
            .send(Message::Text("Authentication token missing".to_string()))
            .await;
        let _ = sender_lock.close().await;
        return;
    }

    // Generate agent key (similar to send_processor logic)
    let agent_key = if user_token.starts_with("a.") || user_token.starts_with("k.") {
        user_token.clone()
    } else {
        match generate_agent_key_for_processor(&processor, &user_token).await {
            Ok(key) => key,
            Err(e) => {
                let mut sender_lock = ws_sender.lock().await;
                let _ = sender_lock
                    .send(Message::Text(format!(
                        "Failed to generate agent key: {}",
                        e
                    )))
                    .await;
                let _ = sender_lock.close().await;
                return;
            }
        }
    };

    // Send welcome message
    {
        let mut sender_lock = ws_sender.lock().await;
        let welcome_msg = json!({
            "type": "connection_established",
            "processor": {
                "namespace": namespace,
                "name": name,
                "stream": processor.stream
            },
            "message": "WebSocket connection established. Send JSON messages to interact with the processor."
        });
        if let Err(e) = sender_lock
            .send(Message::Text(welcome_msg.to_string()))
            .await
        {
            error!("Failed to send welcome message: {:?}", e);
            return;
        }
    }

    // Process incoming messages from the WebSocket
    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                debug!("Received WebSocket message: {}", text);

                // Parse the incoming message
                let content = match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(json) => json,
                    Err(e) => {
                        let mut sender_lock = ws_sender.lock().await;
                        let error_msg = json!({
                            "type": "error",
                            "error": "Invalid JSON",
                            "details": e.to_string()
                        });
                        let _ = sender_lock.send(Message::Text(error_msg.to_string())).await;
                        continue;
                    }
                };

                // Generate unique message ID
                let message_id = ShortUuid::generate().to_string();
                let return_stream_name = format!("{}.return.{}", processor.stream, message_id);

                // Handle the message in a separate task to avoid blocking the WebSocket receiver
                let state_clone = state.clone();
                let processor_clone = processor.clone();
                let user_profile_clone = user_profile.clone();
                let agent_key_clone = agent_key.clone();
                let ws_sender_clone = ws_sender.clone();
                let return_stream_name_clone = return_stream_name.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_websocket_message(
                        content,
                        message_id,
                        return_stream_name_clone,
                        state_clone,
                        processor_clone,
                        user_profile_clone,
                        agent_key_clone,
                        ws_sender_clone,
                    )
                    .await
                    {
                        error!("Error handling WebSocket message: {}", e);
                    }
                });
            }
            Ok(Message::Close(_)) => {
                debug!("WebSocket connection closed by client");
                break;
            }
            Ok(Message::Ping(data)) => {
                let mut sender_lock = ws_sender.lock().await;
                let _ = sender_lock.send(Message::Pong(data)).await;
            }
            Ok(_) => {
                // Ignore other message types (Binary, Pong)
            }
            Err(e) => {
                error!("WebSocket error: {:?}", e);
                break;
            }
        }
    }

    debug!(
        "Bidirectional WebSocket connection closed for processor: {}/{}",
        namespace, name
    );
}

/// Helper function to generate agent key for processor (extracted from send_processor)
async fn generate_agent_key_for_processor(
    processor: &crate::entities::processors::Model,
    user_token: &str,
) -> Result<String, String> {
    let auth_server = SERVER_CONFIG.auth.url.clone();
    if auth_server.is_empty() {
        return Err("Auth server configuration missing".to_string());
    }

    let agent_key_request = crate::models::V1CreateAgentKeyRequest {
        agent_id: format!("processor-{}", processor.id),
        name: format!(
            "websocket-processor-{}-{}",
            processor.id,
            ShortUuid::generate().to_string()
        ),
        duration: 86400, // 24 hour validity
    };

    let agent_key_response =
        crate::agent::agent::create_agent_key(&auth_server, user_token, agent_key_request)
            .await
            .map_err(|e| format!("Failed to create agent key: {}", e))?;

    agent_key_response
        .key
        .ok_or_else(|| "Agent key response missing key".to_string())
}

/// Handle a single WebSocket message by sending it to the processor and streaming back responses
async fn handle_websocket_message(
    content: serde_json::Value,
    message_id: String,
    return_stream_name: String,
    state: AppState,
    processor: crate::entities::processors::Model,
    user_profile: V1UserProfile,
    agent_key: String,
    ws_sender: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!(
        "Processing WebSocket message {} for processor {}",
        message_id, processor.id
    );

    // Get user profile for the message
    let user_prof = get_user_profile_from_token(
        &state.db_pool,
        &user_profile.token.clone().unwrap_or_default(),
    )
    .await
    .map_err(|e| format!("Failed to get user profile: {:?}", e))?;

    // Create the stream message
    let stream_message = V1StreamMessage {
        kind: "StreamMessage".to_string(),
        id: message_id.clone(),
        content,
        created_at: chrono::Utc::now().timestamp(),
        return_stream: Some(return_stream_name.clone()),
        user_id: Some(user_prof.email.clone()),
        orgs: user_prof.organizations.clone().map(|orgs| json!(orgs)),
        handle: user_prof.handle.clone(),
        adapter: Some(format!("processor:{}", processor.id)),
        api_key: Some(agent_key),
    };

    // Send message to processor stream
    match &state.message_queue {
        crate::state::MessageQueue::Redis { client } => {
            let mut conn = client
                .get_connection()
                .map_err(|e| format!("Redis connection error: {}", e))?;

            let message_json = serde_json::to_string(&stream_message)
                .map_err(|e| format!("Failed to serialize message: {}", e))?;

            let _stream_id: String = redis::cmd("XADD")
                .arg(&processor.stream)
                .arg("*")
                .arg("data")
                .arg(&message_json)
                .query(&mut conn)
                .map_err(|e| format!("Failed to send message to stream: {}", e))?;

            debug!(
                "Sent WebSocket message {} to processor stream {}",
                message_id, processor.stream
            );

            // Start listening for responses in a separate task
            let client_clone = client.clone();
            let return_stream_name_clone = return_stream_name.clone();
            let ws_sender_clone = ws_sender.clone();
            let message_id_clone = message_id.clone();

            tokio::spawn(async move {
                if let Err(e) = stream_return_messages_to_websocket(
                    (*client_clone).clone(),
                    return_stream_name_clone,
                    ws_sender_clone,
                    message_id_clone,
                )
                .await
                {
                    error!("Error streaming return messages: {}", e);
                }
            });
        }
        crate::state::MessageQueue::Kafka { .. } => {
            return Err("Kafka streams are not currently supported for WebSocket".into());
        }
    }

    Ok(())
}

/// Stream return messages from Redis to WebSocket client
async fn stream_return_messages_to_websocket(
    client: redis::Client,
    return_stream_name: String,
    ws_sender: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
    message_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("Starting to stream return messages for {}", message_id);

    // Wait a moment for the return stream to be created
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut last_id = "0".to_string();
    let mut stream_complete = false;
    let mut message_count = 0;
    const MAX_WAIT_MS: u64 = 30000; // 30 seconds timeout per read
    const MAX_MESSAGES: usize = 1000; // Prevent infinite loops

    while !stream_complete && message_count < MAX_MESSAGES {
        let client_clone = client.clone();
        let return_stream_name_clone = return_stream_name.clone();
        let last_id_clone = last_id.clone();

        let read_result = tokio::task::spawn_blocking(move || {
            let mut conn = client_clone.get_connection()?;
            redis::cmd("XREAD")
                .arg("BLOCK")
                .arg(MAX_WAIT_MS)
                .arg("STREAMS")
                .arg(&return_stream_name_clone)
                .arg(&last_id_clone)
                .query::<redis::streams::StreamReadReply>(&mut conn)
        })
        .await??;

        if read_result.keys.is_empty() {
            debug!("No more messages for {}, completing", message_id);
            break;
        }

        for key in read_result.keys {
            for id_entry in key.ids {
                last_id = id_entry.id.clone();
                message_count += 1;

                // Skip init messages
                if id_entry.map.contains_key("init") {
                    continue;
                }

                if let Some(data_value) = id_entry.map.get("data") {
                    let data_str = match data_value {
                        redis::Value::BulkString(bytes) => {
                            String::from_utf8_lossy(bytes).to_string()
                        }
                        redis::Value::SimpleString(s) => s.clone(),
                        _ => format!("{:?}", data_value),
                    };

                    // Parse and check for completion
                    if let Ok(json_data) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        // Check for completion signals
                        if json_data
                            .get("stream_complete")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            stream_complete = true;
                        }

                        if let Some(kind) = json_data.get("kind") {
                            if kind.as_str() == Some("StreamResponseMessage") {
                                if let Some(status) = json_data.get("status") {
                                    if status.as_str() == Some("success") {
                                        stream_complete = true;
                                    }
                                }
                            }
                        }
                    }

                    // Send to WebSocket client
                    {
                        let mut sender_lock = ws_sender.lock().await;
                        if let Err(e) = sender_lock.send(Message::Text(data_str)).await {
                            debug!("WebSocket client disconnected: {:?}", e);
                            return Ok(());
                        }
                    }

                    if stream_complete {
                        break;
                    }
                }
            }
        }
    }

    debug!("Completed streaming return messages for {}", message_id);
    Ok(())
}
