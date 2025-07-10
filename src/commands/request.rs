use serde_json::Value;
use nebulous::config::ClientConfig;

fn prepare_request(
    path: &str,
    method: reqwest::Method,
) -> Result<reqwest::RequestBuilder, Box<dyn std::error::Error>> {
    let config = ClientConfig::read()?;
    let current_server = config
        .get_current_server_config()
        .ok_or("Failed to get current server configuration")?;
    let server = current_server
        .server
        .as_deref()
        .ok_or("Server URL is missing in the configuration")?;
    let api_key = current_server
        .api_key
        .as_deref()
        .ok_or("API key is missing in the configuration")?;

    let bearer_token = format!("Bearer {}", api_key);
    let url = format!("{}{}", server, path.trim());

    let client = reqwest::Client::new();
    Ok(client
        .request(method, url)
        .header("Authorization", &bearer_token))
}

pub async fn server_request(
    path: &str,
    method: reqwest::Method,
) -> Result<reqwest::Response, Box<dyn std::error::Error>> {
    server_request_with_payload::<()>(path, method, None).await
}

pub async fn server_request_with_payload<T>(
    path: &str,
    method: reqwest::Method,
    payload: Option<T>,
) -> Result<reqwest::Response, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
{
    match prepare_request(path, method) {
        Ok(req) => {
            let response = match payload {
                Some(data) => req.json(&data).send().await?,
                None => req.send().await?,
            };

            if response.status().is_success() {
                Ok(response)
            } else {
                Err(format!("Request to server failed: {}", response.status()).into())
            }
        }
        Err(e) => Err(format!("Error preparing request: {}", e).into()),
    }
}
