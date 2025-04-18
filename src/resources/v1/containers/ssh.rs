use crate::resources::v1::containers::platform::PlatformConnection;

pub struct SSHConnection {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key: String,
}
impl PlatformConnection for SSHConnection {
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

    async fn run_command(&self, method: &str) -> anyhow::Result<String> {
        // Implement SSH command execution logic here
        Ok(format!("Executed command: {}", method))
    }
}
