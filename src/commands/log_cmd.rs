use nebulous::config::ClientConfig;
use std::error::Error as StdError;

use reqwest::Client;
use serde::Deserialize;

pub async fn fetch_container_logs(
    name: String,
    namespace: String,
) -> Result<String, Box<dyn StdError>> {
    // Step 1: Fetch container ID by calling your server’s HTTP GET /v1/containers/:namespace/:name
    let container_id = fetch_container_id_from_api(&namespace, &name).await?;

    // Load config the same way as in get_cmd.rs
    let config = ClientConfig::read()?;
    let current_server = config.get_current_server_config().unwrap();
    let _server = current_server.server.as_ref().unwrap();
    let api_key = current_server.api_key.as_ref().unwrap();

    let _bearer_token = format!("Bearer {}", api_key);

    // Step 2: Run the local SSH command using the ID as the SSH host (e.g. Tailscale address).
    //         This uses the synchronous `run_ssh_command_ts` from your existing code.
    let output = nebulous::ssh::exec::run_ssh_command_ts(
        &format!("container-{}", container_id),
        vec![
            "cat".to_string(),
            "$HOME/.logs/nebu_container.log".to_string(),
        ],
        false,
        false,
        Some("root"), // TODO: need to fetch from the API
    )?;

    // Output the SSH command’s stdout
    println!("{}", output);
    Ok(output)
}

/// Helper function: calls GET /v1/containers/<namespace>/<name>
/// and returns the container’s `.metadata.id`.
async fn fetch_container_id_from_api(
    namespace: &str,
    name: &str,
) -> Result<String, Box<dyn StdError>> {
    let config = nebulous::config::ClientConfig::read()?;
    let current_server = config.get_current_server_config().unwrap();
    let server = current_server.server.as_ref().unwrap();
    let api_key = current_server.api_key.as_ref().unwrap();
    // Adjust base URL/host as needed:
    let url = format!("{}/v1/containers/{}/{}", server, namespace, name);

    // Use reqwest to fetch container JSON
    let client = Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await?;

    let container = response
        .error_for_status()? // Return Err if e.g. 404 or 500
        .json::<V1Container>()
        .await?;

    Ok(container.metadata.id)
}

/// Minimal struct matching the server’s “Container” JSON shape.
/// We only need the `metadata.id` field for this flow.
#[derive(Deserialize)]
struct V1Container {
    metadata: V1ResourceMeta,
}

/// Minimal struct for container’s metadata (includes ID).
#[derive(Deserialize)]
struct V1ResourceMeta {
    pub id: String,
}
