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
pub struct TailscaleConfig {
    pub api_key: String,
    pub tailnet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadscaleConfig {
    pub api_key: String,
    pub login_server: String,
    pub organization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnConfig {
    pub provider: VpnProvider,
    pub tailscale: Option<TailscaleConfig>,
    pub headscale: Option<HeadscaleConfig>,
}

impl VpnConfig {
    /// Create a new VPN configuration for Tailscale
    pub fn tailscale(api_key: String, tailnet: String) -> Self {
        Self {
            provider: VpnProvider::Tailscale,
            tailscale: Some(TailscaleConfig { api_key, tailnet }),
            headscale: None,
        }
    }

    /// Create a new VPN configuration for Headscale
    pub fn headscale(api_key: String, login_server: String, organization: Option<String>) -> Self {
        Self {
            provider: VpnProvider::Headscale,
            tailscale: None,
            headscale: Some(HeadscaleConfig { api_key, login_server, organization }),
        }
    }

    /// Validate that the configuration is complete for the selected provider
    pub fn validate(&self) -> Result<(), String> {
        match self.provider {
            VpnProvider::Tailscale => {
                if self.tailscale.is_none() {
                    return Err("Tailscale configuration is required for Tailscale provider".to_string());
                }
                let tailscale_config = self.tailscale.as_ref().unwrap();
                if tailscale_config.api_key.is_empty() {
                    return Err("Tailscale API key cannot be empty".to_string());
                }
                if tailscale_config.tailnet.is_empty() {
                    return Err("Tailscale tailnet cannot be empty".to_string());
                }
            }
            VpnProvider::Headscale => {
                if self.headscale.is_none() {
                    return Err("Headscale configuration is required for Headscale provider".to_string());
                }
                let headscale_config = self.headscale.as_ref().unwrap();
                if headscale_config.api_key.is_empty() {
                    return Err("Headscale API key cannot be empty".to_string());
                }
                if headscale_config.login_server.is_empty() {
                    return Err("Headscale login server cannot be empty".to_string());
                }
            }
        }
        Ok(())
    }

    /// Validate environment variables for the specified provider
    pub fn validate_env_for_provider(provider: &VpnProvider) -> Result<(), String> {
        use crate::config::SERVER_CONFIG;
        
        match provider {
            VpnProvider::Tailscale => {
                let api_key = SERVER_CONFIG.vpn_api_key.as_ref();
                let tailnet = SERVER_CONFIG.vpn_tailnet.as_ref();
                
                if api_key.is_none() {
                    return Err("Tailscale requires VPN_API_KEY environment variable".to_string());
                }
                if tailnet.is_none() {
                    return Err("Tailscale requires VPN_TAILNET environment variable".to_string());
                }
            }
            VpnProvider::Headscale => {
                let api_key = SERVER_CONFIG.vpn_api_key.as_ref();
                let login_server = SERVER_CONFIG.vpn_login_server.as_ref();
                
                if api_key.is_none() {
                    return Err("Headscale requires VPN_API_KEY environment variable".to_string());
                }
                if login_server.is_none() {
                    return Err("Headscale requires VPN_LOGIN_SERVER environment variable".to_string());
                }
            }
        }
        Ok(())
    }
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
    use crate::config::SERVER_CONFIG;
    
    let provider = match SERVER_CONFIG.vpn_provider.as_deref() {
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

    // Validate environment variables for the selected provider
    VpnConfig::validate_env_for_provider(&provider)
        .map_err(|e| format!("Environment validation failed: {}", e))?;

    let config = match provider {
        VpnProvider::Tailscale => {
            let api_key = SERVER_CONFIG.vpn_api_key.clone()
                .unwrap(); // Safe because we validated above
            
            let tailnet = SERVER_CONFIG.vpn_tailnet.clone()
                .unwrap(); // Safe because we validated above

            VpnConfig::tailscale(api_key, tailnet)
        }
        VpnProvider::Headscale => {
            let api_key = SERVER_CONFIG.vpn_api_key.clone()
                .unwrap(); // Safe because we validated above
            
            let login_server = SERVER_CONFIG.vpn_login_server.clone()
                .unwrap(); // Safe because we validated above

            VpnConfig::headscale(api_key, login_server, SERVER_CONFIG.vpn_organization.clone())
        }
    };

    // Validate the configuration
    config.validate()
        .map_err(|e| format!("VPN configuration validation failed: {}", e))?;

    debug!("Initializing VPN client with provider: {:?}", provider);
    init_vpn_client(config).await
} 