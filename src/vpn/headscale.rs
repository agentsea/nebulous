use super::{VpnAuthKey, VpnClient, VpnConfig, VpnDevice, VpnKeyCapabilities};
use async_trait::async_trait;
use std::error::Error;
use std::fmt;
use tracing::{debug, error};

#[derive(Debug)]
pub struct NotImplementedError {
    message: String,
}

impl NotImplementedError {
    fn new(operation: &str) -> Self {
        Self {
            message: format!("Headscale VPN client not implemented yet. Operation: {}", operation),
        }
    }
}

impl fmt::Display for NotImplementedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for NotImplementedError {}

pub struct HeadscaleVpnClient {
    _config: VpnConfig,
}

#[async_trait]
impl VpnClient for HeadscaleVpnClient {
    async fn new(config: VpnConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Initializing Headscale VPN client (placeholder implementation)");
        
        // Validate that we have the required configuration
        if config.login_server.is_none() {
            return Err("Headscale login server is required but not provided".into());
        }
        
        if config.api_key.is_none() {
            return Err("Headscale API key is required but not provided".into());
        }

        Ok(Self { _config: config })
    }

    async fn get_device_ip(&self, hostname: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        error!("[Headscale] get_device_ip not implemented yet for hostname: {}", hostname);
        Err(Box::new(NotImplementedError::new("get_device_ip")))
    }

    async fn get_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        error!("[Headscale] get_device_by_name not implemented yet for name: {}", name);
        Err(Box::new(NotImplementedError::new("get_device_by_name")))
    }

    async fn remove_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        error!("[Headscale] remove_device_by_name not implemented yet for name: {}", name);
        Err(Box::new(NotImplementedError::new("remove_device_by_name")))
    }

    async fn create_auth_key(&self, description: &str, _capabilities: Option<VpnKeyCapabilities>) -> Result<VpnAuthKey, Box<dyn std::error::Error + Send + Sync>> {
        error!("[Headscale] create_auth_key not implemented yet for description: {}", description);
        Err(Box::new(NotImplementedError::new("create_auth_key")))
    }
}