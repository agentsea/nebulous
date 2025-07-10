use nebulous::config::ClientConfig;
use std::error::Error as StdError;
use std::io::Write;

use futures::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use tokio_tungstenite::{
    connect_async, tungstenite::http::Request, tungstenite::protocol::Message,
};

pub async fn fetch_container_logs(
    name: String,
    namespace: Option<String>,
    follow: bool,
) -> Result<String, Box<dyn StdError>> {
    // Load config
    let config = ClientConfig::read()?;
    let current_server = config.get_current_server_config().unwrap();
    let server = current_server.server.as_ref().unwrap();
    let api_key = current_server.api_key.as_ref().unwrap();

    let namespace = namespace.unwrap_or("-".to_string());

    if follow {
        // Use WebSocket to stream logs
        let ws_url = server
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let ws_url = format!(
            "{}/v1/containers/{}/{}/logs/stream",
            ws_url, namespace, name
        );

        // Create a request with authorization header for the handshake
        let request = Request::builder()
            .uri(ws_url)
            .header("Authorization", format!("Bearer {}", api_key))
            // Add necessary WebSocket handshake headers (tungstenite builder handles Sec-WebSocket-Key, etc.)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13") // Standard version
            .body(())?;

        // Connect to the WebSocket endpoint with the request
        let (ws_stream, response) = connect_async(request).await?;

        // Check the response status code from the handshake
        if !response.status().is_success() {
            return Err(format!("WebSocket handshake failed: Status {}", response.status()).into());
        }
        println!("WebSocket connection established");

        // Split the stream
        let (mut write, mut read) = ws_stream.split();

        // Buffer stdout for potentially faster printing of rapid log lines
        let stdout_handle = std::io::stdout();
        let mut buffered_stdout = std::io::BufWriter::new(stdout_handle.lock());

        // Print incoming messages until connection is closed
        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    if let Err(e) = writeln!(buffered_stdout, "{}", text) {
                        // If printing fails, log to stderr and potentially break or handle
                        eprintln!(
                            "Error writing to buffered stdout: {}. Log line: {}",
                            e, text
                        );
                        // Consider breaking if stdout is consistently failing
                        // break;
                    }
                }
                Ok(Message::Close(_)) => {
                    if let Err(e) = writeln!(buffered_stdout, "Connection closed by server") {
                        eprintln!("Error writing to buffered stdout: {}", e);
                    }
                    break;
                }
                Ok(Message::Ping(ping_data)) => {
                    // Respond to pings to keep the connection alive
                    if let Err(e) = write.send(Message::Pong(ping_data)).await {
                        if let Err(print_err) =
                            writeln!(buffered_stdout, "Error sending pong: {}", e)
                        {
                            eprintln!("Error writing to buffered stdout: {}", print_err);
                        }
                        // Depending on severity, might want to break or log this error
                        // eprintln!("WebSocket error sending pong: {}", e);
                        // return Err(e.into()); // Or handle differently
                    }
                }
                Err(e) => {
                    // Log WebSocket errors to stderr (or buffered_stdout if preferred, though stderr is typical for errors)
                    eprintln!("WebSocket error: {}", e);
                    // Ensure an error message is flushed if we are about to return an error
                    let _ = buffered_stdout.flush();
                    return Err(e.into());
                }
                _ => {}
            }
            // Periodically flush the buffer to ensure logs are displayed in a timely manner,
            // especially if logs are sparse. Adjust interval as needed.
            // For now, relying on BufWriter's internal buffering and flush on drop/close.
            // Or, could explicitly flush after N messages or N seconds.
        }
        // Ensure all buffered output is written before exiting the follow mode.
        if let Err(e) = buffered_stdout.flush() {
            eprintln!("Error flushing stdout at end of log stream: {}", e);
        }

        Ok("Log streaming finished.".to_string())
    } else {
        // Call the REST API endpoint
        // Step 1: Fetch container ID by calling your server's HTTP GET /v1/containers/:namespace/:name
        let container_id = fetch_container_id_from_api(&namespace, &name).await?;

        // Step 2: Run the local SSH command to stream log content.
        //         This uses the streaming `stream_ssh_command_ts`.
        let cmd = vec![
            "cat".to_string(),
            "$HOME/.logs/nebu_container.log".to_string(),
        ];

        nebulous::ssh::exec::stream_ssh_command_ts(
            &format!("container-{}", container_id),
            cmd,
            false,        // Not interactive for cat
            false,        // No TTY needed for cat
            Some("root"), // TODO: need to fetch from the API
        )?;

        // Output was streamed directly by stream_ssh_command_ts.
        // println!("{}", output);
        Ok("Log content displayed.".to_string()) // Return a success message
    }
}

/// Helper function: calls GET /v1/containers/<namespace>/<name>
/// and returns the container's `.metadata.id`.
async fn fetch_container_id_from_api(
    namespace: &str,
    name: &str,
) -> Result<String, Box<dyn StdError>> {
    let config = ClientConfig::read()?;
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

/// Minimal struct matching the server's "Container" JSON shape.
/// We only need the `metadata.id` field for this flow.
#[derive(Deserialize)]
struct V1Container {
    metadata: V1ResourceMeta,
}

/// Minimal struct for container's metadata (includes ID).
#[derive(Deserialize)]
struct V1ResourceMeta {
    pub id: String,
}
