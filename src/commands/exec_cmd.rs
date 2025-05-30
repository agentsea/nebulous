use crate::cli::ExecArgs;
use crate::commands::request::server_request;
use reqwest::Client;
use serde::Deserialize;
use std::error::Error as StdError;

/// This is your main “exec” function, to be called from your CLI command.
/// 1) Fetch container via HTTP API to retrieve its ID.  
/// 2) Run local SSH command using `run_ssh_command_ts`.
pub async fn exec_cmd(args: ExecArgs) -> Result<(), Box<dyn StdError>> {
    // Step 1: Fetch container ID by calling your server’s HTTP GET /v1/containers/:namespace/:name
    let container_id = fetch_container_id_from_api(&args.namespace, &args.name).await?;

    // Step 2: Run the local SSH command using the ID as the SSH host (e.g. Tailscale address).
    //         This now uses the streaming `stream_ssh_command_ts`.
    nebulous::ssh::exec::stream_ssh_command_ts(
        &format!("container-{}", container_id),
        args.command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect(),
        args.interactive,
        args.tty,
        Some("root"), // Example: pass Some("root") if you need a specific user
    )?;

    // Output is now streamed directly by stream_ssh_command_ts.
    // println!("{}", output);
    Ok(())
}

/// Helper function: calls GET /v1/containers/<namespace>/<name>
/// and returns the container’s `.metadata.id`.
async fn fetch_container_id_from_api(
    namespace: &str,
    name: &str,
) -> Result<String, Box<dyn StdError>> {
    let url = format!("/v1/containers/{}/{}", namespace, name);
    let response = server_request(&url, reqwest::Method::GET).await?;

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
