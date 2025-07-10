use crate::agent::agent::create_agent_key;
use crate::agent::aws::create_s3_scoped_user;
use crate::config::{ClientConfig, SERVER_CONFIG};
use crate::entities::containers;
use crate::handlers::v1::volumes::ensure_volume;
use crate::models::{V1CreateAgentKeyRequest, V1UserProfile};
use crate::orign::get_orign_server;
use crate::query::Query;
use crate::resources::v1::containers::models::{V1Container, V1ContainerRequest};
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use tailscale_client::{Capabilities, CreateAuthKeyRequest, CreateOpts, Devices, TailscaleClient};
use tracing::{debug, error, info};

/// Enum for container status
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub enum ContainerStatus {
    Defined,
    Restarting,
    Exited,
    Paused,
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
    Invalid,
    Creating,
    Created,
    Queued,
}

impl ContainerStatus {
    /// Returns a list of all statuses considered inactive (terminal).
    pub fn inactive() -> Vec<Self> {
        vec![
            ContainerStatus::Completed,
            ContainerStatus::Failed,
            ContainerStatus::Stopped,
            ContainerStatus::Exited,
            ContainerStatus::Invalid,
        ]
    }

    /// Returns a list of all statuses considered active (non-terminal).
    pub fn active() -> Vec<Self> {
        vec![
            ContainerStatus::Defined,
            ContainerStatus::Restarting,
            ContainerStatus::Paused,
            ContainerStatus::Pending,
            ContainerStatus::Running,
            ContainerStatus::Creating,
            ContainerStatus::Created,
            ContainerStatus::Queued,
        ]
    }

    pub fn needs_start(&self) -> bool {
        matches!(
            self,
            ContainerStatus::Defined
                | ContainerStatus::Paused
                | ContainerStatus::Pending
                | ContainerStatus::Queued
        )
    }

    pub fn needs_watch(&self) -> bool {
        matches!(
            self,
            ContainerStatus::Running
                | ContainerStatus::Creating
                | ContainerStatus::Created
                | ContainerStatus::Restarting
        )
    }

    /// Returns true if the container is in a terminal (inactive) state.
    pub fn is_inactive(&self) -> bool {
        matches!(
            self,
            ContainerStatus::Completed
                | ContainerStatus::Failed
                | ContainerStatus::Stopped
                | ContainerStatus::Exited
                | ContainerStatus::Invalid
        )
    }

    /// Returns true if the container is in an active (non-terminal) state.
    pub fn is_active(&self) -> bool {
        !self.is_inactive()
    }
}

impl fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContainerStatus::Defined => write!(f, "defined"),
            ContainerStatus::Restarting => write!(f, "restarting"),
            ContainerStatus::Exited => write!(f, "exited"),
            ContainerStatus::Paused => write!(f, "paused"),
            ContainerStatus::Pending => write!(f, "pending"),
            ContainerStatus::Running => write!(f, "running"),
            ContainerStatus::Completed => write!(f, "completed"),
            ContainerStatus::Failed => write!(f, "failed"),
            ContainerStatus::Stopped => write!(f, "stopped"),
            ContainerStatus::Invalid => write!(f, "invalid"),
            ContainerStatus::Creating => write!(f, "creating"),
            ContainerStatus::Created => write!(f, "created"),
            ContainerStatus::Queued => write!(f, "queued"),
        }
    }
}

impl FromStr for ContainerStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "defined" => Ok(ContainerStatus::Defined),
            "restarting" => Ok(ContainerStatus::Restarting),
            "exited" => Ok(ContainerStatus::Exited),
            "paused" => Ok(ContainerStatus::Paused),
            "pending" => Ok(ContainerStatus::Pending),
            "running" => Ok(ContainerStatus::Running),
            "completed" => Ok(ContainerStatus::Completed),
            "failed" => Ok(ContainerStatus::Failed),
            "stopped" => Ok(ContainerStatus::Stopped),
            "creating" => Ok(ContainerStatus::Creating),
            "created" => Ok(ContainerStatus::Created),
            "queued" => Ok(ContainerStatus::Queued),
            _ => Err(format!("Unknown container status: {}", s)),
        }
    }
}

