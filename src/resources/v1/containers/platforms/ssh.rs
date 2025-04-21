use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::resources::v1::containers::platforms::platform::{PlatformConnection, ShellConnection};

pub struct SSHConnection<V: ContainerModelVersion> {
    version: std::marker::PhantomData<V>,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key: String,
}

impl<V> PlatformConnection<V> for SSHConnection<V> {
    async fn from_spec(spec: V::ContainerPlatform) -> Self {
        SSHConnection {
            version: std::marker::PhantomData,
            host: spec.ssh.host,
            port: spec.ssh.port,
            username: spec.ssh.username,
            private_key: spec.ssh.private_key,
        }
    }
    async fn connect(&self) -> anyhow::Result<()> {
        // Implement SSH connection logic here
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        // Implement SSH disconnection logic here
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        // Implement check for SSH connection status
        true
    }
}

impl<V> ShellConnection for SSHConnection<V> {
    async fn run_command(&self, method: &str) -> anyhow::Result<String> {
        // Implement SSH command execution logic here
        Ok(format!("Executed command: {}", method))
    }
}
