use super::{VpnAuthKey, VpnClient, VpnConfig, VpnDevice, VpnKeyCapabilities};
use async_trait::async_trait;
use std::net::IpAddr;
use std::str::FromStr;
use tailscale_client::{Capabilities, CreateAuthKeyRequest, CreateOpts, Devices, TailscaleClient};
use tracing::{debug, error};

pub struct TailscaleVpnClient {
    client: TailscaleClient,
    config: VpnConfig,
}

#[async_trait]
impl VpnClient for TailscaleVpnClient {
    async fn new(config: VpnConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let api_key = config
            .api_key
            .clone()
            .ok_or_else(|| "Tailscale API key not found in config")?;

        debug!("Initializing Tailscale client with key: {}", api_key);
        let client = TailscaleClient::new(api_key);

        Ok(Self { client, config })
    }

    async fn get_device_ip(&self, hostname: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        debug!("[Tailscale] Attempting to fetch IP for hostname: {}", hostname);

        // Use "-" for the default tailnet
        let device = self
            .client
            .find_device_by_name("-", hostname, None)
            .await?
            .ok_or_else(|| {
                error!("[Tailscale] Device with hostname '{}' not found.", hostname);
                format!("No Tailscale device found with hostname '{}'", hostname)
            })?;

        debug!(
            "[Tailscale] Found device for hostname '{}': Name in response: {:?}",
            hostname,
            device.name.as_deref().unwrap_or("N/A")
        );

        let addresses = device.addresses.as_ref().ok_or_else(|| {
            error!("[Tailscale] Device '{}' has no IP addresses listed.", hostname);
            format!("Tailscale device '{}' has no addresses listed", hostname)
        })?;

        let ipv4 = addresses
            .iter()
            .find(|s| IpAddr::from_str(s).map_or(false, |ip_addr| ip_addr.is_ipv4()))
            .ok_or_else(|| {
                error!(
                    "[Tailscale] No IPv4 address found for device '{}'. Addresses found: {:?}",
                    hostname, addresses
                );
                format!("No IPv4 address found for Tailscale device '{}'", hostname)
            })?;

        debug!("[Tailscale] Found IPv4 '{}' for device '{}'", ipv4, hostname);
        Ok(ipv4.to_string())
    }

    async fn get_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        let tailnet = self
            .config
            .tailnet
            .clone()
            .ok_or_else(|| "tailscale_tailnet not found in config".to_string())?;

        let device = self.client.find_device_by_name(&tailnet, name, None).await?;

        Ok(device.map(|d| VpnDevice {
            name: d.name,
            hostname: d.hostname,
            addresses: d.addresses,
            tags: d.tags,
            created: d.created,
        }))
    }

    async fn remove_device_by_name(&self, name: &str) -> Result<Option<VpnDevice>, Box<dyn std::error::Error + Send + Sync>> {
        let tailnet = self
            .config
            .tailnet
            .clone()
            .ok_or_else(|| "tailscale_tailnet not found in config".to_string())?;

        debug!("Ensuring no existing Tailscale device for {}", name);

        let device = self.client.remove_device_by_name(&tailnet, name, None).await?;

        Ok(device.map(|d| VpnDevice {
            name: d.name,
            hostname: d.hostname,
            addresses: d.addresses,
            tags: d.tags,
            created: d.created,
        }))
    }

    async fn create_auth_key(&self, description: &str, capabilities: Option<VpnKeyCapabilities>) -> Result<VpnAuthKey, Box<dyn std::error::Error + Send + Sync>> {
        let tailnet = self
            .config
            .tailnet
            .clone()
            .ok_or_else(|| "tailscale_tailnet not found in config".to_string())?;

        debug!("Tailnet: {}", tailnet);

        // Convert our generic capabilities to Tailscale-specific ones
        let tailscale_capabilities = capabilities.map(|caps| Capabilities {
            devices: Devices {
                create: caps.devices.and_then(|dev_caps| {
                    dev_caps.create.map(|create_opts| CreateOpts {
                        reusable: create_opts.reusable,
                        ephemeral: create_opts.ephemeral,
                        preauthorized: create_opts.preauthorized,
                        tags: create_opts.tags,
                    })
                }),
            },
        });

        let request_body = CreateAuthKeyRequest {
            description: Some(description.to_string()),
            expirySeconds: None,
            capabilities: tailscale_capabilities.unwrap_or_else(|| Capabilities {
                devices: Devices {
                    create: Some(CreateOpts {
                        reusable: Some(false),
                        ephemeral: Some(true),
                        preauthorized: Some(true),
                        tags: Some(vec!["tag:container".to_string()]),
                    }),
                },
            }),
        };

        let response = match self.client.create_auth_key(&tailnet, true, &request_body).await {
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
        Ok(VpnAuthKey {
            key,
            description: response.description,
            created: response.created,
            expires: response.expires,
            capabilities: None, // Tailscale doesn't return capabilities in response
        })
    }

} 