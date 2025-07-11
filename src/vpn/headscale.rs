use super::{VpnAuthKey, VpnClient, VpnConfig, VpnDevice, VpnKeyCapabilities};
use async_trait::async_trait;
use std::net::IpAddr;
use std::str::FromStr;
use headscale_client::HeadscaleClient;
use std::error::Error;
use std::fmt;
use tracing::{debug, error, info};

#[derive(Debug)]
pub struct HeadscaleError {
    message: String,
}

impl HeadscaleError {
    fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

impl fmt::Display for HeadscaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for HeadscaleError {}

pub struct HeadscaleVpnClient {
    client: HeadscaleClient,
    config: VpnConfig,
}

#[async_trait]
impl VpnClient for HeadscaleVpnClient {
    async fn new(config: VpnConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Initializing Headscale VPN client");
        
        let headscale_config = config.headscale.clone()
            .ok_or_else(|| HeadscaleError::new("Headscale configuration is required but not provided"))?;

        let client = HeadscaleClient::new(headscale_config.login_server, headscale_config.api_key)
            .map_err(|e| HeadscaleError::new(&format!("Failed to create Headscale client: {}", e)))?;

        info!("Headscale VPN client initialized successfully");
        Ok(Self { client, config })
    }

    async fn get_device_ip(&self, hostname: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        debug!("[Headscale] Getting device IP for hostname: {}", hostname);
        
        let device = self.client.find_device_by_name(hostname).await?
            .ok_or_else(|| {
                error!("[Headscale] Device with hostname '{}' not found.", hostname);
                format!("No Headscale device found with hostname '{}'", hostname)
            })?;
        
        let ipv4 = device.ip_addresses
            .iter()
            .find(|s| IpAddr::from_str(s).map_or(false, |ip_addr| ip_addr.is_ipv4()))
            .ok_or_else(|| {
                error!(
                    "[Headscale] No IPv4 address found for device '{}'. Addresses found: {:?}",
                    hostname, device.ip_addresses
                );
                format!("No IPv4 address found for Headscale device '{}'", hostname)
            })?;

        debug!("[Headscale] Found IPv4 '{}' for device '{}'", ipv4, hostname);
        Ok(ipv4.to_string())
    }

    async fn get_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        debug!("[Headscale] Getting device by name: {}", name);

        let device_opt = self.client.find_device_by_name(name).await?;
        
        if let Some(device) = device_opt {
            Ok(Some(VpnDevice {
                name: Some(device.name),
                hostname: Some(device.given_name.unwrap_or_default()),
                addresses: Some(device.ip_addresses),
                tags: None, // Headscale doesn't have tags in the same way
                created: device.last_seen.map(|dt| dt.to_rfc3339()),
            }))
        } else {
            Ok(None)
        }
    }

    async fn remove_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        debug!("[Headscale] Removing device by name: {}", name);
        
        let device = self.get_device_by_name(name).await?;
        
        if let Some(_) = device {
            self.client.remove_device(name)
                .await
                .map_err(|e| HeadscaleError::new(&format!("Failed to delete device {}: {}", name, e)))?;
            
            info!("Successfully removed device: {}", name);
        }
        
        Ok(device)
    }

    async fn create_auth_key(&self, description: &str, capabilities: Option<VpnKeyCapabilities>) -> Result<VpnAuthKey, Box<dyn std::error::Error + Send + Sync>> {
        debug!("[Headscale] Creating auth key with description: {}", description);
        
        let key_result = self.client.create_preauth_key(
            "default", // Use default user
            "24h",     // 24 hour expiration
            false,     // Not reusable
            true,      // Ephemeral
        )
        .await
        .map_err(|e| HeadscaleError::new(&format!("Failed to create auth key: {}", e)))?;

        let key = key_result;
        
        Ok(VpnAuthKey {
            key,
            description: Some(description.to_string()),
            created: Some(chrono::Utc::now().to_rfc3339()),
            expires: Some((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()),
            capabilities: capabilities,
        })
    }
}