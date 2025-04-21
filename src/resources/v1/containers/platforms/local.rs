use super::platform::{PlatformConnection, ShellConnection};
use crate::resources::v1::containers::models::ContainerModelVersion;
use anyhow::Context;

pub struct LocalShell<V: ContainerModelVersion> {
    version: std::marker::PhantomData<V>,
}

impl<V> PlatformConnection<V> for LocalShell<V>
where
    V: ContainerModelVersion,
{
    async fn from_spec(spec: V::ContainerPlatform) -> Self {
        LocalShell {
            version: std::marker::PhantomData,
        }
    }
    async fn connect(&self) -> anyhow::Result<()> {
        match self.is_connected().await {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Failed to connect to local shell")),
        }
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg("which sh")
            .output()
            .await?;
        output.status.success()
    }
}

impl<V> ShellConnection for LocalShell<V>
where
    V: ContainerModelVersion,
{
    async fn run_command(&self, command: &str) -> anyhow::Result<String> {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Command failed with status: {}",
                output.status
            ));
        }
        let stdout = String::from_utf8(&output.stdout).with_context(|| {
            format!("Output for 'sh -c {}' is incompatible with UTF-8.", command)
        })?;
        Ok(stdout)
    }
}