pub trait ContainerPlatform {
    async fn declare(
        &self,
        config: &V1ContainerRequest,
        db: &DatabaseConnection,
        user_profile: &V1UserProfile,
        owner_id: &str,
        namespace: &str,
        api_key: Option<String>,
    ) -> Result<V1Container, Box<dyn std::error::Error + Send + Sync>>;

    async fn reconcile(
        &self,
        container: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn exec(
        &self,
        container_id: &str,
        command: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn logs(
        &self,
        container_id: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn accelerator_map(&self) -> HashMap<String, String>;

    // Default implementation for common environment variables
    async fn get_common_env(
        &self,
        model: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
        let config = ClientConfig::read().unwrap();
        let mut env = HashMap::new();

        debug!("Getting agent key");
        let agent_key = match Query::get_agent_key(db, model.id.clone()).await {
            Ok(key) => key,
            Err(e) => {
                error!("Error getting agent key: {:?}", e);
                return Err(e.into());
            }
        };

        let root_volume_uri = format!("s3://{}/data", SERVER_CONFIG.bucket_name);
        let source = format!("{}/{}", root_volume_uri, model.namespace);

        debug!("Ensuring volume: {:?}", source.clone());
        let _ = match ensure_volume(
            db,
            &model.namespace,
            &model.namespace,
            &model.owner,
            &source.clone(),
            &model.created_by.clone().unwrap_or_default(),
            model.labels.clone(),
        )
        .await
        {
            Ok(_) => (),
            Err(e) => {
                error!("Error ensuring volume: {:?}", e);
                return Err(e.into());
            }
        };

        debug!("Creating s3 token");
        let s3_token =
            match create_s3_scoped_user(&SERVER_CONFIG.bucket_name, &model.namespace, &model.id).await {
                Ok(token) => token,
                Err(e) => {
                    error!("Error creating s3 token: {:?}", e);
                    return Err(e.into());
                }
            };

        debug!("Adding RCLONE environment variables");
        // Add RCLONE environment variables
        env.insert("RCLONE_CONFIG_S3REMOTE_TYPE".to_string(), "s3".to_string());
        env.insert(
            "RCLONE_CONFIG_S3REMOTE_PROVIDER".to_string(),
            "AWS".to_string(),
        );
        env.insert(
            "RCLONE_CONFIG_S3REMOTE_ENV_AUTH".to_string(),
            "true".to_string(),
        );
        debug!("Adding AWS credentials");
        debug!("Access key: {}", s3_token.access_key_id);
        debug!("Secret key: {}", s3_token.secret_access_key);
        env.insert("AWS_ACCESS_KEY_ID".to_string(), s3_token.access_key_id);
        env.insert(
            "AWS_SECRET_ACCESS_KEY".to_string(),
            s3_token.secret_access_key,
        );
        env.insert(
            "RCLONE_CONFIG_S3REMOTE_REGION".to_string(),
            SERVER_CONFIG.bucket_region.clone(),
        );
        env.insert("RCLONE_S3_NO_CHECK_BUCKET".to_string(), "true".to_string());
        env.insert("NEBU_API_KEY".to_string(), agent_key.clone().unwrap());
        env.insert("AGENTSEA_API_KEY".to_string(), agent_key.unwrap());

        let orign_server = get_orign_server();
        if let Some(orign_server) = orign_server {
            env.insert("ORIGN_SERVER".to_string(), orign_server);
        }
        env.insert(
            "AGENTSEA_AUTH_SERVER".to_string(),
            SERVER_CONFIG.auth.url.clone(),
        );
        env.insert(
            "NEBULOUS_SERVER".to_string(),
            SERVER_CONFIG.publish_url.clone().unwrap(),
        );

        env.insert("NEBU_NAMESPACE".to_string(), model.namespace.clone());
        env.insert("NEBU_NAME".to_string(), model.name.clone());
        env.insert("NEBU_CONTAINER_ID".to_string(), model.id.clone());
        env.insert("NEBU_DATE".to_string(), chrono::Utc::now().to_rfc3339());
        env.insert("HF_HOME".to_string(), "/nebu/cache/huggingface".to_string());
        env.insert("NAMESPACE_VOLUME_URI".to_string(), source.clone());
        env.insert(
            "NAME_VOLUME_URI".to_string(),
            format!("{}/{}", source.clone(), model.name),
        );
        env.insert("ROOT_VOLUME_URI".to_string(), root_volume_uri);

        match self.get_tailscale_device_key(model).await {
            Ok(key) => {
                env.insert("TS_AUTHKEY".to_string(), key);
            }
            Err(e) => {
                error!(
                    "Failed to get Tailscale device key for container {}: {:?}. Propagating error.",
                    model.id, e
                );
                return Err(e);
            }
        }

        // env.insert(
        //     "RCLONE_CONFIG_S3REMOTE_ACL".to_string(),
        //     "private".to_string(),
        // );

        // Add more common environment variables as needed
        Ok(env)
    }

    async fn get_tailscale_device_name(&self, model: &containers::Model) -> String {
        get_tailscale_device_name(model).await
    }

    async fn get_tailscale_device_ip(
        &self,
        model: &containers::Model,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let client = self.get_tailscale_client().await;
        let name = self.get_tailscale_device_name(model).await;

        let device = client
            .find_device_by_name("-", &name, None)
            .await?
            .ok_or_else(|| format!("No Tailscale device found with name '{}'", name))?;

        let addresses = device
            .addresses
            .as_ref()
            .ok_or_else(|| format!("Tailscale device '{}' has no addresses", name))?;

        let ipv4 = addresses
            .iter()
            .find(|s| IpAddr::from_str(s).map_or(false, |ip_addr| ip_addr.is_ipv4()))
            .ok_or_else(|| format!("No IPv4 address found for Tailscale device '{}'", name))?;

        Ok(ipv4.to_string())
    }

    async fn get_tailscale_client(&self) -> TailscaleClient {
        let tailscale_api_key = SERVER_CONFIG
            .tailscale.as_ref()
            .map(|ts| ts.api_key.clone())
            .expect("Tailscale API key not found in config");
        debug!("Tailscale key: {}", tailscale_api_key);
        TailscaleClient::new(tailscale_api_key)
    }

    async fn get_tailscale_device_key(
        &self,
        model: &containers::Model,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let tailnet = SERVER_CONFIG
            .tailscale.as_ref()
            .map(|ts| ts.tailnet.clone())
            .expect("Tailscale tailnet not found in config");

        debug!("Tailnet: {}", tailnet);

        let client = self.get_tailscale_client().await;
        let name = self.get_tailscale_device_name(model).await;

        debug!("Ensuring no existing Tailscale device for {}", name);
        match client.remove_device_by_name(&tailnet, &name, None).await {
            Ok(Some(_deleted_device)) => {
                debug!(
                    "Successfully removed existing Tailscale device for {}",
                    name
                );
            }
            Ok(None) => {
                debug!(
                    "No existing Tailscale device found for {}, proceeding.",
                    name
                );
            }
            Err(e) => {
                error!(
                    "Error removing existing Tailscale device for {}: {}. Proceeding with key creation attempt.",
                    name, e
                );
            }
        }

        let request_body = CreateAuthKeyRequest {
            description: Some(format!("Nebu ephemeral key for container {}", model.id)),
            expirySeconds: None,
            capabilities: Capabilities {
                devices: Devices {
                    create: Some(CreateOpts {
                        reusable: Some(false),
                        ephemeral: Some(true),
                        preauthorized: Some(true),
                        tags: Some(vec!["tag:container".to_string()]),
                    }),
                },
            },
        };

        let response = match client.create_auth_key(&tailnet, true, &request_body).await {
            Ok(resp) => resp,
            Err(e) => {
                return Err(format!("Failed to create Tailscale auth key: {}", e).into());
            }
        };

        debug!("CreateAuthKeyResponse: {:?}", response);
        let key = response.key.ok_or_else(|| {
            "Server did not return a value in `key` from Tailscale API after key creation"
                .to_string()
        })?;

        debug!("Tailscale auth key generated: {}", key);
        Ok(key)
    }

    async fn get_agent_key(
        &self,
        user_profile: &V1UserProfile,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let config = ClientConfig::read().unwrap();

        debug!("[DEBUG] get_agent_key: Entering function");
        debug!("[DEBUG] get_agent_key: user_profile = {:?}", user_profile);

        // Check if token exists and log it
        if user_profile.token.is_none() {
            error!("[ERROR] get_agent_key: user_profile.token is None!");
            return Err("User profile does not have a token".into());
        }

        debug!("[DEBUG] get_agent_key: Creating agent key request");
        let create_agent_key_request = V1CreateAgentKeyRequest {
            agent_id: "nebu".to_string(),
            name: format!("nebu-{}", uuid::Uuid::new_v4()),
            duration: 604800,
        };

        debug!("[DEBUG] get_agent_key: Getting server config");
        let server_config = match config.get_current_server_config() {
            Some(cfg) => cfg,
            None => {
                error!("[ERROR] get_agent_key: No current server config found");
                return Err("No current server configuration available".into());
            }
        };

        let auth_server = match &server_config.auth_server {
            Some(server) => {
                debug!("[DEBUG] get_agent_key: Using auth_server: {}", server);
                server
            }
            None => {
                error!("[ERROR] get_agent_key: No auth_server in server config");
                return Err("No auth server specified in configuration".into());
            }
        };

        debug!("[DEBUG] get_agent_key: Calling create_agent_key");
        let agent_key = match create_agent_key(
            auth_server,
            &user_profile.token.clone().unwrap(),
            create_agent_key_request,
        )
        .await
        {
            Ok(key) => {
                debug!("[DEBUG] get_agent_key: Successfully created agent key");
                key
            }
            Err(e) => {
                error!("[ERROR] get_agent_key: Failed to create agent key: {:?}", e);
                return Err(format!("Failed to create agent key: {:?}", e).into());
            }
        };

        if agent_key.key.is_none() {
            error!("[ERROR] get_agent_key: agent_key.key is None!");
            return Err("Agent key returned from server is None".into());
        }

        debug!("[DEBUG] get_agent_key: Successfully obtained agent key");
        Ok(agent_key.key.unwrap())
    }

    /// Store an agent key as a secret in the database
    async fn store_agent_key_secret(
        &self,
        db: &DatabaseConnection,
        user_profile: &V1UserProfile,
        container_id: &str,
        owner_id: &str,
        key: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::entities::secrets;
        use sea_orm::{EntityTrait, Set};

        debug!(
            "[DEBUG] store_agent_key_secret: Starting for container {}",
            container_id
        );
        debug!("[DEBUG] store_agent_key_secret: owner_id={}", owner_id);
        debug!(
            "[DEBUG] store_agent_key_secret: user_profile = {:?}",
            user_profile
        );

        // TODO: Re-evaluate how agent keys are generated. >>>>>
        // get_agent_key relied on user_profile.token which is no longer available here.
        // For now, we might need a placeholder or a different mechanism.
        // Let's use a temporary placeholder value for now.
        let agent_key = match key {
            Some(key) => key,
            None => self.get_agent_key(user_profile).await?,
        };
        debug!(
            "[DEBUG] store_agent_key_secret: Using temporary agent key: {}",
            agent_key
        );

        debug!("[DEBUG] store_agent_key_secret: Creating new secret model");
        // Create a new secret with the agent key
        let secret = match secrets::Model::new(
            container_id.to_string(),
            format!("agent-key-{}", container_id),
            "container-reconciler".to_string(),
            "container-reconciler".to_string(),
            &agent_key,
            Some(owner_id.to_string()),
            None,
            None,
        ) {
            Ok(s) => {
                debug!("[DEBUG] store_agent_key_secret: Created secret model");
                s
            }
            Err(e) => {
                error!(
                    "[ERROR] store_agent_key_secret: Failed to create secret model: {}",
                    e
                );
                return Err(e.into());
            }
        };

        let namespace = secret.namespace.clone();
        let name = secret.name.clone();

        let full_name = format!("{namespace}/{name}");
        debug!(
            "[DEBUG] store_agent_key_secret: Secret full_name={}",
            full_name
        );

        // Convert to active model for insertion
        let active_model = secrets::ActiveModel {
            id: Set(secret.id),
            name: Set(name),
            namespace: Set(namespace),
            full_name: Set(full_name),
            owner: Set(secret.owner),
            owner_ref: Set(secret.owner_ref),
            encrypted_value: Set(secret.encrypted_value),
            nonce: Set(secret.nonce),
            labels: Set(None),
            created_by: Set(secret.created_by),
            updated_at: Set(secret.updated_at),
            created_at: Set(secret.created_at),
            expires_at: Set(None),
        };

        debug!("[DEBUG] store_agent_key_secret: Inserting secret into database");
        // Insert into database
        match secrets::Entity::insert(active_model).exec(db).await {
            Ok(_) => {
                debug!("[DEBUG] store_agent_key_secret: Successfully inserted secret");
            }
            Err(e) => {
                error!(
                    "[ERROR] store_agent_key_secret: Failed to insert secret: {}",
                    e
                );
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "Failed to store agent key secret: {}",
                    e
                )));
            }
        }

        info!(
            "[RunPod] Stored agent key secret for container {}",
            container_id
        );

        Ok(())
    }
}

