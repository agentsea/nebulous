use crate::models::V1AuthzConfig;
use crate::models::V1UserProfile;
use crate::proxy::authz::evaluate_authorization_rules;
use crate::proxy::meters::{send_request_metrics, send_response_metrics};
use crate::query::Query;
use crate::resources::v1::containers::base::get_vpn_device_name;
use crate::AppState;
use axum::body::Body;
use axum::http::Uri;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tracing::{debug, error};

#[allow(dead_code)]
pub async fn forward_container(
    State(_app_state): State<AppState>, // replace with actual state usage if needed
    user_profile: V1UserProfile,
    namespace: String,
    name: String,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
    original_uri: Uri,
) -> impl IntoResponse {
    // 2) Fetch container from DB just like before
    let container_model =
        match Query::find_container_by_namespace_and_name(&_app_state.db_pool, &namespace, &name)
            .await
        {
            Ok(Some(c)) => c,
            Ok(None) => {
                return (StatusCode::NOT_FOUND, "No container found").into_response();
            }
            Err(e) => {
                eprintln!("[PROXY] Database error: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
            }
        };

    debug!("[PROXY] Container model: {container_model:?}");

    // 2) Deserialize authz config
    let authz_config = match container_model.clone().authz {
        Some(json_val) => {
            serde_json::from_value::<V1AuthzConfig>(json_val).unwrap_or_default()
            // or handle parse errors
        }
        None => V1AuthzConfig::default(),
    };

    debug!("[PROXY] Authz config: {authz_config:?}");

    // Example for JSON body parsing if "application/json"
    let mut json_body_opt: Option<Value> = None;
    if let Some(content_type) = headers.get("content-type") {
        if content_type
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("application/json")
        {
            if let Ok(json_body) = serde_json::from_slice::<Value>(&body) {
                json_body_opt = Some(json_body);
            }
        }
    }

    debug!("[PROXY] JSON body: {json_body_opt:?}");

    // Example: Evaluate each rule
    let mut is_allowed = authz_config.default_action != "deny";

    // Build your request path for matching:
    let request_path = format!("/containers/{}/{}", namespace, name);

    evaluate_authorization_rules(
        &mut is_allowed,
        &user_profile,
        &authz_config,
        &request_path,
        json_body_opt.as_ref(),
    );

    if !is_allowed {
        return (StatusCode::FORBIDDEN, "Access denied").into_response();
    }

    // ---------------------------------------------------
    //  Send request_value metrics to openmeter
    // ---------------------------------------------------
    let maybe_meters = container_model.parse_meters().unwrap_or(None);

    // Only iterate if we actually have some meters
    if let Some(ref meters) = maybe_meters.clone() {
        match send_request_metrics(&container_model.id, meters, &json_body_opt).await {
            Ok(_) => {
                debug!("[Proxy] Successfully sent metrics to OpenMeter");
            }
            Err(e) => {
                error!("[Proxy] Failed to send metrics to OpenMeter: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("OpenMeter error: {}", e),
                )
                    .into_response();
            }
        }
    }

    // If this is a JSON request, parse and print the JSON body
    if let Some(content_type) = headers.get("content-type") {
        if content_type
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("application/json")
        {
            if let Ok(json_body) = serde_json::from_slice::<Value>(&body) {
                println!("[PROXY] ➡️ Request JSON Body: {json_body}");
            }
        }
    }

    let hostname = match container_model.tailnet_ip {
        Some(ip) => ip,
        None => get_vpn_device_name(&container_model).await,
    };

    debug!("[PROXY] Hostname: {hostname}");

    // Here is the change: include port if we have it
    let port_str = if let Some(port) = container_model.proxy_port {
        format!(":{}", port)
    } else {
        "".to_string()
    };

    // Integrate it into the target URL
    debug!("[PROXY] Original URI: {}", original_uri);

    let full_uri = original_uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("");

    let target_url = format!("http://{}{}{}", hostname, port_str, full_uri);
    debug!("[PROXY] Target URL with path: {target_url}");

    let client = reqwest::Client::new();

    // Build outbound request with the same method, body, and forwarded headers
    let mut req_builder = client.request(method.clone(), &target_url).body(body);

    // Forward headers, skipping "transfer-encoding" if needed
    for (key, value) in headers.iter() {
        if key.as_str().eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        req_builder = req_builder.header(key, value);
    }

    // Send the request
    match req_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            let bytes = resp.bytes().await.unwrap_or_else(|_| Bytes::new());

            // If response is JSON, parse and print
            if let Some(content_type) = resp_headers.get("content-type") {
                if content_type
                    .to_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains("application/json")
                {
                    if let Ok(json_resp) = serde_json::from_slice::<Value>(&bytes) {
                        println!("[PROXY] ⬅️ Response JSON Body: {json_resp}");

                        // Replace response metrics logic with a call to the new function
                        if let Some(ref meters) = maybe_meters.clone() {
                            if let Err(e) =
                                send_response_metrics(&container_model.id, meters, &json_resp).await
                            {
                                error!("[Proxy] Failed to send response metrics: {}", e);
                            }
                        }
                    }
                }
            }

            // Build an Axum response
            let mut response = Response::builder().status(status);
            for (key, value) in resp_headers.iter() {
                response = response.header(key, value);
            }
            response.body(Body::from(bytes)).unwrap().into_response()
        }
        Err(e) => {
            eprintln!("[PROXY] ❌ Forwarding error: {e}");
            (StatusCode::BAD_GATEWAY, "Failed to forward request").into_response()
        }
    }
}
