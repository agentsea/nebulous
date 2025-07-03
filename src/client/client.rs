use crate::config::ClientConfig;
use crate::models::V1StreamData;
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerRequest, V1ContainerSearch, V1Containers, V1UpdateContainer,
};
use crate::resources::v1::processors::models::{
    V1Processor, V1ProcessorRequest, V1ProcessorScaleRequest, V1Processors, V1UpdateProcessor,
};
use crate::resources::v1::secrets::models::{V1Secret, V1SecretRequest, V1Secrets};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;

#[derive(Debug)]
pub struct NebulousClient {
    pub http_client: HttpClient,
    pub base_url: String,
    pub api_key: String,
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
        let config = ClientConfig::read()?;
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
    ) -> Result<V1Container, Box<dyn Error>> {
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
            let typed: V1Container = serde_json::from_value(container)?;
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
    ) -> Result<V1Secret, Box<dyn Error>> {
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
            let typed: V1Secret = serde_json::from_value(raw)?;
            Ok(typed)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to create secret: {}", error_text).into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // GET METHODS (with optional namespace/name)
    // ─────────────────────────────────────────────────────────────────────────────

    /// Gets a specific container by namespace and name, returning a typed `V1Container`.
    /// `namespace` cannot be empty
    /// `name` cannot be empty.
    pub async fn get_container(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<V1Container, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/containers/{}/{}", self.base_url, namespace, name);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let container = response.json::<V1Container>().await?;
            Ok(container)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to get container {}/{}: {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Lists all containers, returning a typed `V1Containers`.
    pub async fn get_containers(&self) -> Result<V1Containers, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/containers", self.base_url);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let containers = response.json::<V1Containers>().await?;
            Ok(containers)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to list containers: {}", error_text).into())
        }
    }

    /// Gets a specific secret by namespace and name, returning a typed `V1Secret`.
    /// `name` cannot be empty.
    pub async fn get_secret(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<V1Secret, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/secrets/{}/{}", self.base_url, namespace, name);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let secret = response.json::<V1Secret>().await?;
            Ok(secret)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to get secret {}/{}: {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Lists all secrets, returning a typed `V1Secrets`.
    pub async fn get_secrets(&self) -> Result<V1Secrets, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/secrets", self.base_url);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let secrets = response.json::<V1Secrets>().await?;
            Ok(secrets)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to list secrets: {}", error_text).into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // DELETE METHODS
    // ─────────────────────────────────────────────────────────────────────────────

    /// Deletes a container by `/:namespace/:name`.  
    pub async fn delete_container(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<(), Box<dyn Error>> {
        let url = format!("{}/v1/containers/{}/{}", self.base_url, namespace, name);

        let response = self
            .http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            println!("Container '{}/{}' successfully deleted", namespace, name);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to delete container '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Deletes a secret by `/:namespace/:name`.  
    pub async fn delete_secret(&self, name: &str, namespace: &str) -> Result<(), Box<dyn Error>> {
        let url = format!("{}/v1/secrets/{}/{}", self.base_url, namespace, name);

        let response = self
            .http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            println!("Secret '{}/{}' successfully deleted", namespace, name);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to delete secret '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // PATCH METHODS
    // ─────────────────────────────────────────────────────────────────────────────

    /// PATCH a container by `/:namespace/:name`.  
    pub async fn patch_container(
        &self,
        name: &str,
        namespace: &str,
        update_request: &V1UpdateContainer,
    ) -> Result<V1Container, Box<dyn Error>> {
        let url = format!("{}/v1/containers/{}/{}", self.base_url, namespace, name);

        let response = self
            .http_client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(update_request)
            .send()
            .await?;

        if response.status().is_success() {
            let container = response.json::<V1Container>().await?;
            Ok(container)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to patch container '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // SEARCH METHODS
    // ─────────────────────────────────────────────────────────────────────────────

    pub async fn search_containers(
        &self,
        search_request: &V1ContainerSearch,
    ) -> Result<V1Containers, Box<dyn Error>> {
        let url = format!("{}/v1/containers/search", self.base_url);
        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(search_request)
            .send()
            .await?;

        if response.status().is_success() {
            let containers = response.json::<V1Containers>().await?;
            Ok(containers)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to search containers: {}", error_text).into())
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // PROCESSOR METHODS
    // ─────────────────────────────────────────────────────────────────────────────

    /// Creates a processor using the Nebulous API.
    pub async fn create_processor(
        &self,
        processor_request: &V1ProcessorRequest,
    ) -> Result<V1Processor, Box<dyn Error>> {
        let url = format!("{}/v1/processors", self.base_url);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(processor_request)
            .send()
            .await?;

        if response.status().is_success() {
            let raw = response.json::<Value>().await?;
            let typed: V1Processor = serde_json::from_value(raw)?;
            Ok(typed)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to create processor: {}", error_text).into())
        }
    }

    /// Lists all processors, returning a typed `V1Processors`.
    pub async fn list_processors(&self) -> Result<V1Processors, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/processors", self.base_url);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let processors = response.json::<V1Processors>().await?;
            Ok(processors)
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to list processors: {}", error_text).into())
        }
    }

    /// Gets a specific processor by namespace and name, returning a typed `V1Processor`.
    /// `name` cannot be empty.
    pub async fn get_processor(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<V1Processor, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/processors/{}/{}", self.base_url, namespace, name);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            let processor = response.json::<V1Processor>().await?;
            Ok(processor)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to get processor {}/{}: {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Deletes a processor by `/:namespace/:name`.
    pub async fn delete_processor(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<(), Box<dyn Error>> {
        let url = format!("{}/v1/processors/{}/{}", self.base_url, namespace, name);

        let response = self
            .http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if response.status().is_success() {
            println!("Processor '{}/{}' successfully deleted", namespace, name);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to delete processor '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Updates (PATCH) a processor by `/:namespace/:name`.
    pub async fn update_processor(
        &self,
        name: &str,
        namespace: &str,
        update_request: &V1UpdateProcessor,
    ) -> Result<V1Processor, Box<dyn Error>> {
        let url = format!("{}/v1/processors/{}/{}", self.base_url, namespace, name);

        let response = self
            .http_client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(update_request)
            .send()
            .await?;

        if response.status().is_success() {
            let processor = response.json::<V1Processor>().await?;
            Ok(processor)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to patch processor '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Scales a processor by `/:namespace/:name`.
    pub async fn scale_processor(
        &self,
        name: &str,
        namespace: &str,
        scale_request: &V1ProcessorScaleRequest,
    ) -> Result<V1Processor, Box<dyn Error>> {
        let url = format!(
            "{}/v1/processors/{}/{}/scale",
            self.base_url, namespace, name
        );

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(scale_request)
            .send()
            .await?;

        if response.status().is_success() {
            let processor = response.json::<V1Processor>().await?;
            Ok(processor)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to scale processor '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }

    /// Sends a message to a processor's stream. Returns the raw response Value.
    /// If `stream_data.wait` is true, it will block until a response is received or timeout.
    pub async fn send_processor_message(
        &self,
        name: &str,
        namespace: &str,
        stream_data: &V1StreamData,
    ) -> Result<Value, Box<dyn Error>> {
        let url = format!(
            "{}/v1/processors/{}/{}/messages",
            self.base_url, namespace, name
        );

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(stream_data)
            .send()
            .await?;

        if response.status().is_success() {
            let response_json = response.json::<Value>().await?;
            Ok(response_json)
        } else {
            let error_text = response.text().await?;
            Err(format!(
                "Failed to send message to processor '{}/{}': {}",
                namespace, name, error_text
            )
            .into())
        }
    }
}
