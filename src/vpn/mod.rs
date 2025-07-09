use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

pub mod headscale;
pub mod tailscale;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VpnProvider {
    Tailscale,
    Headscale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnConfig {
    pub provider: VpnProvider,
    pub api_key: Option<String>,
    pub tailnet: Option<String>,
    pub login_server: Option<String>,
    pub organization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnDevice {
    pub name: Option<String>,
    pub hostname: Option<String>,
    pub addresses: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub created: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnAuthKey {
    pub key: String,
    pub description: Option<String>,
    pub created: Option<String>,
    pub expires: Option<String>,
    pub capabilities: Option<VpnKeyCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnKeyCapabilities {
    pub devices: Option<VpnDeviceCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnDeviceCapabilities {
    pub create: Option<VpnCreateOpts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnCreateOpts {
    pub reusable: Option<bool>,
    pub ephemeral: Option<bool>,
    pub preauthorized: Option<bool>,
    pub tags: Option<Vec<String>>,
}

#[async_trait]
pub trait VpnClient: Send + Sync {
    async fn new(config: VpnConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        Self: Sized;

    async fn get_device_ip(&self, hostname: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn get_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>>;

    async fn remove_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>>;

    async fn create_auth_key(&self, description: &str, capabilities: Option<VpnKeyCapabilities>) -> Result<VpnAuthKey, Box<dyn std::error::Error + Send + Sync>>;

    fn get_device_name(&self, container_id: &str) -> String {
        format!("container-{}", container_id)
    }

}

pub async fn create_vpn_client(config: VpnConfig) -> Result<Box<dyn VpnClient>, Box<dyn std::error::Error + Send + Sync>> {
    match config.provider {
        VpnProvider::Tailscale => {
            let client = tailscale::TailscaleVpnClient::new(config).await?;
            Ok(Box::new(client))
        }
        VpnProvider::Headscale => {
            let client = headscale::HeadscaleVpnClient::new(config).await?;
            Ok(Box::new(client))
        }
    }
}

pub static mut VPN_CLIENT: Option<Box<dyn VpnClient>> = None;

pub async fn init_vpn_client(config: VpnConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = create_vpn_client(config).await?;
    unsafe {
        VPN_CLIENT = Some(client);
    }
    Ok(())
}

#[allow(static_mut_refs)]
pub fn get_vpn_client() -> Result<&'static dyn VpnClient, Box<dyn std::error::Error + Send + Sync>> {
    unsafe {
        VPN_CLIENT
            .as_ref()
            .map(|client| client.as_ref())
            .ok_or_else(|| "VPN client not initialized".into())
    }
}

pub async fn get_ip_for_device_hostname(hostname: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = get_vpn_client()?;
    client.get_device_ip(hostname).await
}

pub fn get_device_name_for_container(container_id: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = get_vpn_client()?;
    Ok(client.get_device_name(container_id))
}

pub async fn init_vpn_from_config() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::config::CONFIG;
    
    let provider = match CONFIG.vpn_provider.as_deref() {
        Some("headscale") | Some("Headscale") => VpnProvider::Headscale,
        Some("tailscale") | Some("Tailscale") => VpnProvider::Tailscale,
        Some(_) => {
            // Default to Tailscale for any other value
            VpnProvider::Tailscale
        }
        None => {
            // Default to Tailscale for backward compatibility
            VpnProvider::Tailscale
        }
    };

    let config = VpnConfig {
        provider: provider.clone(),
        api_key: CONFIG.vpn_api_key.clone().or_else(|| CONFIG.tailscale_api_key.clone()),
        tailnet: CONFIG.vpn_tailnet.clone().or_else(|| CONFIG.tailscale_tailnet.clone()),
        login_server: CONFIG.vpn_login_server.clone(),
        organization: CONFIG.vpn_organization.clone(),
    };

    debug!("Initializing VPN client with provider: {:?}", provider);
    init_vpn_client(config).await
} 