pub trait ContainerController {
    async fn reconcile(&self);
}

pub async fn get_tailscale_device_name(model: &containers::Model) -> String {
    format!("container-{}", model.id)
}

/// Fetches the IPv4 address of a device from Tailscale using its hostname.
pub async fn get_ip_for_tailscale_device_hostname(
    device_hostname: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    debug!(
        "[Tailscale] Attempting to fetch IP for hostname: {}",
        device_hostname
    );
    let tailscale_api_key = SERVER_CONFIG
        .tailscale.as_ref()
        .map(|ts| ts.api_key.clone())
        .expect("Tailscale API key not found in config");
    let client = TailscaleClient::new(tailscale_api_key);

    // Use "-" for the default tailnet.
    let device = client
        .find_device_by_name("-", device_hostname, None)
        .await?
        .ok_or_else(|| {
            error!(
                "[Tailscale] Device with hostname '{}' not found.",
                device_hostname
            );
            format!(
                "No Tailscale device found with hostname '{}'",
                device_hostname
            )
        })?;

    debug!(
        "[Tailscale] Found device for hostname '{}': Name in response: {:?}",
        device_hostname,
        device.name.as_deref().unwrap_or("N/A")
    );

    let addresses = device.addresses.as_ref().ok_or_else(|| {
        error!(
            "[Tailscale] Device '{}' has no IP addresses listed.",
            device_hostname
        );
        format!(
            "Tailscale device '{}' has no addresses listed",
            device_hostname
        )
    })?;

    let ipv4 = addresses
        .iter()
        .find(|s| IpAddr::from_str(s).map_or(false, |ip_addr| ip_addr.is_ipv4()))
        .ok_or_else(|| {
            error!(
                "[Tailscale] No IPv4 address found for device '{}'. Addresses found: {:?}",
                device_hostname, addresses
            );
            format!(
                "No IPv4 address found for Tailscale device '{}'",
                device_hostname
            )
        })?;

    debug!(
        "[Tailscale] Found IPv4 '{}' for device '{}'",
        ipv4, device_hostname
    );
    Ok(ipv4.to_string())
}
