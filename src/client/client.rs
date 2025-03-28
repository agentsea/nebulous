use crate::config::GlobalConfig;
use crate::models::{V1ContainerRequest, V1SecretRequest};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;

#[derive(Debug)]
pub struct NebulousClient {
    http_client: HttpClient,
    base_url: String,
    api_key: String,
}

/// A simple DTO for container responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContainerResponse {
    pub metadata: ContainerMetadata,
}

/// The metadata part of the container response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContainerMetadata {
    pub id: Option<String>,
    pub name: Option<String>,
}

/// A simple DTO for secret responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretResponse {
    pub metadata: SecretMetadata,
}

/// The metadata part of the secret response.
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub id: Option<String>,
    pub name: Option<String>,
}

impl NebulousClient {
    /// Creates a new NebulousClient by reading from the global config.
    /// You could also pass server and api key directly if preferred.
    pub fn new_from_config() -> Result<Self, Box<dyn Error>> {
        let config = GlobalConfig::read()?;
        let current_server = config
            .get_current_server_config()
            .ok_or("No current server config found")?;
        let server_url = current_server
            .server
            .clone()
            .ok_or("Server URL not found in config")?;
        let api = current_server
            .api_key
            .clone()
            .ok_or("API key not found in config")?;

        Ok(Self {
            http_client: HttpClient::new(),
            base_url: server_url,
            api_key: api,
        })
    }

    /// Convenience constructor if you already have the values on hand.
    pub fn new<S: Into<String>>(server: S, api_key: S) -> Self {
        Self {
            http_client: HttpClient::new(),
            base_url: server.into(),
            api_key: api_key.into(),
        }
    }

    /// Creates a container using the Nebulous API.
    pub async fn create_container(
        &self,
        container_request: &V1ContainerRequest,
    ) -> Result<ContainerResponse, Box<dyn Error>> {
        let url = format!("{}/v1/containers", self.base_url);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(container_request)
            .send()
            .await?;

        if response.status().is_success() {
            let container: Value = response.json().await?;
            // If you just need the raw JSON, return it directly.
            // Here, we map it into a typed struct.
            // Adjust as needed for your actual response shape.
            let typed: ContainerResponse = serde_json::from_value(container)?;
            Ok(typed)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to create container: {}", error_text).into())
        }
    }

    /// Creates a secret using the Nebulous API.
    pub async fn create_secret(
        &self,
        secret_request: &V1SecretRequest,
    ) -> Result<SecretResponse, Box<dyn Error>> {
        let url = format!("{}/v1/secrets", self.base_url);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(secret_request)
            .send()
            .await?;

        if response.status().is_success() {
            let raw = response.json::<Value>().await?;
            let typed: SecretResponse = serde_json::from_value(raw)?;
            Ok(typed)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to create secret: {}", error_text).into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // GET METHODS (with optional namespace/name)
    // ─────────────────────────────────────────────────────────────────────────────

    /// Gets JSON data about containers.  
    /// - If `name` is provided, fetches container at `/:namespace/:name`.  
    /// - Otherwise, lists all containers.  
    /// `namespace` defaults to `"default"` if not provided.
    pub async fn get_containers(
        &self,
        name: Option<&str>,
        namespace: Option<&str>,
    ) -> Result<Value, Box<dyn Error>> {
        if let Some(real_name) = name {
            // Single container
            let real_ns = namespace.unwrap_or("default");
            let url = format!("{}/v1/containers/{}/{}", self.base_url, real_ns, real_name);
            let response = self
                .http_client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await?;

            if response.status().is_success() {
                let container_json = response.json::<Value>().await?;
                Ok(container_json)
            } else {
                let error_text = response.text().await?;
                Err(format!(
                    "Failed to get container {}/{}: {}",
                    real_ns, real_name, error_text
                )
                .into())
            }
        } else {
            // List containers
            let url = format!("{}/v1/containers", self.base_url);
            let response = self
                .http_client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await?;

            if response.status().is_success() {
                let containers_json = response.json::<Value>().await?;
                Ok(containers_json)
            } else {
                let error_text = response.text().await?;
                Err(format!("Failed to list containers: {}", error_text).into())
            }
        }
    }

    /// Gets JSON data about secrets.  
    /// - If `name` is provided, fetches secret at `/:namespace/:name`.  
    /// - Otherwise, lists all secrets.  
    /// `namespace` defaults to `"default"` if not provided.
    pub async fn get_secrets(
        &self,
        name: Option<&str>,
        namespace: Option<&str>,
    ) -> Result<Value, Box<dyn Error>> {
        if let Some(real_name) = name {
            // Single secret
            let real_ns = namespace.unwrap_or("default");
            let url = format!("{}/v1/secrets/{}/{}", self.base_url, real_ns, real_name);
            let response = self
                .http_client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await?;

            if response.status().is_success() {
                let secret_json = response.json::<Value>().await?;
                Ok(secret_json)
            } else {
                let error_text = response.text().await?;
                Err(format!(
                    "Failed to get secret {}/{}: {}",
                    real_ns, real_name, error_text
                )
                .into())
            }
        } else {
            // List secrets
            let url = format!("{}/v1/secrets", self.base_url);
            let response = self
                .http_client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await?;

            if response.status().is_success() {
                let secrets_json = response.json::<Value>().await?;
                Ok(secrets_json)
            } else {
                let error_text = response.text().await?;
                Err(format!("Failed to list secrets: {}", error_text).into())
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // DELETE METHODS
    // ─────────────────────────────────────────────────────────────────────────────

    /// Deletes a container by `/:namespace/:name`.  
    /// Defaults to `"default"` namespace if none is provided.
    pub async fn delete_container(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        let ns = namespace.unwrap_or("default");
        let url = format!("{}/v1/containers/{}/{}", self.base_url, ns, name);

        let response = self
            .http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            println!("Container '{}/{}' successfully deleted", ns, name);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to delete container '{}/{}': {}",
                ns, name, error_text
            )
            .into())
        }
    }

    /// Deletes a secret by `/:namespace/:name`.  
    /// Defaults to `"default"` namespace if none is provided.
    pub async fn delete_secret(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        let ns = namespace.unwrap_or("default");
        let url = format!("{}/v1/secrets/{}/{}", self.base_url, ns, name);

        let response = self
            .http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            println!("Secret '{}/{}' successfully deleted", ns, name);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to delete secret '{}/{}': {}", ns, name, error_text).into())
        }
    }
}
