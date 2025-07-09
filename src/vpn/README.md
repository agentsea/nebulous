# VPN Abstraction Layer

This module provides a unified interface for VPN providers (Tailscale and Headscale) in the Nebulous system.

## Overview

The VPN abstraction layer allows Nebulous to work with different VPN providers through a common interface. Currently supported providers:

- **Tailscale**: Managed Tailscale service (fully implemented)
- **Headscale**: Open-source Tailscale alternative (⚠️ placeholder implementation)

## Architecture

### Core Components

1. **`VpnClient` trait**: The main interface that all VPN providers must implement
2. **`VpnConfig`**: Configuration structure for VPN providers
3. **Provider implementations**: Concrete implementations for each VPN provider

### Key Features

- **Unified API**: Same interface for all VPN providers
- **Device Management**: Create, remove, and query devices
- **Authentication**: Generate auth keys with specific capabilities
- **IP Discovery**: Get device IP addresses by hostname
- **Installation Scripts**: Provider-specific installation and startup commands

## Usage

### Configuration

The VPN provider is configured through environment variables:

```bash
# VPN Provider (tailscale or headscale)
export VPN_PROVIDER=tailscale

# API Key (supports multiple env var names for backward compatibility)
export VPN_API_KEY=your_api_key
# or
export TAILSCALE_API_KEY=your_tailscale_api_key

# Tailnet/Organization
export VPN_TAILNET=your_tailnet
# or
export TAILSCALE_TAILNET=your_tailnet

# Login Server (for Headscale)
export VPN_LOGIN_SERVER=http://headscale:50443

# Organization (for Headscale)
export VPN_ORGANIZATION=your_org
```

### Initialization

The VPN client is automatically initialized when the application starts:

```rust
// In main.rs
nebulous::vpn::init_vpn_from_config().await?;
```

### Using the VPN Client

```rust
use nebulous::vpn::{get_vpn_client, get_ip_for_device_hostname};

// Get the global VPN client
let client = get_vpn_client()?;

// Get device IP by hostname
let ip = client.get_device_ip("my-device").await?;

// Create an auth key
let auth_key = client.create_auth_key(
    "My container key",
    Some(capabilities)
).await?;

// Get device name for a container
let device_name = client.get_device_name("container-123");
```

### Provider-Specific Features

#### Tailscale

- Uses the official `tailscale-client` crate
- Supports all Tailscale API features
- Default login server: `https://login.tailscale.com`

#### Headscale

- **⚠️ PLACEHOLDER IMPLEMENTATION** - Currently raises `NotImplementedError` for all operations
- Uses HTTP API calls to Headscale server (when implemented)
- Compatible with Tailscale client binaries
- Customizable login server URL
- Supports organizations and user management

## Migration from Direct Tailscale Usage

### Before (Direct Tailscale)

```rust
use tailscale_client::TailscaleClient;

let client = TailscaleClient::new(api_key);
let device = client.find_device_by_name("-", hostname, None).await?;
```

### After (VPN Abstraction)

```rust
use nebulous::vpn::get_vpn_client;

let client = get_vpn_client()?;
let ip = client.get_device_ip(hostname).await?;
```

## Adding New VPN Providers

To add a new VPN provider:

1. Create a new module in `src/vpn/` (e.g., `src/vpn/wireguard.rs`)
2. Implement the `VpnClient` trait
3. Add the provider to the `VpnProvider` enum
4. Update the factory function in `create_vpn_client()`
5. Add configuration options to `VpnConfig`

Example:

```rust
// src/vpn/wireguard.rs
pub struct WireguardVpnClient {
    // Implementation
}

#[async_trait]
impl VpnClient for WireguardVpnClient {
    // Implement all required methods
}
```

## Environment Variables Reference

| Variable | Description | Default |
|----------|-------------|---------|
| `VPN_PROVIDER` | VPN provider (tailscale/headscale) | tailscale |
| `VPN_API_KEY` | API key for the VPN provider | - |
| `VPN_TAILNET` | Tailnet/organization name | - |
| `VPN_LOGIN_SERVER` | Login server URL | - |
| `VPN_ORGANIZATION` | Organization name (Headscale) | - |

### Backward Compatibility

The following legacy environment variables are still supported:

- `TAILSCALE_API_KEY` → `VPN_API_KEY`
- `TAILSCALE_TAILNET` → `VPN_TAILNET`
- `TS_API_KEY` → `VPN_API_KEY`

## Error Handling

All VPN operations return `Result<T, Box<dyn std::error::Error + Send + Sync>>` to handle provider-specific errors gracefully.

## Testing

To test with different providers:

```bash
# Test with Tailscale
export VPN_PROVIDER=tailscale
export VPN_API_KEY=your_tailscale_key

# Test with Headscale (PLACEHOLDER - will raise NotImplementedError)
export VPN_PROVIDER=headscale
export VPN_API_KEY=your_headscale_key
export VPN_LOGIN_SERVER=http://headscale:50443
``` 